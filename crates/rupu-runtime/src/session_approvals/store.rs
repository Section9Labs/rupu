//! Private durable per-call approval records. All mutation methods require a
//! short exclusive lease; no lease is held while an operator or tool is running.
#![cfg(unix)]

use fs2::FileExt;
use rustix::fs::{AtFlags, Mode, OFlags};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

pub const MAX_ACTIVE_RECORDS: usize = 4096;
pub const MAX_ACTIVE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RECORD_BYTES: u64 = 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_REVIEW_BYTES: usize = 256 * 1024;
pub const MAX_PENDING_PER_SESSION: usize = 16;
pub const MAX_PENDING: usize = 256;
pub const MAX_LIFETIME_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Write,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteIdentity {
    pub server_id: String,
    pub tool: String,
    pub frozen_fingerprint: String,
    pub credential_binding_generation: Option<String>,
}

/// Created only by the broker from a verified runtime registration. The store
/// proves bytes/state/durability, not live process, grant or target authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub approval_id: String,
    pub cp_instance_id: String,
    pub broker_epoch: String,
    pub session_id: String,
    pub root_turn_run_id: String,
    pub run_id: String,
    pub turn_index: u64,
    pub tool_call_id: String,
    pub call_sequence: u64,
    pub workspace_id: String,
    pub workspace_path: String,
    pub worker_registration_id: String,
    pub worker_generation: u64,
    pub parent_run_id: Option<String>,
    pub root_run_id: String,
    pub lineage_fingerprint: String,
    pub tool: String,
    pub called_as: String,
    pub effect: Effect,
    pub descriptor_fingerprint: String,
    pub grant_fingerprint: String,
    pub settings_revision: u64,
    pub orchestration_revision: u64,
    pub approval_authority_revision: u64,
    #[serde(default)]
    pub host_mcp_registry_fingerprint: Option<String>,
    pub remote: Option<RemoteIdentity>,
    pub input: Value,
    pub review: Value,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Allowed,
    Denied,
    Expired,
    Revoked,
    Cancelled,
    DispatchReserved,
    Completed,
    Unknown,
    NotSubmitted,
}
impl State {
    pub fn is_known_terminal(self) -> bool {
        matches!(
            self,
            Self::Denied
                | Self::Expired
                | Self::Revoked
                | Self::Cancelled
                | Self::Completed
                | Self::NotSubmitted
        )
    }
    pub fn is_waiting(self) -> bool {
        matches!(self, Self::Pending | Self::Allowed)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    Deny,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub decision_id: String,
    pub expected_revision: u64,
    pub intent_fingerprint: String,
    pub decision: Decision,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionReceipt {
    pub request: DecisionRequest,
    pub at_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeRequest {
    pub operation_id: String,
    pub expected_revision: u64,
    pub intent_fingerprint: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeReceipt {
    pub request: RevokeRequest,
    pub at_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub success: bool,
    /// A bounded operator-safe explanation, never raw stderr/tool output.
    pub error: Option<String>,
    pub result_fingerprint: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub schema_version: u32,
    pub intent: Intent,
    pub input_fingerprint: String,
    pub review_fingerprint: String,
    pub intent_fingerprint: String,
    pub revision: u64,
    pub state: State,
    pub updated_at_ms: u64,
    pub decision: Option<DecisionReceipt>,
    pub revoke: Option<RevokeReceipt>,
    pub dispatch_id: Option<String>,
    pub dispatched_at_ms: Option<u64>,
    pub completion: Option<Completion>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug)]
pub struct PreDispatch {
    pub intent_fingerprint: String,
    pub worker_registration_id: String,
    pub worker_generation: u64,
    pub broker_epoch: String,
    pub approval_authority_revision: u64,
}
/// Not Clone; issued only after the consumed reservation has reached disk.
#[derive(Debug)]
pub struct DispatchTicket {
    record: Record,
}
impl DispatchTicket {
    pub fn record(&self) -> &Record {
        &self.record
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("approval storage unavailable")]
    Io(#[from] io::Error),
    #[error("invalid approval data")]
    Invalid,
    #[error("approval state changed")]
    Conflict,
    #[error("approval not found for this session")]
    NotFound,
    #[error("approval expired before dispatch")]
    Expired,
    #[error("approval capacity reached; unresolved evidence was preserved")]
    Capacity,
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<rustix::io::Errno> for Error {
    fn from(value: rustix::io::Errno) -> Self {
        Self::Io(value.into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub records: Vec<Record>,
    pub next_cursor: Option<String>,
}

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);
const STORE_DIRECTORY: &str = "session-tool-approvals";

pub struct Store {
    path: PathBuf,
    pub(super) root: File,
    pub(super) active: File,
    calls: File,
    identities: File,
    pub(super) archive: super::archive::Archive,
    _lease: File,
    cursor_key: [u8; 32],
    active_bytes: std::cell::Cell<Option<u64>>,
    active_count: std::cell::Cell<Option<usize>>,
    #[cfg(test)]
    active_scans: std::cell::Cell<usize>,
    #[cfg(test)]
    record_limit_override: std::cell::Cell<usize>,
    #[cfg(test)]
    fail_publication_stage: std::cell::Cell<Option<PublishStage>>,
    // A flock excludes other Store allocations, not shared concurrent calls.
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl Drop for Store {
    fn drop(&mut self) {
        // fork may briefly retain a duplicate until CLOEXEC runs. Closing only
        // our descriptor would leave that shared flock held in the meantime.
        // Explicit unlock ends this owner's lease before the broker gate opens.
        let _ = FileExt::unlock(&self._lease);
    }
}

impl Store {
    /// `global` already exists. The lease is nonblocking and must be dropped
    /// before waiting for a user, IPC or tool. It is not an execution lease.
    pub fn open(global: &Path) -> Result<Self> {
        let global = global.canonicalize()?;
        let parent = open_directory(&global)?;
        let root = private_directory(&parent, STORE_DIRECTORY)?;
        let lease = File::from(rustix::fs::openat(
            &root,
            "lease",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?);
        private_file(&lease)?;
        if lease.metadata()?.len() != 0 {
            return Err(Error::Invalid);
        }
        FileExt::try_lock_exclusive(&lease)?;
        same_entry(&root, "lease", &lease)?;
        lease.sync_all()?;
        root.sync_all()?;
        let active = private_directory(&root, "active")?;
        cleanup_active_temporaries(&active)?;
        let calls = private_directory(&root, "calls")?;
        let identities = private_directory(&root, "identities")?;
        let archive = super::archive::Archive::open(&root)?;
        let cursor_key = match read_optional::<[u8; 32]>(&root, "cursor-key.json", 1024)? {
            Some(key) => key,
            None => {
                let mut key = [0; 32];
                key[..16].copy_from_slice(&ulid::Ulid::new().to_bytes());
                key[16..].copy_from_slice(&ulid::Ulid::new().to_bytes());
                publish(&root, "cursor-key.json", &key, false, 1024)?;
                key
            }
        };
        let store = Self {
            path: global.join(STORE_DIRECTORY),
            root,
            active,
            calls,
            identities,
            archive,
            _lease: lease,
            cursor_key,
            active_bytes: std::cell::Cell::new(None),
            active_count: std::cell::Cell::new(None),
            #[cfg(test)]
            active_scans: std::cell::Cell::new(0),
            #[cfg(test)]
            record_limit_override: std::cell::Cell::new(MAX_ACTIVE_RECORDS),
            #[cfg(test)]
            fail_publication_stage: std::cell::Cell::new(None),
            _not_sync: std::marker::PhantomData,
        };
        store.check_location()?;
        Ok(store)
    }

    pub fn create(&mut self, intent: Intent, now_ms: u64) -> Result<Record> {
        self.check_location()?;
        validate_intent(&intent)?;
        if intent.created_at_ms > now_ms || now_ms >= intent.expires_at_ms {
            return Err(Error::Expired);
        }
        if self
            .optional(&intent.session_id, &intent.approval_id)?
            .is_some()
        {
            return Err(Error::Conflict);
        }
        // An ID used by a different session is also permanently consumed.
        if self.read_active(&intent.approval_id)?.is_some()
            || self.archive.lookup_any(&intent.approval_id)?.is_some()
        {
            return Err(Error::Conflict);
        }
        let mut rows = self.active_records()?;
        // Socket-close cleanup is best effort. Expire only waiting records
        // before admission so a lost worker cannot consume pending capacity
        // forever. The validated snapshot seeds byte/count accounting, making
        // these transitions linear rather than rescanning for every update.
        for row in &mut rows {
            if row.state.is_waiting() {
                self.expire_if_needed(row, now_ms)?;
            }
        }
        // Retain a compact recent window and amortize immutable archive pages.
        // Protected records alone never cause pointless archival scans.
        if self.active_count.get() != Some(rows.len())
            || rows.len() >= 64 && rows.iter().any(|r| r.state.is_known_terminal())
        {
            self.archive_terminal()?;
            rows = self.active_records()?;
        }
        if rows.len() >= self.record_limit()
            || rows.iter().filter(|r| r.state.is_waiting()).count() >= MAX_PENDING
            || rows
                .iter()
                .filter(|r| r.state.is_waiting() && r.intent.session_id == intent.session_id)
                .count()
                >= MAX_PENDING_PER_SESSION
        {
            return Err(Error::Capacity);
        }
        let record = Record {
            schema_version: 1,
            input_fingerprint: fingerprint(&intent.input)?,
            review_fingerprint: fingerprint(&intent.review)?,
            intent_fingerprint: fingerprint(&intent)?,
            updated_at_ms: now_ms,
            intent,
            revision: 0,
            state: State::Pending,
            decision: None,
            revoke: None,
            dispatch_id: None,
            dispatched_at_ms: None,
            completion: None,
            reason: None,
        };
        let claim = CallClaim::for_record(&record)?;
        if read_optional::<CallClaim>(&self.calls, &record_name(&claim.call_fingerprint), 4096)?
            .is_some()
            || read_optional::<CallClaim>(
                &self.identities,
                &record_name(&record.intent.approval_id),
                4096,
            )?
            .is_some()
        {
            return Err(Error::Conflict);
        }
        // Both immutable identities reach disk before an actionable pending
        // record. An orphaned claim is a refusal, never permission to retry.
        publish(
            &self.identities,
            &record_name(&record.intent.approval_id),
            &claim,
            false,
            4096,
        )?;
        publish(
            &self.calls,
            &record_name(&claim.call_fingerprint),
            &claim,
            false,
            4096,
        )?;
        self.write_record(&record, false)?;
        Ok(record)
    }

    pub fn get(&self, session_id: &str, approval_id: &str) -> Result<Record> {
        self.check_location()?;
        self.optional(session_id, approval_id)?
            .ok_or(Error::NotFound)
    }

    pub fn refresh(&mut self, session: &str, id: &str, now_ms: u64) -> Result<Record> {
        let mut row = self.get(session, id)?;
        if row.state.is_waiting() {
            self.expire_if_needed(&mut row, now_ms)?;
        }
        Ok(row)
    }

    pub fn cancel(&mut self, session: &str, id: &str, reason: &str, now_ms: u64) -> Result<Record> {
        if !valid_text(reason, 1024) {
            return Err(Error::Invalid);
        }
        let mut row = self.get(session, id)?;
        if row.state.is_waiting() {
            row.state = State::Cancelled;
        } else if row.state == State::DispatchReserved {
            row.state = State::Unknown;
        } else {
            return Ok(row);
        }
        row.reason = Some(reason.into());
        self.update(row, now_ms)
    }

    pub fn decide(
        &mut self,
        session: &str,
        id: &str,
        request: DecisionRequest,
        now_ms: u64,
    ) -> Result<Record> {
        valid_id(&request.decision_id)?;
        let mut record = self.get(session, id)?;
        if let Some(old) = &record.decision {
            return if old.request == request {
                Ok(record)
            } else {
                Err(Error::Conflict)
            };
        }
        check_revision(
            &record,
            request.expected_revision,
            &request.intent_fingerprint,
        )?;
        if record.state != State::Pending {
            return Err(Error::Conflict);
        }
        if self.expire_if_needed(&mut record, now_ms)? {
            return Err(Error::Expired);
        }
        record.state = match request.decision {
            Decision::AllowOnce => State::Allowed,
            Decision::Deny => State::Denied,
        };
        record.decision = Some(DecisionReceipt {
            request,
            at_ms: now_ms,
        });
        self.update(record, now_ms)
    }

    pub fn revoke(
        &mut self,
        session: &str,
        id: &str,
        request: RevokeRequest,
        now_ms: u64,
    ) -> Result<Record> {
        valid_id(&request.operation_id)?;
        let mut record = self.get(session, id)?;
        if let Some(old) = &record.revoke {
            return if old.request == request {
                Ok(record)
            } else {
                Err(Error::Conflict)
            };
        }
        check_revision(
            &record,
            request.expected_revision,
            &request.intent_fingerprint,
        )?;
        if !record.state.is_waiting() {
            return Err(Error::Conflict);
        }
        if self.expire_if_needed(&mut record, now_ms)? {
            return Err(Error::Expired);
        }
        record.state = State::Revoked;
        record.revoke = Some(RevokeReceipt {
            request,
            at_ms: now_ms,
        });
        self.update(record, now_ms)
    }

    /// Broker must perform live ownership/grant/target checks immediately
    /// before this call under its admission lock. This verifies frozen identity
    /// and atomically consumes the allow; it cannot verify external state.
    pub fn reserve(
        &mut self,
        session: &str,
        id: &str,
        expected: &PreDispatch,
        now_ms: u64,
    ) -> Result<DispatchTicket> {
        let mut record = self.get(session, id)?;
        if record.state != State::Allowed
            || record.intent_fingerprint != expected.intent_fingerprint
            || record.intent.worker_registration_id != expected.worker_registration_id
            || record.intent.worker_generation != expected.worker_generation
            || record.intent.broker_epoch != expected.broker_epoch
            || record.intent.approval_authority_revision != expected.approval_authority_revision
        {
            return Err(Error::Conflict);
        }
        if self.expire_if_needed(&mut record, now_ms)? {
            return Err(Error::Expired);
        }
        record.state = State::DispatchReserved;
        record.dispatch_id = Some(ulid::Ulid::new().to_string());
        record.dispatched_at_ms = Some(now_ms);
        let record = self.update(record, now_ms)?;
        Ok(DispatchTicket { record })
    }

    pub fn complete(
        &mut self,
        ticket: DispatchTicket,
        completion: Completion,
        now_ms: u64,
    ) -> Result<Record> {
        if completion
            .error
            .as_ref()
            .is_some_and(|v| !valid_text(v, 1024))
            || completion
                .result_fingerprint
                .as_ref()
                .is_some_and(|v| !valid_fingerprint(v))
        {
            return Err(Error::Invalid);
        }
        let mut record = self.get(
            &ticket.record.intent.session_id,
            &ticket.record.intent.approval_id,
        )?;
        if record.state != State::DispatchReserved
            || record.revision != ticket.record.revision
            || record.dispatch_id != ticket.record.dispatch_id
            || record.intent_fingerprint != ticket.record.intent_fingerprint
        {
            return Err(Error::Conflict);
        }
        record.state = State::Completed;
        record.completion = Some(completion);
        self.update(record, now_ms)
    }

    /// Never clears uncertainty, even if a later caller claims no effect.
    pub fn mark_unknown(
        &mut self,
        session: &str,
        id: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<Record> {
        if !valid_text(reason, 1024) {
            return Err(Error::Invalid);
        }
        let mut record = self.get(session, id)?;
        if record.state == State::Unknown {
            return Ok(record);
        }
        if record.state != State::DispatchReserved {
            return Err(Error::Conflict);
        }
        record.state = State::Unknown;
        record.reason = Some(reason.into());
        self.update(record, now_ms)
    }

    /// Cancellation before reservation is known no-dispatch. A reserved call
    /// is unknown; this method does not claim stopping rolled its effect back.
    pub fn cancel_worker(
        &mut self,
        registration: &str,
        generation: u64,
        reason: &str,
        now_ms: u64,
    ) -> Result<usize> {
        if !valid_text(reason, 1024) {
            return Err(Error::Invalid);
        }
        let mut count = 0;
        for mut row in self.active_records()? {
            if row.intent.worker_registration_id != registration
                || row.intent.worker_generation != generation
            {
                continue;
            }
            if row.state.is_waiting() {
                row.state = State::Cancelled;
            } else if row.state == State::DispatchReserved {
                row.state = State::Unknown;
            } else {
                continue;
            }
            row.reason = Some(reason.into());
            self.update(row, now_ms)?;
            count += 1;
        }
        Ok(count)
    }

    /// A fresh broker never resumes an old permit. Unknown remains pinned.
    pub fn recover_epoch(&mut self, epoch: &str, now_ms: u64) -> Result<usize> {
        valid_id(epoch)?;
        let mut count = 0;
        for mut row in self.active_records()? {
            if row.intent.broker_epoch == epoch {
                continue;
            }
            if row.state.is_waiting() {
                row.state = State::Revoked;
            } else if row.state == State::DispatchReserved {
                row.state = State::Unknown;
            } else {
                continue;
            }
            row.reason = Some("broker_epoch_changed".into());
            self.update(row, now_ms)?;
            count += 1;
        }
        Ok(count)
    }

    pub fn list(&self, session: &str, cursor: Option<&str>, limit: usize) -> Result<Page> {
        self.check_location()?;
        valid_id(session)?;
        if !(1..=50).contains(&limit) {
            return Err(Error::Invalid);
        }
        let mut active: Vec<_> = self
            .active_records()?
            .into_iter()
            .filter(|r| r.intent.session_id == session)
            .collect();
        active.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then(a.intent.approval_id.cmp(&b.intent.approval_id))
        });
        let snapshot = fingerprint(
            &active
                .iter()
                .map(|r| (&r.intent.approval_id, r.revision))
                .collect::<Vec<_>>(),
        )?;
        let mut state = match cursor {
            Some(value) => super::archive::Cursor::decode(value, &self.cursor_key)?,
            None => super::archive::Cursor {
                session_id: session.into(),
                snapshot: snapshot.clone(),
                active_offset: 0,
                archive_page: self.archive.head(session)?,
                archive_offset: 0,
                limit,
            },
        };
        if state.session_id != session
            || state.snapshot != snapshot
            || state.limit != limit
            || state.active_offset > active.len()
        {
            return Err(Error::Conflict);
        }
        let end = (state.active_offset + limit).min(active.len());
        let mut records = active[state.active_offset..end].to_vec();
        state.active_offset = end;
        if records.len() < limit {
            if let Some(hash) = state.archive_page.clone() {
                let page = self.archive.page(&hash, session)?;
                if state.archive_offset > page.records.len() {
                    return Err(Error::Invalid);
                }
                let end = (state.archive_offset + limit - records.len()).min(page.records.len());
                records.extend_from_slice(&page.records[state.archive_offset..end]);
                state.archive_offset = end;
                if end == page.records.len() {
                    state.archive_page = page.previous;
                    state.archive_offset = 0;
                }
            }
        }
        let next_cursor = if state.active_offset < active.len() || state.archive_page.is_some() {
            Some(state.encode(&self.cursor_key)?)
        } else {
            None
        };
        Ok(Page {
            records,
            next_cursor,
        })
    }

    /// Known outcomes move to immutable pages and per-ID tombstones before
    /// active files are removed. Unknown/reserved/waiting records never rotate.
    pub fn archive_terminal(&mut self) -> Result<usize> {
        self.check_location()?;
        let records = self.active_records_raw()?;
        let mut remaining = Vec::new();
        let mut moved = 0;
        for row in records {
            if let Some(archived) = self.archive.committed_lookup(&row.intent.approval_id)? {
                if archived != row {
                    return Err(Error::Invalid);
                }
                self.remove_active(&row)?;
                moved += 1;
            } else {
                remaining.push(row);
            }
        }
        if let Some(committed) = self.archive.resume_pending(&remaining)? {
            let ids: std::collections::BTreeSet<_> = committed
                .iter()
                .map(|r| r.intent.approval_id.clone())
                .collect();
            for row in committed {
                self.remove_active(&row)?;
                moved += 1;
            }
            remaining.retain(|r| !ids.contains(&r.intent.approval_id));
        }
        let mut groups: std::collections::BTreeMap<String, Vec<Record>> =
            std::collections::BTreeMap::new();
        for row in remaining {
            if row.state.is_known_terminal() {
                groups
                    .entry(row.intent.session_id.clone())
                    .or_default()
                    .push(row);
            }
        }
        for rows in groups.values() {
            for batch in rows.chunks(super::archive::MAX_PAGE_RECORDS) {
                for row in self.archive.append(batch)? {
                    self.remove_active(&row)?;
                    moved += 1;
                }
            }
        }
        Ok(moved)
    }

    fn optional(&self, session: &str, id: &str) -> Result<Option<Record>> {
        valid_id(session)?;
        valid_id(id)?;
        let row = match self.read_active(id)? {
            Some(row) => Some(row),
            None => self.archive.lookup_any(id)?,
        };
        if let Some(row) = &row {
            self.validate_claim(row)?;
        }
        Ok(row.filter(|r| r.intent.session_id == session))
    }
    fn read_active(&self, id: &str) -> Result<Option<Record>> {
        valid_id(id)?;
        let row: Option<Record> = read_optional(&self.active, &record_name(id), MAX_RECORD_BYTES)?;
        if let Some(row) = &row {
            validate_record(row)?;
            self.validate_claim(row)?;
            if row.intent.approval_id != id {
                return Err(Error::Invalid);
            }
        }
        Ok(row)
    }
    fn active_records_raw(&self) -> Result<Vec<Record>> {
        self.check_location()?;
        #[cfg(test)]
        self.active_scans.set(self.active_scans.get() + 1);
        let mut result = Vec::new();
        let mut bytes = 0u64;
        for name in names(&self.active, MAX_ACTIVE_RECORDS + 32)? {
            if name.starts_with("pending-") {
                continue;
            }
            let (row, size): (Record, u64) =
                read_optional_measured(&self.active, &name, MAX_RECORD_BYTES)?
                    .ok_or(Error::Invalid)?;
            validate_record(&row)?;
            self.validate_claim(&row)?;
            if record_name(&row.intent.approval_id) != name {
                return Err(Error::Invalid);
            }
            bytes = bytes.checked_add(size).ok_or(Error::Capacity)?;
            if bytes > MAX_ACTIVE_BYTES || result.len() >= self.record_limit() {
                return Err(Error::Capacity);
            }
            result.push(row);
        }
        self.active_bytes.set(Some(bytes));
        self.active_count.set(Some(result.len()));
        Ok(result)
    }
    fn active_records(&self) -> Result<Vec<Record>> {
        let mut result = Vec::new();
        for row in self.active_records_raw()? {
            if let Some(archived) = self.archive.committed_lookup(&row.intent.approval_id)? {
                if archived != row {
                    return Err(Error::Invalid);
                }
            } else {
                result.push(row);
            }
        }
        Ok(result)
    }
    fn remove_active(&self, expected: &Record) -> Result<()> {
        let actual = self
            .read_active(&expected.intent.approval_id)?
            .ok_or(Error::Invalid)?;
        if actual != *expected || !actual.state.is_known_terminal() {
            return Err(Error::Invalid);
        }
        let size = read_optional_measured::<Record>(
            &self.active,
            &record_name(&expected.intent.approval_id),
            MAX_RECORD_BYTES,
        )?
        .ok_or(Error::Invalid)?
        .1;
        let before = self.active_bytes.get();
        let before_count = self.active_count.get();
        self.active_bytes.set(None);
        self.active_count.set(None);
        rustix::fs::unlinkat(
            &self.active,
            record_name(&expected.intent.approval_id).as_str(),
            AtFlags::empty(),
        )?;
        self.active.sync_all()?;
        if let Some(bytes) = before {
            self.active_bytes
                .set(Some(bytes.checked_sub(size).ok_or(Error::Invalid)?));
        }
        if let Some(count) = before_count {
            self.active_count
                .set(Some(count.checked_sub(1).ok_or(Error::Invalid)?));
        }
        Ok(())
    }
    fn expire_if_needed(&mut self, row: &mut Record, now_ms: u64) -> Result<bool> {
        if now_ms < row.intent.created_at_ms || now_ms >= row.intent.expires_at_ms {
            row.state = State::Expired;
            row.reason = Some("approval_expired".into());
            *row = self.update(row.clone(), now_ms)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn update(&mut self, mut row: Record, now_ms: u64) -> Result<Record> {
        row.revision = row.revision.checked_add(1).ok_or(Error::Invalid)?;
        row.updated_at_ms = now_ms;
        self.write_record(&row, true)?;
        Ok(row)
    }
    fn write_record(&self, row: &Record, replace: bool) -> Result<()> {
        self.check_location()?;
        validate_record(row)?;
        self.validate_claim(row)?;
        if self.active_bytes.get().is_none() || self.active_count.get().is_none() {
            self.active_records_raw()?;
        }
        let old: Option<(Record, u64)> = read_optional_measured(
            &self.active,
            &record_name(&row.intent.approval_id),
            MAX_RECORD_BYTES,
        )?;
        let old_size = match old {
            Some((old, size)) if replace => {
                validate_record(&old)?;
                self.validate_claim(&old)?;
                if old.intent != row.intent || old.revision.checked_add(1) != Some(row.revision) {
                    return Err(Error::Conflict);
                }
                size
            }
            None if !replace => 0,
            _ => return Err(Error::Conflict),
        };
        let bytes = self
            .active_bytes
            .get()
            .ok_or(Error::Invalid)?
            .checked_sub(old_size)
            .and_then(|v| v.checked_add(encode(row, MAX_RECORD_BYTES).ok()?.len() as u64))
            .ok_or(Error::Capacity)?;
        if bytes > MAX_ACTIVE_BYTES {
            return Err(Error::Capacity);
        }
        let count = self
            .active_count
            .get()
            .ok_or(Error::Invalid)?
            .checked_add(usize::from(!replace))
            .ok_or(Error::Capacity)?;
        if count > self.record_limit() {
            return Err(Error::Capacity);
        }
        self.active_bytes.set(None);
        self.active_count.set(None);
        #[cfg(not(test))]
        publish(
            &self.active,
            &record_name(&row.intent.approval_id),
            row,
            replace,
            MAX_RECORD_BYTES,
        )?;
        #[cfg(test)]
        publish_hook(
            &self.active,
            &record_name(&row.intent.approval_id),
            &encode(row, MAX_RECORD_BYTES)?,
            replace,
            &mut |stage| {
                if self.fail_publication_stage.get() == Some(stage) {
                    Err(io::Error::other("fixture publication failure"))
                } else {
                    Ok(())
                }
            },
        )?;
        self.active_bytes.set(Some(bytes));
        self.active_count.set(Some(count));
        Ok(())
    }
    fn record_limit(&self) -> usize {
        #[cfg(test)]
        {
            self.record_limit_override.get()
        }
        #[cfg(not(test))]
        {
            MAX_ACTIVE_RECORDS
        }
    }
    fn check_location(&self) -> Result<()> {
        let current = open_directory(&self.path)?;
        same_file(&current, &self.root)?;
        same_entry(&self.root, "active", &self.active)?;
        same_entry(&self.root, "calls", &self.calls)?;
        same_entry(&self.root, "identities", &self.identities)?;
        self.archive.check_location(&self.root)?;
        Ok(())
    }
    fn validate_claim(&self, row: &Record) -> Result<()> {
        let expected = CallClaim::for_record(row)?;
        let call: CallClaim =
            read_optional(&self.calls, &record_name(&expected.call_fingerprint), 4096)?
                .ok_or(Error::Invalid)?;
        let id: CallClaim = read_optional(
            &self.identities,
            &record_name(&row.intent.approval_id),
            4096,
        )?
        .ok_or(Error::Invalid)?;
        if call != expected || id != expected {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallClaim {
    schema_version: u32,
    approval_id: String,
    session_id: String,
    call_fingerprint: String,
    intent_fingerprint: String,
}
impl CallClaim {
    fn for_record(row: &Record) -> Result<Self> {
        let i = &row.intent;
        // Deliberately omit CP epoch/instance, worker generation, tool/input:
        // replacing any of those cannot mint a second attempt for this call.
        let call_fingerprint = fingerprint(&(
            "session-approval-call-v1",
            &i.session_id,
            &i.root_turn_run_id,
            &i.run_id,
            i.turn_index,
            &i.tool_call_id,
            i.call_sequence,
        ))?;
        Ok(Self {
            schema_version: 1,
            approval_id: i.approval_id.clone(),
            session_id: i.session_id.clone(),
            call_fingerprint,
            intent_fingerprint: row.intent_fingerprint.clone(),
        })
    }
}

fn check_revision(record: &Record, revision: u64, fingerprint: &str) -> Result<()> {
    if record.revision != revision || record.intent_fingerprint != fingerprint {
        Err(Error::Conflict)
    } else {
        Ok(())
    }
}
pub(super) fn validate_record(record: &Record) -> Result<()> {
    validate_intent(&record.intent)?;
    if record.schema_version != 1
        || record.input_fingerprint != fingerprint(&record.intent.input)?
        || record.review_fingerprint != fingerprint(&record.intent.review)?
        || record.intent_fingerprint != fingerprint(&record.intent)?
    {
        return Err(Error::Invalid);
    }
    let dispatched = matches!(
        record.state,
        State::DispatchReserved | State::Completed | State::Unknown
    );
    if dispatched != record.dispatch_id.is_some()
        || dispatched != record.dispatched_at_ms.is_some()
        || (record.state == State::Completed) != record.completion.is_some()
    {
        return Err(Error::Invalid);
    }
    if (record.state == State::Allowed || dispatched)
        && record.decision.as_ref().map(|d| d.request.decision) != Some(Decision::AllowOnce)
    {
        return Err(Error::Invalid);
    }
    if record.state == State::Pending && record.decision.is_some()
        || record.state == State::Denied
            && record.decision.as_ref().map(|d| d.request.decision) != Some(Decision::Deny)
    {
        return Err(Error::Invalid);
    }
    if let Some(decision) = &record.decision {
        valid_id(&decision.request.decision_id)?;
        if decision.request.intent_fingerprint != record.intent_fingerprint
            || decision.request.expected_revision >= record.revision
        {
            return Err(Error::Invalid);
        }
    }
    if let Some(revoke) = &record.revoke {
        valid_id(&revoke.request.operation_id)?;
        if record.state != State::Revoked
            || revoke.request.intent_fingerprint != record.intent_fingerprint
            || revoke.request.expected_revision >= record.revision
        {
            return Err(Error::Invalid);
        }
    }
    if record.reason.as_ref().is_some_and(|v| !valid_text(v, 1024)) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn validate_intent(value: &Intent) -> Result<()> {
    for id in [
        &value.approval_id,
        &value.cp_instance_id,
        &value.broker_epoch,
        &value.session_id,
        &value.root_turn_run_id,
        &value.run_id,
        &value.workspace_id,
        &value.worker_registration_id,
        &value.root_run_id,
    ] {
        valid_id(id)?;
    }
    if let Some(id) = &value.parent_run_id {
        valid_id(id)?;
    }
    if !valid_text(&value.tool_call_id, 512)
        || !valid_text(&value.tool, 256)
        || !valid_text(&value.called_as, 256)
        || !valid_text(&value.workspace_path, 4096)
        || !Path::new(&value.workspace_path).is_absolute()
        || value.expires_at_ms <= value.created_at_ms
        || value.expires_at_ms - value.created_at_ms > MAX_LIFETIME_MS
        || !valid_fingerprint(&value.lineage_fingerprint)
        || !valid_fingerprint(&value.descriptor_fingerprint)
        || !valid_fingerprint(&value.grant_fingerprint)
        || value
            .host_mcp_registry_fingerprint
            .as_ref()
            .is_some_and(|v| !valid_fingerprint(v))
    {
        return Err(Error::Invalid);
    }
    if let Some(remote) = &value.remote {
        valid_id(&remote.server_id)?;
        if !valid_text(&remote.tool, 256)
            || !valid_fingerprint(&remote.frozen_fingerprint)
            || remote
                .credential_binding_generation
                .as_ref()
                .is_some_and(|v| !valid_text(v, 128))
        {
            return Err(Error::Invalid);
        }
    }
    bounded_value(&value.input, MAX_INPUT_BYTES)?;
    bounded_value(&value.review, MAX_REVIEW_BYTES)?;
    Ok(())
}
pub(super) fn valid_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn bounded_value(value: &Value, max: usize) -> Result<()> {
    let mut pending = vec![(value, 0usize)];
    let mut count = 0usize;
    while let Some((value, depth)) = pending.pop() {
        count += 1;
        if depth > 64 || count > 50_000 {
            return Err(Error::Invalid);
        }
        match value {
            Value::Array(a) => pending.extend(a.iter().map(|v| (v, depth + 1))),
            Value::Object(o) => pending.extend(o.values().map(|v| (v, depth + 1))),
            _ => (),
        }
    }
    encode(value, max as u64)?;
    Ok(())
}
pub(super) fn fingerprint<T: Serialize>(value: &T) -> Result<String> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
            Value::Object(o) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    o.into_iter().map(|(k, v)| (k, canonical(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            v => v,
        }
    }
    let value = canonical(serde_json::to_value(value).map_err(|_| Error::Invalid)?);
    Ok(format!(
        "sha256:{}",
        bytes_hash(&serde_json::to_vec(&value).map_err(|_| Error::Invalid)?)
    ))
}
pub(super) fn bytes_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn record_name(id: &str) -> String {
    format!("{}.json", bytes_hash(id.as_bytes()))
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid private approval store")
}
fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut file = File::from(rustix::fs::open("/", DIRECTORY_FLAGS, Mode::empty())?);
    for part in path.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                file = File::from(rustix::fs::openat(
                    &file,
                    name,
                    DIRECTORY_FLAGS,
                    Mode::empty(),
                )?)
            }
            _ => return Err(invalid()),
        }
    }
    Ok(file)
}
pub(super) fn private_directory(parent: &File, name: &str) -> io::Result<File> {
    match rustix::fs::mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) => parent.sync_all()?,
        Err(rustix::io::Errno::EXIST) => (),
        Err(e) => return Err(e.into()),
    }
    let file = File::from(rustix::fs::openat(
        parent,
        name,
        DIRECTORY_FLAGS,
        Mode::empty(),
    )?);
    private_dir(&file)?;
    Ok(file)
}
fn private_dir(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn private_file(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.uid() != rustix::process::geteuid().as_raw()
    {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn same_file(a: &File, b: &File) -> io::Result<()> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    if a.dev() != b.dev() || a.ino() != b.ino() {
        Err(invalid())
    } else {
        Ok(())
    }
}
pub(super) fn same_entry(parent: &File, name: &str, expected: &File) -> io::Result<()> {
    let actual = File::from(rustix::fs::openat(parent, name, READ_FLAGS, Mode::empty())?);
    if expected.metadata()?.is_dir() {
        private_dir(&actual)?;
    } else {
        private_file(&actual)?;
    }
    same_file(&actual, expected)
}
fn names(dir: &File, max: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        if out.len() >= max {
            return Err(Error::Capacity);
        }
        out.push(String::from_utf8(bytes.to_vec()).map_err(|_| Error::Invalid)?);
    }
    out.sort();
    Ok(out)
}
fn cleanup_active_temporaries(dir: &File) -> Result<()> {
    let mut removed = false;
    for name in names(dir, MAX_ACTIVE_RECORDS + 32)? {
        let Some(suffix) = name.strip_prefix("pending-") else {
            continue;
        };
        if suffix.parse::<ulid::Ulid>().is_err() {
            return Err(Error::Invalid);
        }
        let file = File::from(rustix::fs::openat(
            dir,
            name.as_str(),
            READ_FLAGS,
            Mode::empty(),
        )?);
        private_file(&file)?;
        if file.metadata()?.len() > MAX_RECORD_BYTES {
            return Err(Error::Invalid);
        }
        same_entry(dir, &name, &file)?;
        rustix::fs::unlinkat(dir, name.as_str(), AtFlags::empty())?;
        removed = true;
    }
    if removed {
        dir.sync_all()?;
    }
    Ok(())
}
pub(super) fn encode<T: Serialize>(value: &T, max: u64) -> Result<Vec<u8>> {
    struct Cap {
        bytes: Vec<u8>,
        max: u64,
    }
    impl Write for Cap {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if b.len() as u64 > self.max.saturating_sub(self.bytes.len() as u64) {
                return Err(invalid());
            }
            self.bytes.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut out = Cap {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut out, value).map_err(|_| Error::Invalid)?;
    Ok(out.bytes)
}
pub(super) fn read_optional<T: DeserializeOwned>(
    dir: &File,
    name: &str,
    max: u64,
) -> Result<Option<T>> {
    Ok(read_optional_measured(dir, name, max)?.map(|(value, _)| value))
}
fn read_optional_measured<T: DeserializeOwned>(
    dir: &File,
    name: &str,
    max: u64,
) -> Result<Option<(T, u64)>> {
    let file = match rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(e) => return Err(Error::Io(e.into())),
    };
    private_file(&file)?;
    let before = file.metadata()?;
    if before.len() > max {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::new();
    (&file).take(max + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() as u64 > max
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(Error::Invalid);
    }
    Ok(Some((
        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?,
        bytes.len() as u64,
    )))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublishStage {
    FileSynced,
    Published,
    DirectorySynced,
}
pub(super) fn publish<T: Serialize>(
    dir: &File,
    name: &str,
    value: &T,
    replace: bool,
    max: u64,
) -> Result<()> {
    publish_hook(dir, name, &encode(value, max)?, replace, &mut |_| Ok(()))
}
pub(super) fn publish_hook(
    dir: &File,
    name: &str,
    bytes: &[u8],
    replace: bool,
    hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
) -> Result<()> {
    let previous = if replace {
        let f = File::from(rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty())?);
        private_file(&f)?;
        Some(f)
    } else {
        None
    };
    let temp = format!("pending-{}", ulid::Ulid::new());
    let mut f = File::from(rustix::fs::openat(
        dir,
        temp.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    let result = (|| -> Result<()> {
        private_file(&f)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        hook(PublishStage::FileSynced)?;
        if let Some(old) = previous {
            same_entry(dir, name, &old)?;
            rustix::fs::renameat(dir, temp.as_str(), dir, name)?;
        } else {
            // Atomic no-replace rename keeps the final inode single-link at
            // every crash point. A link+unlink publication has an unsafe
            // nlink=2 interval and is deliberately not a portability fallback.
            #[cfg(any(
                target_vendor = "apple",
                target_os = "linux",
                target_os = "android",
                target_os = "redox"
            ))]
            rustix::fs::renameat_with(
                dir,
                temp.as_str(),
                dir,
                name,
                rustix::fs::RenameFlags::NOREPLACE,
            )?;
            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "linux",
                target_os = "android",
                target_os = "redox"
            )))]
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic approval publication unavailable",
            )));
        }
        hook(PublishStage::Published)?;
        dir.sync_all()?;
        hook(PublishStage::DirectorySynced)?;
        Ok(())
    })();
    let _ = rustix::fs::unlinkat(dir, temp.as_str(), AtFlags::empty());
    result
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
