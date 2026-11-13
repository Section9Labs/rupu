//! Point-in-time observation of cooperating local scheduler step bodies.
//!
//! A saved record is not liveness. Readers witness a generation-specific OS
//! lock while holding the mutation lock. The lifetime owner lock excludes
//! other cooperating producers; old binaries and arbitrary writers do not
//! participate in this advisory protocol. These observations say nothing
//! about provider progress or child work that outlives its scheduler body.
use crate::runs::{RunRecord, RunStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io,
    path::Path,
    sync::{Arc, Mutex},
};
use ulid::Ulid;

mod disk;
#[cfg(test)]
mod tests;

pub const FEATURE: &str = "workflow.observation.executing_steps.v1";
pub const SCOPE: &str = "scheduler_step_bodies";
pub const SNAPSHOT_FILE: &str = "execution-observation.json";
pub const OWNER_LOCK_FILE: &str = "execution-owner.lock";
pub const MUTATION_LOCK_FILE: &str = "execution-mutation.lock";
pub const CANCELLATION_FILE: &str = "execution-cancelled.json";
pub const MAX_MEMBERS: usize = 256;
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
const MAX_OWNER_BYTES: usize = 256 * 1024;

pub const fn supported() -> bool {
    cfg!(unix)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionValidity {
    Current,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIteration {
    pub loop_id: String,
    pub iteration: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionMember {
    pub activation_id: String,
    pub step_id: String,
    pub iterations: Vec<ExecutionIteration>,
    pub entered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionObservation {
    pub schema: String,
    pub scope: String,
    pub run_id: String,
    pub workspace_id: String,
    pub host_id: String,
    pub owner_started_at: DateTime<Utc>,
    pub execution_generation: Option<String>,
    /// Monotonic within this generation, encoded without JSON number rounding.
    pub revision: Option<String>,
    /// Scheduler publication time, never a synthesized reader timestamp.
    pub observed_at: Option<DateTime<Utc>>,
    pub validity: ExecutionValidity,
    pub complete: bool,
    pub reason: Option<String>,
    pub members: Vec<ExecutionMember>,
}

#[derive(Debug, Clone)]
pub struct ExecutionRead {
    pub envelope: ExecutionObservation,
    pub execution_input_bytes: usize,
}

impl ExecutionObservation {
    fn unavailable(record: &RunRecord, reason: &str) -> Self {
        Self {
            schema: FEATURE.into(),
            scope: SCOPE.into(),
            run_id: record.id.clone(),
            workspace_id: record.workspace_id.clone(),
            host_id: "local".into(),
            owner_started_at: record.started_at,
            execution_generation: None,
            revision: None,
            observed_at: None,
            validity: ExecutionValidity::Unavailable,
            complete: false,
            reason: Some(reason.into()),
            members: Vec::new(),
        }
    }
    fn qualifies(mut self, validity: ExecutionValidity, reason: &str) -> Self {
        self.validity = validity;
        self.complete = false;
        self.reason = Some(reason.into());
        self
    }
    fn matches(&self, record: &RunRecord) -> bool {
        self.run_id == record.id
            && self.workspace_id == record.workspace_id
            && self.owner_started_at == record.started_at
    }
    fn validate(&self) -> io::Result<()> {
        let generation = self
            .execution_generation
            .as_deref()
            .ok_or_else(|| invalid("generation missing"))?;
        let revision = self
            .revision
            .as_deref()
            .ok_or_else(|| invalid("revision missing"))?;
        let parsed = revision
            .parse::<u64>()
            .map_err(|_| invalid("invalid revision"))?;
        let mut ids = std::collections::BTreeSet::new();
        if self.schema != FEATURE
            || self.scope != SCOPE
            || self.host_id != "local"
            || !identifier(&self.run_id, 128)
            || !identifier(&self.workspace_id, 128)
            || !opaque_id(generation)
            || revision != parsed.to_string()
            || parsed == 0
            || self.observed_at.is_none()
            || self.members.len() > MAX_MEMBERS
            || self.reason.as_ref().is_some_and(|s| s.len() > 256)
            || (self.validity == ExecutionValidity::Current
                && (!self.complete || self.reason.is_some()))
            || (self.validity != ExecutionValidity::Current && self.complete)
        {
            return Err(invalid("invalid execution observation"));
        }
        for member in &self.members {
            validate_member(&member.step_id, &member.iterations)?;
            if !opaque_id(&member.activation_id)
                || !ids.insert(&member.activation_id)
                || Some(member.entered_at) > self.observed_at
            {
                return Err(invalid("invalid execution member"));
            }
        }
        Ok(())
    }
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn opaque_id(value: &str) -> bool {
    value.len() == 26 && Ulid::from_string(value).is_ok_and(|id| id.to_string() == value)
}
fn validate_member(step: &str, iterations: &[ExecutionIteration]) -> io::Result<()> {
    if !identifier(step, 1024)
        || iterations.len() > 16
        || iterations.iter().any(|v| !identifier(&v.loop_id, 256))
    {
        return Err(invalid("invalid execution coordinates"));
    }
    let mut names = std::collections::BTreeSet::new();
    if iterations.iter().any(|v| !names.insert(&v.loop_id)) {
        return Err(invalid("duplicate execution coordinates"));
    }
    Ok(())
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn generation_lock(generation: &str) -> String {
    format!("execution-generation-{generation}.lock")
}
fn revoked_file(generation: &str) -> String {
    format!("execution-generation-{generation}.revoked")
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancellation {
    run_id: String,
    workspace_id: String,
    owner_started_at: DateTime<Utc>,
}
impl Cancellation {
    fn for_record(record: &RunRecord) -> Self {
        Self {
            run_id: record.id.clone(),
            workspace_id: record.workspace_id.clone(),
            owner_started_at: record.started_at,
        }
    }
}

fn current_record(dir: &File, expected: &RunRecord) -> io::Result<RunRecord> {
    let mut bytes = 0;
    let data = disk::read(dir, "run.json", MAX_OWNER_BYTES, &mut bytes)?
        .ok_or_else(|| invalid("run owner missing"))?;
    let actual: RunRecord =
        serde_json::from_slice(&data).map_err(|_| invalid("run owner malformed"))?;
    if actual.id != expected.id
        || actual.workspace_id != expected.workspace_id
        || actual.started_at != expected.started_at
        || actual.workflow_name != expected.workflow_name
        || actual.workspace_path != expected.workspace_path
        || actual.transcript_dir != expected.transcript_dir
    {
        return Err(invalid("run owner changed"));
    }
    Ok(actual)
}

struct State {
    snapshot: ExecutionObservation,
    owner_lock: Option<disk::OwnedLock>,
    generation_lock: Option<disk::OwnedLock>,
    active: bool,
}
struct Inner {
    dir: File,
    expected: RunRecord,
    state: Mutex<State>,
}

/// One scheduler invocation. Handles do not extend its authority after drop.
pub struct ExecutionOwner {
    inner: Arc<Inner>,
    finished: bool,
}
#[derive(Clone)]
pub struct ExecutionHandle {
    inner: Arc<Inner>,
}
pub struct ExecutionGuard {
    handle: ExecutionHandle,
    activation: String,
    finished: bool,
}

impl ExecutionOwner {
    pub fn start(run_dir: &Path, record: &RunRecord) -> io::Result<Self> {
        let dir = disk::open_run_directory(run_dir)?;
        let _mutation = disk::mutation_lock(&dir, true)?;
        let owner_lock = disk::lock(&dir, OWNER_LOCK_FILE, true)?;
        let current = current_record(&dir, record)?;
        if current.status == RunStatus::Cancelled || disk::exists(&dir, CANCELLATION_FILE)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "execution cancelled",
            ));
        }
        let generation = Ulid::new().to_string();
        let generation_lock = disk::new_lock(&dir, &generation_lock(&generation))?;
        let mut snapshot = ExecutionObservation::unavailable(record, "initializing");
        snapshot.execution_generation = Some(generation);
        snapshot.revision = Some("1".into());
        snapshot.observed_at = Some(Utc::now());
        snapshot.validity = ExecutionValidity::Current;
        snapshot.complete = true;
        snapshot.reason = None;
        publish(&dir, &snapshot)?;
        Ok(Self {
            inner: Arc::new(Inner {
                dir,
                expected: current,
                state: Mutex::new(State {
                    snapshot,
                    owner_lock: Some(owner_lock),
                    generation_lock: Some(generation_lock),
                    active: true,
                }),
            }),
            finished: false,
        })
    }
    pub fn handle(&self) -> ExecutionHandle {
        ExecutionHandle {
            inner: Arc::clone(&self.inner),
        }
    }
    pub fn invalidate(&self, reason: &str) {
        self.handle().invalidate(reason);
    }
    pub fn finish(mut self) -> io::Result<()> {
        let result = self.inner.finish_owner();
        self.finished = true;
        result
    }
}
impl Drop for ExecutionOwner {
    fn drop(&mut self) {
        if !self.finished {
            self.inner.revoke("owner_dropped", true);
        }
    }
}

impl ExecutionHandle {
    /// A local hint only. Admission always rechecks durable revocation.
    pub fn is_valid(&self) -> bool {
        self.inner.state.lock().is_ok_and(|s| s.active)
    }
    pub fn invalidate(&self, reason: &str) {
        self.inner.revoke(reason, false);
    }
    pub fn enter(
        &self,
        step_id: &str,
        iterations: Vec<ExecutionIteration>,
    ) -> io::Result<ExecutionGuard> {
        let activation = Ulid::new().to_string();
        let result = self.inner.change(|snapshot| {
            validate_member(step_id, &iterations)?;
            if snapshot.members.len() >= MAX_MEMBERS {
                return Err(invalid("execution member limit"));
            }
            snapshot.members.push(ExecutionMember {
                activation_id: activation.clone(),
                step_id: step_id.into(),
                iterations,
                entered_at: Utc::now(),
            });
            Ok(())
        });
        if result.is_err() {
            self.invalidate("admission_failed");
        }
        result.map(|()| ExecutionGuard {
            handle: self.clone(),
            activation,
            finished: false,
        })
    }
}
impl ExecutionGuard {
    pub fn finish(mut self) -> io::Result<()> {
        let result = self.handle.inner.change(|snapshot| {
            let index = snapshot
                .members
                .iter()
                .position(|v| v.activation_id == self.activation)
                .ok_or_else(|| invalid("activation no longer owned"))?;
            snapshot.members.remove(index);
            Ok(())
        });
        self.finished = true;
        if result.is_err() {
            self.handle.invalidate("completion_failed");
        }
        result
    }
}
impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.handle.invalidate("unfinished_activation");
        }
    }
}

impl Inner {
    fn change(
        &self,
        apply: impl FnOnce(&mut ExecutionObservation) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("execution state poisoned"))?;
        if !state.active {
            return Err(invalid("execution generation revoked"));
        }
        let _mutation = disk::mutation_lock(&self.dir, false)?;
        self.check(&state.snapshot)?;
        let mut next = state.snapshot.clone();
        apply(&mut next)?;
        advance(&mut next)?;
        publish(&self.dir, &next)?;
        state.snapshot = next;
        Ok(())
    }
    fn check(&self, snapshot: &ExecutionObservation) -> io::Result<()> {
        let current = current_record(&self.dir, &self.expected)?;
        if current.status == RunStatus::Cancelled || disk::exists(&self.dir, CANCELLATION_FILE)? {
            return Err(invalid("execution cancelled"));
        }
        let generation = snapshot
            .execution_generation
            .as_deref()
            .ok_or_else(|| invalid("generation missing"))?;
        if disk::exists(&self.dir, &revoked_file(generation))? {
            return Err(invalid("execution generation revoked"));
        }
        let mut bytes = 0;
        let data = disk::read(&self.dir, SNAPSHOT_FILE, MAX_SNAPSHOT_BYTES, &mut bytes)?
            .ok_or_else(|| invalid("execution snapshot missing"))?;
        let current: ExecutionObservation =
            serde_json::from_slice(&data).map_err(|_| invalid("execution snapshot malformed"))?;
        if current != *snapshot {
            return Err(invalid("execution snapshot replaced"));
        }
        Ok(())
    }
    fn revoke(&self, reason: &str, release_owner: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active = false;
        // Releasing this exact generation lock is the fail-closed fallback
        // even if disk persistence, mutation locking, or bounds failed.
        state.generation_lock.take();
        if let Some(generation) = state.snapshot.execution_generation.as_deref() {
            let _ = disk::marker(&self.dir, &revoked_file(generation));
        }
        if let Ok(_mutation) = disk::lock(&self.dir, MUTATION_LOCK_FILE, false) {
            let mut bytes = 0;
            let ours = disk::read(&self.dir, SNAPSHOT_FILE, MAX_SNAPSHOT_BYTES, &mut bytes)
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_slice::<ExecutionObservation>(&v).ok())
                .is_some_and(|v| v.execution_generation == state.snapshot.execution_generation);
            if ours {
                let safe_reason = if identifier(reason, 256) {
                    reason
                } else {
                    "generation_revoked"
                };
                state.snapshot.validity = ExecutionValidity::Stale;
                state.snapshot.complete = false;
                state.snapshot.reason = Some(safe_reason.into());
                if advance(&mut state.snapshot).is_ok() {
                    let _ = publish(&self.dir, &state.snapshot);
                }
            }
        }
        if release_owner {
            state.owner_lock.take();
        }
    }
    fn finish_owner(&self) -> io::Result<()> {
        let result = (|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("execution state poisoned"))?;
            if !state.active || !state.snapshot.members.is_empty() {
                return Err(invalid("execution owner has unfinished bodies"));
            }
            let _mutation = disk::mutation_lock(&self.dir, false)?;
            self.check(&state.snapshot)?;
            state.snapshot.validity = ExecutionValidity::Stale;
            state.snapshot.complete = false;
            state.snapshot.reason = Some("scheduler_finished".into());
            advance(&mut state.snapshot)?;
            publish(&self.dir, &state.snapshot)?;
            state.active = false;
            state.generation_lock.take();
            state.owner_lock.take();
            Ok(())
        })();
        if result.is_err() {
            self.revoke("owner_finish_failed", true);
        }
        result
    }
}

fn advance(snapshot: &mut ExecutionObservation) -> io::Result<()> {
    let current = snapshot
        .revision
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| invalid("revision missing"))?;
    snapshot.revision = Some(
        current
            .checked_add(1)
            .ok_or_else(|| invalid("execution revision exhausted"))?
            .to_string(),
    );
    snapshot.observed_at = Some(Utc::now());
    Ok(())
}
fn publish(dir: &File, snapshot: &ExecutionObservation) -> io::Result<()> {
    snapshot.validate()?;
    let data = serde_json::to_vec(snapshot).map_err(io::Error::other)?;
    if data.len() > MAX_SNAPSHOT_BYTES {
        return Err(invalid("execution snapshot exceeds limit"));
    }
    disk::atomic_write(dir, SNAPSHOT_FILE, &data)
}

/// Revoke before publishing a cancel. The owner-scoped barrier also prevents a
/// first producer starting after cancellation when no snapshot existed yet.
pub fn revoke_for_owner(run_dir: &Path, record: &RunRecord, _reason: &str) -> io::Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (run_dir, record);
        return Ok(());
    }
    #[cfg(unix)]
    {
        let dir = disk::open_run_directory(run_dir)?;
        let _mutation = disk::mutation_lock(&dir, true)?;
        current_record(&dir, record)?;
        let barrier =
            serde_json::to_vec(&Cancellation::for_record(record)).map_err(io::Error::other)?;
        disk::atomic_write(&dir, CANCELLATION_FILE, &barrier)?;
        // The barrier is authoritative even when the optional snapshot is absent
        // or malformed. Cancellation preserves the legacy run's operation.
        Ok(())
    }
}

/// Strict mutation proof shared by explicit retry and pre-body failure
/// finalization. Only the terminal-to-running CAS calls `clear_cancelled`.
/// Starting a producer never clears a barrier because its record says Running.
#[cfg(unix)]
pub(crate) struct RetryGuard {
    dir: File,
    _mutation: disk::OwnedLock,
}
#[cfg(unix)]
impl RetryGuard {
    pub(crate) fn acquire(run_dir: &Path) -> io::Result<Self> {
        let dir = disk::open_run_directory(run_dir)?;
        let mutation = disk::mutation_lock(&dir, true)?;
        Ok(Self {
            dir,
            _mutation: mutation,
        })
    }
    pub(crate) fn load(&self) -> io::Result<RunRecord> {
        let mut bytes = 0;
        let data = disk::read(&self.dir, "run.json", MAX_OWNER_BYTES, &mut bytes)?
            .ok_or_else(|| invalid("run owner missing"))?;
        serde_json::from_slice(&data).map_err(|_| invalid("run owner malformed"))
    }
    pub(crate) fn idle_owner(&self, record: &RunRecord) -> io::Result<disk::OwnedLock> {
        let idle = disk::lock(&self.dir, OWNER_LOCK_FILE, true)?;
        let mut bytes = 0;
        if let Some(data) = disk::read(&self.dir, CANCELLATION_FILE, 4096, &mut bytes)? {
            let barrier: Cancellation =
                serde_json::from_slice(&data).map_err(|_| invalid("cancel barrier malformed"))?;
            if barrier.run_id != record.id
                || barrier.workspace_id != record.workspace_id
                || barrier.owner_started_at != record.started_at
            {
                return Err(invalid("cancel barrier owner mismatch"));
            }
        }
        Ok(idle)
    }
    pub(crate) fn clear_cancelled(&self) -> io::Result<()> {
        disk::remove(&self.dir, CANCELLATION_FILE)
    }
}

/// Capture one immutable envelope through the caller's already pinned run
/// directory. `current` is a witnessed timepoint, not a promise of future work.
pub fn read_for_owner(dir: &File, record: &RunRecord, archived: bool) -> ExecutionRead {
    let mut bytes = 0;
    let result = read_inner(dir, record, archived, &mut bytes);
    let envelope = match result {
        Ok(value) => value,
        Err(reason) => ExecutionObservation::unavailable(record, reason),
    };
    ExecutionRead {
        envelope,
        execution_input_bytes: bytes,
    }
}
fn read_inner(
    dir: &File,
    record: &RunRecord,
    archived: bool,
    bytes: &mut usize,
) -> Result<ExecutionObservation, &'static str> {
    // Missing old-producer records do not create locks or any files.
    if !disk::exists(dir, SNAPSHOT_FILE).map_err(|_| "observation_io")? {
        return Err("observation_missing");
    }
    let _mutation = disk::lock(dir, MUTATION_LOCK_FILE, false).map_err(|e| {
        if e.kind() == io::ErrorKind::WouldBlock {
            "observation_busy"
        } else {
            "observation_io"
        }
    })?;
    let data = disk::read(dir, SNAPSHOT_FILE, MAX_SNAPSHOT_BYTES, bytes)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::FileTooLarge {
                "observation_oversized"
            } else {
                "observation_io"
            }
        })?
        .ok_or("observation_missing")?;
    let snapshot: ExecutionObservation =
        serde_json::from_slice(&data).map_err(|_| "observation_malformed")?;
    snapshot.validate().map_err(|_| "observation_malformed")?;
    if !snapshot.matches(record) {
        return Err("owner_mismatch");
    }
    if archived {
        return Ok(snapshot.qualifies(ExecutionValidity::Stale, "archived_record"));
    }
    if record.status == RunStatus::Cancelled {
        return Ok(snapshot.qualifies(ExecutionValidity::Stale, "run_cancelled"));
    }
    if let Some(data) =
        disk::read(dir, CANCELLATION_FILE, 4096, bytes).map_err(|_| "observation_io")?
    {
        let barrier: Cancellation =
            serde_json::from_slice(&data).map_err(|_| "observation_malformed")?;
        if barrier.run_id != record.id
            || barrier.workspace_id != record.workspace_id
            || barrier.owner_started_at != record.started_at
        {
            return Err("owner_mismatch");
        }
        return Ok(snapshot.qualifies(ExecutionValidity::Stale, "run_cancelled"));
    }
    let generation = snapshot
        .execution_generation
        .as_deref()
        .ok_or("observation_malformed")?;
    if disk::exists(dir, &revoked_file(generation)).map_err(|_| "observation_io")? {
        return Ok(snapshot.qualifies(ExecutionValidity::Stale, "generation_revoked"));
    }
    if snapshot.validity != ExecutionValidity::Current {
        return Ok(snapshot);
    }
    if !disk::is_locked(dir, &generation_lock(generation)).map_err(|_| "observation_io")? {
        return Ok(snapshot.qualifies(ExecutionValidity::Stale, "owner_not_live"));
    }
    // Revocation on an abnormal drop does not wait for our mutation lock.
    if disk::exists(dir, &revoked_file(generation)).map_err(|_| "observation_io")? {
        return Ok(snapshot.qualifies(ExecutionValidity::Stale, "generation_revoked"));
    }
    Ok(snapshot)
}
