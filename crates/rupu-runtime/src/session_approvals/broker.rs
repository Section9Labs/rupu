//! Authenticated CP-owned approval broker. Private worker channels can request
//! and consume one decision; they can never make an operator decision.
use super::{
    protocol::{self, ClientFrame, ServerFrame},
    store::{self, Store},
    view::ApprovalView,
};
use rupu_tools::approval::{ApprovalOwner, CallApprovalRequest};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    os::unix::net::UnixStream as StdStream,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{net::UnixStream, sync::Notify};

const MAX_SESSION_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OWNERS: usize = 256;
const MAX_CHANNELS: u64 = 256;

#[derive(Clone, Debug)]
struct Registration {
    session_id: String,
    workspace_id: String,
    workspace_path: PathBuf,
    worker_pid: u32,
    generation: u64,
    id: String,
}

#[derive(Debug)]
pub struct Broker {
    global: PathBuf,
    store_root: PathBuf,
    storage_gate: Arc<tokio::sync::Mutex<()>>,
    instance_id: String,
    epoch: String,
    ready: AtomicBool,
    generations: AtomicU64,
    channels: AtomicU64,
    live: Mutex<BTreeMap<String, Registration>>,
    changed: Notify,
    clock: Mutex<(Instant, u64)>,
}

#[derive(Deserialize)]
struct SessionOwner {
    session_id: String,
    #[serde(default)]
    builtin_harness: bool,
    workspace_id: String,
    workspace_path: PathBuf,
    permission_mode: String,
    status: String,
    active_run_id: Option<String>,
    worker_pid: Option<u32>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

fn failure() -> io::Error {
    io::Error::other("approval authority is unavailable or changed")
}

/// Bounded, no-symlink active record read. Never enumerate sessions or fall
/// back to archived/remote records to establish permission.
fn read_session(global: &Path, id: &str) -> io::Result<SessionOwner> {
    if !identifier(id) {
        return Err(failure());
    }
    let root = File::from(rustix::fs::open(
        global,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let sessions = rustix::fs::openat(
        &root,
        "sessions",
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let directory = rustix::fs::openat(
        &sessions,
        id,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let file = File::from(rustix::fs::openat(
        &directory,
        "session.json",
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_SESSION_BYTES {
        return Err(failure());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SESSION_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SESSION_BYTES {
        return Err(failure());
    }
    let owner: SessionOwner = serde_json::from_slice(&bytes).map_err(|_| failure())?;
    if owner.session_id != id || !owner.workspace_path.is_absolute() {
        return Err(failure());
    }
    Ok(owner)
}

fn session(global: &Path, id: &str) -> io::Result<SessionOwner> {
    let owner = read_session(global, id)?;
    if !owner.builtin_harness || owner.permission_mode != "ask" {
        return Err(failure());
    }
    Ok(owner)
}

pub fn fingerprint(value: &impl serde::Serialize) -> io::Result<String> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
            value => value,
        }
    }
    let value = serde_json::to_value(value).map_err(|_| failure())?;
    let bytes = serde_json::to_vec(&canonical(value)).map_err(|_| failure())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

pub(super) fn reviewable(value: &Value, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    match value {
        Value::Object(map) => map.iter().all(|(key, value)| {
            let key = key.to_ascii_lowercase();
            ![
                "password",
                "secret",
                "token",
                "authorization",
                "api_key",
                "apikey",
                "private_key",
                "credential",
            ]
            .iter()
            .any(|s| key.contains(s))
                && reviewable(value, depth + 1)
        }),
        Value::Array(a) => a.iter().all(|v| reviewable(v, depth + 1)),
        _ => true,
    }
}

impl Broker {
    /// Called only for explicit opt-in authenticated CP startup. A fresh epoch
    /// invalidates prior pending permits, but keeps their durable evidence.
    pub fn open(global: PathBuf) -> io::Result<Arc<Self>> {
        let global = global.canonicalize()?;
        let store_root = global.clone();
        let epoch = ulid::Ulid::new().to_string();
        let mut store = Store::open(&store_root).map_err(|_| failure())?;
        store
            .recover_epoch(&epoch, now_ms())
            .map_err(|_| failure())?;
        drop(store);
        Ok(Arc::new(Self {
            global,
            store_root,
            storage_gate: Arc::new(tokio::sync::Mutex::new(())),
            instance_id: ulid::Ulid::new().to_string(),
            epoch,
            ready: AtomicBool::new(true),
            generations: AtomicU64::new(1),
            channels: AtomicU64::new(0),
            live: Mutex::new(BTreeMap::new()),
            changed: Notify::new(),
            clock: Mutex::new((Instant::now(), now_ms())),
        }))
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
    pub fn now(&self) -> u64 {
        let Ok(mut clock) = self.clock.lock() else {
            return u64::MAX;
        };
        let value = now_ms().max(
            clock
                .1
                .saturating_add(clock.0.elapsed().as_millis().min(u64::MAX as u128) as u64),
        );
        *clock = (Instant::now(), value);
        value
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn session_workspace(&self, id: &str) -> io::Result<(String, PathBuf)> {
        let owner = session(&self.global, id)?;
        Ok((owner.workspace_id, owner.workspace_path))
    }
    pub fn channel_workspace(&self, id: &str) -> io::Result<Option<(String, PathBuf)>> {
        let owner = read_session(&self.global, id)?;
        if owner.permission_mode != "ask" {
            return Ok(None);
        }
        if !owner.builtin_harness {
            return Err(failure());
        }
        Ok(Some((owner.workspace_id, owner.workspace_path)))
    }

    async fn storage<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Store) -> store::Result<T> + Send + 'static,
    ) -> store::Result<T> {
        let root = self.store_root.clone();
        // One broker must not interpret its own concurrent read/decision lease
        // contention as worker authority loss. No operation is retried. Move
        // the owned guard into the actual job so caller cancellation cannot
        // release it while an already-dispatched blocking write still runs.
        let guard = self.storage_gate.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            operation(&mut Store::open(&root)?)
        })
        .await
        .map_err(|_| store::Error::Invalid)?
    }

    pub async fn record(
        &self,
        session_id: String,
        approval_id: String,
    ) -> store::Result<store::Record> {
        let now = self.now();
        self.storage(move |s| s.refresh(&session_id, &approval_id, now))
            .await
    }
    pub async fn list(
        &self,
        session_id: String,
        cursor: Option<String>,
        limit: usize,
    ) -> store::Result<store::Page> {
        self.storage(move |s| s.list(&session_id, cursor.as_deref(), limit))
            .await
    }
    pub fn live_record(&self, record: &store::Record) -> bool {
        if !self.is_ready()
            || record.intent.cp_instance_id != self.instance_id
            || record.intent.broker_epoch != self.epoch
        {
            return false;
        }
        let registration = self
            .live
            .lock()
            .ok()
            .and_then(|map| map.get(&record.intent.worker_registration_id).cloned());
        registration.is_some_and(|r| {
            r.generation == record.intent.worker_generation
                && self
                    .live_owner(
                        &r,
                        &ApprovalOwner {
                            session_id: record.intent.session_id.clone(),
                            root_turn_run_id: record.intent.root_turn_run_id.clone(),
                            run_id: record.intent.run_id.clone(),
                            parent_run_id: record.intent.parent_run_id.clone(),
                            root_run_id: record.intent.root_run_id.clone(),
                            workspace_id: record.intent.workspace_id.clone(),
                            workspace_path: record.intent.workspace_path.clone(),
                            settings_revision: record.intent.settings_revision,
                            orchestration_revision: record.intent.orchestration_revision,
                            host_mcp_registry_fingerprint: record
                                .intent
                                .host_mcp_registry_fingerprint
                                .clone(),
                        },
                    )
                    .is_ok()
        })
    }
    pub async fn decide(
        self: &Arc<Self>,
        session_id: String,
        id: String,
        request: store::DecisionRequest,
    ) -> store::Result<store::Record> {
        let record = self.record(session_id.clone(), id.clone()).await?;
        // Exact retry lookup returns the existing receipt even after owner
        // revocation; it cannot issue another dispatch reservation.
        let replay = record
            .decision
            .as_ref()
            .is_some_and(|d| d.request == request);
        let check_broker = self.clone();
        let check_record = record.clone();
        let live = tokio::task::spawn_blocking(move || check_broker.live_record(&check_record))
            .await
            .map_err(|_| store::Error::Invalid)?;
        if !replay && !live {
            return Err(store::Error::Conflict);
        }
        let now = self.now();
        let record = self
            .storage(move |s| s.decide(&session_id, &id, request, now))
            .await?;
        self.changed.notify_waiters();
        Ok(record)
    }
    pub async fn revoke(
        self: &Arc<Self>,
        session_id: String,
        id: String,
        request: store::RevokeRequest,
    ) -> store::Result<store::Record> {
        let record = self.record(session_id.clone(), id.clone()).await?;
        let replay = record.revoke.as_ref().is_some_and(|d| d.request == request);
        let check_broker = self.clone();
        let check_record = record.clone();
        let live = tokio::task::spawn_blocking(move || check_broker.live_record(&check_record))
            .await
            .map_err(|_| store::Error::Invalid)?;
        if !replay && !live {
            return Err(store::Error::Conflict);
        }
        let now = self.now();
        let record = self
            .storage(move |s| s.revoke(&session_id, &id, request, now))
            .await?;
        self.changed.notify_waiters();
        Ok(record)
    }

    /// The returned stream is passed only as the intended child process's stdin.
    /// Runtime consumes it, restores stdin and marks the private duplicate CLOEXEC.
    pub fn spawn_channel(
        self: &Arc<Self>,
        expected_session: Option<String>,
        workspace_id: String,
        workspace_path: PathBuf,
    ) -> io::Result<StdStream> {
        if !self.is_ready()
            || !identifier(&workspace_id)
            || !workspace_path.is_absolute()
            || expected_session.as_ref().is_some_and(|id| !identifier(id))
        {
            return Err(failure());
        }
        let workspace_path = workspace_path.canonicalize()?;
        let (server, client) = StdStream::pair()?;
        server.set_nonblocking(true)?;
        let server = UnixStream::from_std(server)?;
        if self.channels.fetch_add(1, Ordering::AcqRel) >= MAX_CHANNELS {
            self.channels.fetch_sub(1, Ordering::AcqRel);
            return Err(failure());
        }
        let broker = self.clone();
        tokio::spawn(async move {
            let _ = broker
                .serve_channel(server, expected_session, workspace_id, workspace_path)
                .await;
            broker.channels.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(client)
    }

    fn live_owner(&self, registration: &Registration, owner: &ApprovalOwner) -> io::Result<()> {
        if !self.is_ready()
            || owner.session_id != registration.session_id
            || owner.workspace_id != registration.workspace_id
            || Path::new(&owner.workspace_path).canonicalize()? != registration.workspace_path
            || !identifier(&owner.run_id)
            || owner.root_turn_run_id != owner.root_run_id
        {
            return Err(failure());
        }
        let current = session(&self.global, &registration.session_id)?;
        if current.workspace_id != registration.workspace_id
            || current.workspace_path.canonicalize()? != registration.workspace_path
            || current.worker_pid != Some(registration.worker_pid)
            || current.active_run_id.as_deref() != Some(owner.root_turn_run_id.as_str())
            || current.status != "running"
        {
            return Err(failure());
        }
        if let Some(expected) = &owner.host_mcp_registry_fingerprint {
            let registry = rupu_config::mcp::Registry::load(&self.global).map_err(|_| failure())?;
            if super::authority::fingerprint(&self.global, &registry)? != *expected {
                return Err(failure());
            }
        }
        Ok(())
    }

    async fn check_live_owner(
        self: &Arc<Self>,
        registration: &Registration,
        owner: &ApprovalOwner,
    ) -> io::Result<()> {
        let broker = self.clone();
        let registration = registration.clone();
        let owner = owner.clone();
        tokio::task::spawn_blocking(move || broker.live_owner(&registration, &owner))
            .await
            .map_err(|_| failure())?
    }

    async fn serve_channel(
        self: &Arc<Self>,
        mut stream: UnixStream,
        expected_session: Option<String>,
        workspace_id: String,
        workspace_path: PathBuf,
    ) -> io::Result<()> {
        let bind = tokio::time::timeout(
            Duration::from_secs(30),
            protocol::read_frame::<_, ClientFrame>(&mut stream),
        )
        .await
        .map_err(|_| failure())??;
        let ClientFrame::Bind {
            session_id,
            worker_pid,
        } = bind
        else {
            return Err(failure());
        };
        if expected_session
            .as_ref()
            .is_some_and(|id| id != &session_id)
            || worker_pid == 0
        {
            return Err(failure());
        }
        let current = session(&self.global, &session_id)?;
        if current.workspace_id != workspace_id
            || current.workspace_path.canonicalize()? != workspace_path
            || current.worker_pid != Some(worker_pid)
        {
            return Err(failure());
        }
        let registration = Registration {
            session_id,
            workspace_id,
            workspace_path,
            worker_pid,
            generation: self.generations.fetch_add(1, Ordering::AcqRel),
            id: ulid::Ulid::new().to_string(),
        };
        self.live
            .lock()
            .map_err(|_| failure())?
            .insert(registration.id.clone(), registration.clone());
        let result = self.channel_loop(&mut stream, &registration).await;
        if let Ok(mut map) = self.live.lock() {
            map.remove(&registration.id);
        }
        self.changed.notify_waiters();
        result
    }

    async fn channel_loop(
        self: &Arc<Self>,
        stream: &mut UnixStream,
        registration: &Registration,
    ) -> io::Result<()> {
        protocol::write_frame(
            stream,
            &ServerFrame::Bound {
                instance_id: self.instance_id.clone(),
                broker_epoch: self.epoch.clone(),
                worker_generation: registration.generation,
            },
        )
        .await?;
        let mut owners = BTreeMap::<String, ApprovalOwner>::new();
        loop {
            match protocol::read_frame::<_, ClientFrame>(stream).await? {
                ClientFrame::CheckOwner { owner } => {
                    self.check_live_owner(registration, &owner).await?;
                    if owner.parent_run_id.is_none() {
                        owners.retain(|_, known| known.root_turn_run_id == owner.root_turn_run_id);
                    }
                    let lineage = match &owner.parent_run_id {
                        None => owner.run_id == owner.root_turn_run_id,
                        Some(parent) => owners.get(parent).is_some_and(|p| {
                            p.root_turn_run_id == owner.root_turn_run_id
                                && p.session_id == owner.session_id
                        }),
                    };
                    if !lineage
                        || owners.len() >= MAX_OWNERS && !owners.contains_key(&owner.run_id)
                        || owners.get(&owner.run_id).is_some_and(|old| old != &owner)
                    {
                        return Err(failure());
                    }
                    owners.insert(owner.run_id.clone(), owner);
                    protocol::write_frame(stream, &ServerFrame::Ready {}).await?;
                }
                ClientFrame::Request { call } => {
                    if owners.get(&call.owner.run_id) != Some(&call.owner) {
                        return Err(failure());
                    }
                    self.check_live_owner(registration, &call.owner).await?;
                    self.request(stream, registration, call).await?;
                }
                _ => return Err(failure()),
            }
        }
    }

    async fn request(
        self: &Arc<Self>,
        stream: &mut UnixStream,
        registration: &Registration,
        call: CallApprovalRequest,
    ) -> io::Result<()> {
        if !reviewable(&call.input, 0) {
            protocol::write_frame(
                stream,
                &ServerFrame::Denied {
                    reason: "Input contains fields that cannot be safely presented for approval."
                        .into(),
                },
            )
            .await?;
            return Ok(());
        }
        let now = self.now();
        let owner = call.owner.clone();
        let remote = if let Some(remote) = &call.remote {
            if owner.host_mcp_registry_fingerprint.is_none() {
                return Err(failure());
            }
            let registry = rupu_config::mcp::Registry::load(&self.global).map_err(|_| failure())?;
            let config = registry
                .enabled_server(&remote.server_id)
                .map_err(|_| failure())?;
            // Environment credentials have no revocable metadata revision.
            // They remain usable in existing modes, but cannot obtain an Ask permit.
            if config.bearer_env.is_some() {
                protocol::write_frame(stream, &ServerFrame::Denied {
                    reason: "This MCP server uses an environment credential without a revocable revision. Ask requires a host credential-store binding or an unauthenticated server.".into(),
                }).await?;
                return Ok(());
            }
            Some(store::RemoteIdentity {
                server_id: remote.server_id.clone(),
                tool: remote.tool.clone(),
                frozen_fingerprint: call.descriptor_fingerprint.clone(),
                credential_binding_generation: config.bearer_store.clone(),
            })
        } else {
            None
        };
        let review = json!({"version":1,"tool":call.tool,"called_as":call.called_as,"effect":call.effect,"arguments":call.input});
        let intent = store::Intent {
            approval_id: ulid::Ulid::new().to_string(),
            cp_instance_id: self.instance_id.clone(),
            broker_epoch: self.epoch.clone(),
            session_id: owner.session_id.clone(),
            root_turn_run_id: owner.root_turn_run_id.clone(),
            run_id: owner.run_id.clone(),
            turn_index: call.turn_index,
            tool_call_id: call.tool_call_id,
            call_sequence: call.call_sequence,
            workspace_id: owner.workspace_id.clone(),
            workspace_path: owner.workspace_path.clone(),
            worker_registration_id: registration.id.clone(),
            worker_generation: registration.generation,
            parent_run_id: owner.parent_run_id.clone(),
            root_run_id: owner.root_run_id.clone(),
            lineage_fingerprint: fingerprint(&owner)?,
            tool: call.tool,
            called_as: call.called_as,
            effect: match call.effect {
                rupu_tools::approval::ApprovalEffect::Write => store::Effect::Write,
                rupu_tools::approval::ApprovalEffect::External => store::Effect::External,
            },
            descriptor_fingerprint: call.descriptor_fingerprint,
            grant_fingerprint: call.grant_fingerprint,
            settings_revision: owner.settings_revision,
            orchestration_revision: owner.orchestration_revision,
            approval_authority_revision: 0,
            host_mcp_registry_fingerprint: owner.host_mcp_registry_fingerprint.clone(),
            remote,
            input: call.input,
            review,
            created_at_ms: now,
            expires_at_ms: now.saturating_add(store::MAX_LIFETIME_MS),
        };
        let record = self
            .storage(move |s| s.create(intent, now))
            .await
            .map_err(|_| failure())?;
        let session_id = record.intent.session_id.clone();
        let id = record.intent.approval_id.clone();
        let result = self
            .wait_and_dispatch(stream, registration, &owner, record)
            .await;
        if result.is_err() {
            let now = self.now();
            let _ = self
                .storage(move |s| s.cancel(&session_id, &id, "worker channel closed", now))
                .await;
        }
        result
    }

    async fn wait_and_dispatch(
        self: &Arc<Self>,
        stream: &mut UnixStream,
        registration: &Registration,
        owner: &ApprovalOwner,
        mut record: store::Record,
    ) -> io::Result<()> {
        protocol::write_frame(
            stream,
            &ServerFrame::Pending {
                event: serde_json::to_value(ApprovalView::from_record(
                    &record,
                    false,
                    true,
                    self.now(),
                ))
                .map_err(|_| failure())?,
            },
        )
        .await?;
        loop {
            self.check_live_owner(registration, owner).await?;
            if record.state == store::State::Allowed {
                break;
            }
            if !record.state.is_waiting() {
                protocol::write_frame(
                    stream,
                    &ServerFrame::Denied {
                        reason: record
                            .reason
                            .clone()
                            .unwrap_or_else(|| "Tool call was not approved.".into()),
                    },
                )
                .await?;
                return Ok(());
            }
            tokio::select! {
                _ = self.changed.notified() => {},
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                ready = stream.readable() => {
                    ready?;
                    let mut byte = [0];
                    match stream.try_read(&mut byte) {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {},
                        _ => return Err(failure()),
                    }
                }
            }
            record = self
                .record(
                    record.intent.session_id.clone(),
                    record.intent.approval_id.clone(),
                )
                .await
                .map_err(|_| failure())?;
        }
        self.check_live_owner(registration, owner).await?;
        let session_id = record.intent.session_id.clone();
        let id = record.intent.approval_id.clone();
        let pre = store::PreDispatch {
            intent_fingerprint: record.intent_fingerprint.clone(),
            worker_registration_id: registration.id.clone(),
            worker_generation: registration.generation,
            broker_epoch: self.epoch.clone(),
            approval_authority_revision: 0,
        };
        let reserve_broker = self.clone();
        let reserve_registration = registration.clone();
        let reserve_owner = owner.clone();
        let ticket = self
            .storage(move |s| {
                // Recheck inside the blocking reservation job after its queue wait.
                // External configuration is not transactionally locked by this lease.
                let live = reserve_broker
                    .live
                    .lock()
                    .map_err(|_| store::Error::Invalid)?;
                if !live
                    .get(&reserve_registration.id)
                    .is_some_and(|r| r.generation == reserve_registration.generation)
                {
                    return Err(store::Error::Conflict);
                }
                reserve_broker
                    .live_owner(&reserve_registration, &reserve_owner)
                    .map_err(|_| store::Error::Conflict)?;
                s.reserve(&session_id, &id, &pre, reserve_broker.now())
            })
            .await
            .map_err(|_| failure())?;
        let reserved = ticket.record();
        protocol::write_frame(
            stream,
            &ServerFrame::Reserved {
                approval_id: reserved.intent.approval_id.clone(),
                intent_fingerprint: reserved.intent_fingerprint.clone(),
            },
        )
        .await?;
        let frame = protocol::read_frame::<_, ClientFrame>(stream).await?;
        let ClientFrame::Complete {
            approval_id,
            intent_fingerprint,
            outcome,
        } = frame
        else {
            return Err(failure());
        };
        if approval_id != reserved.intent.approval_id
            || intent_fingerprint != reserved.intent_fingerprint
        {
            return Err(failure());
        }
        let outcome = store::Completion {
            success: outcome.success,
            error: outcome.error,
            result_fingerprint: outcome.result_fingerprint,
        };
        let now = self.now();
        self.storage(move |s| s.complete(ticket, outcome, now))
            .await
            .map_err(|_| failure())?;
        protocol::write_frame(stream, &ServerFrame::Recorded {}).await
    }
}

#[cfg(test)]
#[tokio::test]
async fn cancelled_observer_does_not_release_running_storage_job() {
    let root = tempfile::tempdir().unwrap();
    let broker = Broker::open(root.path().into()).unwrap();
    let operation_broker = broker.clone();
    let (started, observed) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let task = tokio::spawn(async move {
        operation_broker
            .storage(move |_| {
                let _ = started.send(());
                held.recv().map_err(|_| store::Error::Invalid)?;
                Ok(())
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), observed)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        broker.storage_gate.try_lock().is_err(),
        "the actual blocking operation still owns serialization"
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if broker.storage_gate.try_lock().is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
