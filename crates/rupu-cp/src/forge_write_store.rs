//! Private durable intent/reservation journal for operator forge writes.
//!
//! Unix only. A caller holds Store across preflight, dispatch and receipt write.
//! Only a fresh, noncloneable ticket can cross the dispatch boundary. Reopening
//! never resumes a provider write. Limits are an initial bounded implementation,
//! not a permanent product quota: terminal snapshot archival/compaction must
//! preserve operation/intent identity tombstones and all uncertain evidence.
#![cfg(unix)]

use fs2::FileExt;
use rustix::fs::{AtFlags, Mode, OFlags};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_UNUSED_INTENTS: usize = 16;
pub const MAX_OPERATIONS: usize = 4096;
pub const MAX_RECORD_BYTES: u64 = 256 * 1024;
pub const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INTENTS: usize = MAX_OPERATIONS + MAX_UNUSED_INTENTS;
const MAX_LIFETIME: u64 = 15 * 60;
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);
const STORE_NAME: &str = "forge-write-operations";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub schema: u8,
    pub intent_id: String,
    pub digest: String,
    /// Normalized authority/action/input hash, excluding ephemeral epochs.
    pub key: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub payload: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Reserved,
    Dispatching,
    Terminal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    NotSubmitted,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub from_phase: Phase,
    pub from_receipt_version: u64,
    pub observed_updated_at: u64,
    pub recovered_at: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub schema: u8,
    pub operation_id: String,
    pub intent: Intent,
    pub created_at: u64,
    pub updated_at: u64,
    pub receipt_version: u64,
    pub dispatch_marked_at: Option<u64>,
    pub recovery: Option<Recovery>,
    pub phase: Phase,
    pub outcome: Option<Outcome>,
    pub reason: Option<String>,
    pub result: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedOutcome {
    InProgress,
    NotSubmitted,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub operation: Operation,
    pub writer_active: bool,
    /// A status read observes persisted names; it never promises a new fsync.
    pub durability: &'static str,
    pub observed_outcome: ObservedOutcome,
}

/// Constructible only by a durable fresh reservation under this live lease.
#[derive(Debug)]
pub struct ReservationTicket {
    lease: String,
    operation: Operation,
}
/// Constructible only after the dispatch marker's file AND directory fsync.
#[derive(Debug)]
pub struct DispatchTicket {
    lease: String,
    operation: Operation,
}
impl ReservationTicket {
    pub fn operation(&self) -> &Operation {
        &self.operation
    }
}
impl DispatchTicket {
    pub fn operation(&self) -> &Operation {
        &self.operation
    }
}
#[derive(Debug)]
pub enum Reservation {
    New {
        intent: Intent,
        ticket: ReservationTicket,
    },
    Replay(Operation),
}

pub struct Reader {
    path: PathBuf,
    root: File,
    intents: File,
    operations: File,
    lease: File,
}
pub struct Store {
    reader: Reader,
    nonce: String,
    // The flock serializes different Store allocations, not calls sharing one
    // allocation. Single-owner !Sync prevents concurrent check/publish races;
    // Cell<()> remains Send so ownership can cross spawn_blocking/async awaits.
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl Reader {
    /// No creation, cleanup, recovery or provider requests.
    pub fn open(global: &Path) -> io::Result<Self> {
        Self::open_inner(global, false)
    }
    fn open_inner(global: &Path, create: bool) -> io::Result<Self> {
        let global = global.canonicalize()?;
        let owner = open_directory(&global)?;
        let root = if create {
            private_directory(&owner, STORE_NAME)?
        } else {
            open_private_directory(&owner, STORE_NAME)?
        };
        let flags = OFlags::RDWR
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC
            | if create {
                OFlags::CREATE
            } else {
                OFlags::empty()
            };
        let lease = File::from(rustix::fs::openat(
            &root,
            "lease",
            flags,
            Mode::from_bits_truncate(0o600),
        )?);
        private_file(&lease)?;
        if lease.metadata()?.len() != 0 {
            return Err(invalid("invalid forge operation lease"));
        }
        let (intents, operations) = if create {
            // Caller obtains the exclusive lease before altering private children.
            FileExt::try_lock_exclusive(&lease)?;
            same_entry(&root, "lease", &lease)?;
            lease.sync_all()?;
            root.sync_all()?;
            (
                private_directory(&root, "intents")?,
                private_directory(&root, "operations")?,
            )
        } else {
            (
                open_private_directory(&root, "intents")?,
                open_private_directory(&root, "operations")?,
            )
        };
        let reader = Self {
            path: global.join(STORE_NAME),
            root,
            intents,
            operations,
            lease,
        };
        reader.check_location()?;
        Ok(reader)
    }
    pub fn intent(&self, id: &str) -> io::Result<Option<Intent>> {
        self.check_location()?;
        valid_id(id)?;
        let result = self.read_intent(id)?;
        if result.is_some() {
            return Ok(result);
        }
        // Known terminal receipts retain the complete review after redundant
        // standalone intent snapshots have been compacted.
        let mut found = None;
        for operation in self.all_operations()? {
            if operation.intent.intent_id == id {
                if found.is_some() {
                    return Err(invalid("duplicate intent reservation"));
                }
                found = Some(operation.intent);
            }
        }
        Ok(found)
    }
    pub fn operation(&self, id: &str) -> io::Result<Option<Status>> {
        self.check_location()?;
        valid_id(id)?;
        // When no writer holds the lease, hold it briefly across the read so a
        // recovered reserved record cannot be misclassified during dispatch.
        // A separate open file description per observation prevents concurrent
        // Reader calls from unlocking one another's flock.
        let observation_lease = File::from(rustix::fs::openat(
            &self.root,
            "lease",
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        private_file(&observation_lease)?;
        same_file(&observation_lease, &self.lease)?;
        let active = match FileExt::try_lock_exclusive(&observation_lease) {
            Ok(()) => false,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => true,
            Err(e) => return Err(e),
        };
        let result = (|| {
            self.check_location()?;
            let operation = self.read_operation(id)?;
            Ok(operation.map(|operation| {
                let observed_outcome = match (operation.phase, operation.outcome, active) {
                    (_, _, true) => ObservedOutcome::InProgress,
                    (Phase::Terminal, Some(Outcome::NotSubmitted), _) => {
                        ObservedOutcome::NotSubmitted
                    }
                    (Phase::Terminal, Some(Outcome::Accepted), _) => ObservedOutcome::Accepted,
                    (Phase::Terminal, Some(Outcome::Rejected), _) => ObservedOutcome::Rejected,
                    (Phase::Terminal, Some(Outcome::Unknown), _) => ObservedOutcome::Unknown,
                    (Phase::Reserved, _, false) => ObservedOutcome::NotSubmitted,
                    _ => ObservedOutcome::Unknown,
                };
                Status {
                    operation,
                    writer_active: active,
                    durability: if active { "unconfirmed" } else { "observed" },
                    observed_outcome,
                }
            }))
        })();
        if !active {
            FileExt::unlock(&observation_lease)?;
        }
        result
    }
    fn read_intent(&self, id: &str) -> io::Result<Option<Intent>> {
        let value: Option<Intent> = read_optional(&self.intents, &file_name(id))?;
        if let Some(value) = &value {
            validate_intent(value)?;
            if value.intent_id != id {
                return Err(invalid("foreign intent record"));
            }
        }
        Ok(value)
    }
    fn read_operation(&self, id: &str) -> io::Result<Option<Operation>> {
        let value: Option<Operation> = read_optional(&self.operations, &file_name(id))?;
        if let Some(value) = &value {
            validate_operation(value)?;
            if value.operation_id != id {
                return Err(invalid("foreign operation record"));
            }
        }
        Ok(value)
    }
    fn all_operations(&self) -> io::Result<Vec<Operation>> {
        self.byte_total()?;
        let mut rows = Vec::new();
        let mut intents = HashSet::new();
        for name in names(&self.operations, MAX_OPERATIONS)? {
            let id = record_id(&name)?;
            let row = self
                .read_operation(id)?
                .ok_or_else(|| invalid("operation disappeared"))?;
            if !intents.insert(row.intent.intent_id.clone()) {
                return Err(invalid("intent has multiple operations"));
            }
            rows.push(row);
        }
        Ok(rows)
    }
    fn byte_total(&self) -> io::Result<u64> {
        let mut bytes = 0u64;
        for (dir, limit) in [
            (&self.intents, MAX_INTENTS),
            (&self.operations, MAX_OPERATIONS),
        ] {
            for name in names(dir, limit)? {
                record_id(&name)?;
                let file = File::from(rustix::fs::openat(
                    dir,
                    name.as_str(),
                    READ_FLAGS,
                    Mode::empty(),
                )?);
                private_file(&file)?;
                let len = file.metadata()?.len();
                if len > MAX_RECORD_BYTES {
                    return Err(capacity("forge record bound exceeded"));
                }
                bytes = bytes
                    .checked_add(len)
                    .ok_or_else(|| capacity("forge store overflow"))?;
                if bytes > MAX_STORE_BYTES {
                    return Err(capacity("forge store requires terminal archival"));
                }
            }
        }
        Ok(bytes)
    }
    fn check_location(&self) -> io::Result<()> {
        let current = open_directory(&self.path)?;
        private_dir_metadata(&current)?;
        same_file(&current, &self.root)?;
        same_entry(&self.root, "intents", &self.intents)?;
        same_entry(&self.root, "operations", &self.operations)?;
        same_entry(&self.root, "lease", &self.lease)
    }
}

impl Store {
    /// Acquires the cross-process lease. Recovery only records observations; it
    /// never dispatches a provider request or creates an executable ticket.
    pub fn open(global: &Path) -> io::Result<Self> {
        let store = Self {
            reader: Reader::open_inner(global, true)?,
            nonce: ulid::Ulid::new().to_string(),
            _not_sync: std::marker::PhantomData,
        };
        store.cleanup_pending()?;
        store.recover()?;
        Ok(store)
    }
    pub fn intent(&self, id: &str) -> io::Result<Option<Intent>> {
        self.reader.intent(id)
    }
    pub fn operation(&self, id: &str) -> io::Result<Option<Operation>> {
        self.reader.check_location()?;
        valid_id(id)?;
        self.reader.read_operation(id)
    }
    pub fn publish_intent(
        &self,
        id: &str,
        digest: &str,
        key: &str,
        expires_at: u64,
        payload: Value,
    ) -> io::Result<Intent> {
        self.reader.check_location()?;
        valid_id(id)?;
        valid_digest(digest)?;
        valid_digest(key)?;
        let created_at = now()?;
        let intent = Intent {
            schema: 1,
            intent_id: id.into(),
            digest: digest.into(),
            key: key.into(),
            created_at,
            expires_at,
            payload,
        };
        validate_intent(&intent)?;
        let operations = self.reader.all_operations()?;
        if operations.iter().any(|o| o.intent.intent_id == id) {
            return Err(conflict("intent identity is already reserved"));
        }
        if operations
            .iter()
            .any(|o| o.intent.key == key && unresolved(o))
        {
            return Err(conflict(
                "equivalent forge intent has an unresolved operation",
            ));
        }
        let referenced: HashSet<_> = operations
            .iter()
            .map(|o| o.intent.intent_id.as_str())
            .collect();
        let mut unused = 0;
        for name in names(&self.reader.intents, MAX_INTENTS)? {
            let id = record_id(&name)?;
            let value = self
                .reader
                .read_intent(id)?
                .ok_or_else(|| invalid("intent disappeared"))?;
            if !referenced.contains(value.intent_id.as_str()) {
                unused += 1;
            }
        }
        if unused >= MAX_UNUSED_INTENTS {
            return Err(capacity(
                "unused forge intent capacity reached; expire unused reviews",
            ));
        }
        self.publish(
            &self.reader.intents,
            &file_name(id),
            &intent,
            false,
            &mut |_| Ok(()),
        )?;
        Ok(intent)
    }
    pub fn reserve(
        &self,
        operation_id: &str,
        intent_id: &str,
        digest: &str,
    ) -> io::Result<Reservation> {
        self.reserve_with(operation_id, intent_id, digest, &mut |_| Ok(()))
    }
    fn reserve_with(
        &self,
        operation_id: &str,
        intent_id: &str,
        digest: &str,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<Reservation> {
        self.reader.check_location()?;
        valid_id(operation_id)?;
        valid_id(intent_id)?;
        valid_digest(digest)?;
        if let Some(operation) = self.reader.read_operation(operation_id)? {
            if operation.intent.intent_id != intent_id || operation.intent.digest != digest {
                return Err(conflict("operation identity cannot be rebound"));
            }
            return Ok(Reservation::Replay(operation));
        }
        let operations = self.reader.all_operations()?;
        if operations.len() >= MAX_OPERATIONS {
            return Err(capacity(
                "forge receipt capacity requires terminal archival",
            ));
        }
        if operations.iter().any(|o| o.intent.intent_id == intent_id) {
            return Err(conflict("intent is permanently bound to another operation"));
        }
        let intent = self
            .reader
            .read_intent(intent_id)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "intent not found"))?;
        if intent.digest != digest {
            return Err(conflict("review digest changed"));
        }
        if intent.expires_at <= now()? {
            return Err(conflict("forge review expired"));
        }
        if operations
            .iter()
            .any(|o| o.intent.key == intent.key && unresolved(o))
        {
            return Err(conflict(
                "equivalent forge intent has an unresolved operation",
            ));
        }
        let now = now()?;
        let operation = Operation {
            schema: 1,
            operation_id: operation_id.into(),
            intent: intent.clone(),
            created_at: now,
            updated_at: now,
            receipt_version: 1,
            dispatch_marked_at: None,
            recovery: None,
            phase: Phase::Reserved,
            outcome: None,
            reason: None,
            result: Value::Null,
        };
        self.publish(
            &self.reader.operations,
            &file_name(operation_id),
            &operation,
            false,
            hook,
        )?;
        Ok(Reservation::New {
            intent,
            ticket: ReservationTicket {
                lease: self.nonce.clone(),
                operation,
            },
        })
    }
    /// Consumes the sole pre-dispatch ticket even when marker publication fails.
    pub fn mark_dispatching(&self, ticket: ReservationTicket) -> io::Result<DispatchTicket> {
        self.mark_dispatching_with(ticket, &mut |_| Ok(()))
    }
    fn mark_dispatching_with(
        &self,
        ticket: ReservationTicket,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<DispatchTicket> {
        self.mark_dispatching_at(ticket, now()?, hook)
    }
    fn mark_dispatching_at(
        &self,
        ticket: ReservationTicket,
        observed_at: u64,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<DispatchTicket> {
        self.check_ticket(&ticket.lease, &ticket.operation, Phase::Reserved)?;
        if ticket.operation.intent.expires_at <= observed_at {
            // An expired preflight must not publish a dispatch marker. Preserve
            // its one-use reservation as an explicit pre-dispatch refusal.
            self.finish(
                ticket.operation,
                Outcome::NotSubmitted,
                "review_expired_before_dispatch",
                Value::Null,
                &mut |_| Ok(()),
            )?;
            return Err(conflict("forge review expired before dispatch"));
        }
        let mut operation = ticket.operation;
        operation.phase = Phase::Dispatching;
        operation.dispatch_marked_at = Some(now()?.max(operation.updated_at));
        operation.receipt_version += 1;
        operation.updated_at = now()?
            .max(operation.updated_at)
            .max(operation.dispatch_marked_at.unwrap_or(0));
        self.publish(
            &self.reader.operations,
            &file_name(&operation.operation_id),
            &operation,
            true,
            hook,
        )?;
        Ok(DispatchTicket {
            lease: self.nonce.clone(),
            operation,
        })
    }
    pub fn finish_not_submitted(
        &self,
        ticket: ReservationTicket,
        reason: &str,
        result: Value,
    ) -> io::Result<Operation> {
        self.check_ticket(&ticket.lease, &ticket.operation, Phase::Reserved)?;
        self.finish(
            ticket.operation,
            Outcome::NotSubmitted,
            reason,
            result,
            &mut |_| Ok(()),
        )
    }
    pub fn finish_dispatch(
        &self,
        ticket: DispatchTicket,
        outcome: Outcome,
        reason: &str,
        result: Value,
    ) -> io::Result<Operation> {
        self.finish_dispatch_with(ticket, outcome, reason, result, &mut |_| Ok(()))
    }
    fn finish_dispatch_with(
        &self,
        ticket: DispatchTicket,
        outcome: Outcome,
        reason: &str,
        result: Value,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<Operation> {
        if outcome == Outcome::NotSubmitted {
            return Err(invalid("dispatch can never become not submitted"));
        }
        self.check_ticket(&ticket.lease, &ticket.operation, Phase::Dispatching)?;
        self.finish(ticket.operation, outcome, reason, result, hook)
    }
    fn finish(
        &self,
        mut operation: Operation,
        outcome: Outcome,
        reason: &str,
        result: Value,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<Operation> {
        if reason.len() > 1024 || reason.chars().any(char::is_control) {
            return Err(invalid("invalid receipt reason"));
        }
        operation.phase = Phase::Terminal;
        operation.outcome = Some(outcome);
        operation.reason = Some(reason.into());
        operation.result = result;
        operation.receipt_version += 1;
        operation.updated_at = now()?
            .max(operation.updated_at)
            .max(operation.recovery.as_ref().map_or(0, |r| r.recovered_at));
        self.publish(
            &self.reader.operations,
            &file_name(&operation.operation_id),
            &operation,
            true,
            hook,
        )?;
        Ok(operation)
    }
    fn check_ticket(&self, lease: &str, operation: &Operation, phase: Phase) -> io::Result<()> {
        self.reader.check_location()?;
        if lease != self.nonce || operation.phase != phase {
            return Err(conflict("foreign or consumed forge ticket"));
        }
        let current = self
            .reader
            .read_operation(&operation.operation_id)?
            .ok_or_else(|| invalid("reserved operation disappeared"))?;
        if current != *operation {
            return Err(conflict("reserved operation changed"));
        }
        Ok(())
    }
    fn recover(&self) -> io::Result<()> {
        for mut operation in self.reader.all_operations()? {
            if operation.phase != Phase::Terminal {
                operation.recovery = Some(Recovery {
                    from_phase: operation.phase,
                    from_receipt_version: operation.receipt_version,
                    observed_updated_at: operation.updated_at,
                    recovered_at: now()?.max(operation.updated_at),
                });
            }
            match operation.phase {
                Phase::Reserved => {
                    self.finish(
                        operation,
                        Outcome::NotSubmitted,
                        "interrupted_before_dispatch",
                        Value::Null,
                        &mut |_| Ok(()),
                    )?;
                }
                Phase::Dispatching => {
                    self.finish(
                        operation,
                        Outcome::Unknown,
                        "interrupted_after_dispatch_marker",
                        Value::Null,
                        &mut |_| Ok(()),
                    )?;
                }
                Phase::Terminal => (),
            }
        }
        // A prior writer may have published a fully fsynced file but failed its
        // directory fsync. Adopt visible terminal evidence only after syncing
        // these immutable/atomically replaced names under the exclusive lease.
        self.reader.operations.sync_all()?;
        self.reader.intents.sync_all()?;
        Ok(())
    }
    /// Removes expired UNUSED reviews and redundant snapshots of proven terminal
    /// records only. Every operation retains its full immutable intent forever in
    /// this initial slice. Unknown intent snapshots are retained as well.
    pub fn compact_intents(&self, at: u64) -> io::Result<usize> {
        self.reader.check_location()?;
        let operations = self.reader.all_operations()?;
        let mut removed = 0;
        for name in names(&self.reader.intents, MAX_INTENTS)? {
            let id = record_id(&name)?;
            let intent = self
                .reader
                .read_intent(id)?
                .ok_or_else(|| invalid("intent disappeared"))?;
            let referenced = operations.iter().find(|o| o.intent.intent_id == id);
            let removable = match referenced {
                Some(op) => op.phase == Phase::Terminal && op.outcome != Some(Outcome::Unknown),
                None => intent.expires_at <= at,
            };
            if removable {
                let file = File::from(rustix::fs::openat(
                    &self.reader.intents,
                    name.as_str(),
                    READ_FLAGS,
                    Mode::empty(),
                )?);
                private_file(&file)?;
                same_entry(&self.reader.intents, &name, &file)?;
                rustix::fs::unlinkat(&self.reader.intents, name.as_str(), AtFlags::empty())?;
                self.reader.intents.sync_all()?;
                removed += 1;
            }
        }
        Ok(removed)
    }
    fn publish<T: Serialize>(
        &self,
        dir: &File,
        name: &str,
        value: &T,
        replace: bool,
        hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
    ) -> io::Result<()> {
        self.reader.check_location()?;
        let bytes = encode(value)?;
        let previous = if replace {
            let file = File::from(rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty())?);
            private_file(&file)?;
            file.metadata()?.len()
        } else {
            0
        };
        if self
            .reader
            .byte_total()?
            .saturating_sub(previous)
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > MAX_STORE_BYTES)
        {
            return Err(capacity(
                "forge store byte capacity requires terminal archival",
            ));
        }
        publish_bytes(dir, name, &bytes, replace, hook)?;
        self.reader.check_location()
    }
    fn cleanup_pending(&self) -> io::Result<()> {
        for (dir, max) in [
            (&self.reader.intents, MAX_INTENTS),
            (&self.reader.operations, MAX_OPERATIONS),
        ] {
            for name in names(dir, max + 16)? {
                if let Some(suffix) = name.strip_prefix("pending-") {
                    if ulid::Ulid::from_string(suffix).is_err() {
                        return Err(invalid("invalid pending record"));
                    }
                    let file = File::from(rustix::fs::openat(
                        dir,
                        name.as_str(),
                        READ_FLAGS,
                        Mode::empty(),
                    )?);
                    let meta = file.metadata()?;
                    if !meta.is_file()
                        || meta.mode() & 0o077 != 0
                        || meta.uid() != rustix::process::geteuid().as_raw()
                        || !(1..=2).contains(&meta.nlink())
                        || meta.len() > MAX_RECORD_BYTES
                    {
                        return Err(invalid("unsafe pending record"));
                    }
                    // Pending names are never authoritative. Removing a leftover
                    // publication link heals the temporary link count without
                    // touching the published reservation/receipt name.
                    rustix::fs::unlinkat(dir, name.as_str(), AtFlags::empty())?;
                    dir.sync_all()?;
                }
            }
        }
        Ok(())
    }
}

fn unresolved(operation: &Operation) -> bool {
    operation.phase != Phase::Terminal || operation.outcome == Some(Outcome::Unknown)
}
fn validate_intent(intent: &Intent) -> io::Result<()> {
    valid_id(&intent.intent_id)?;
    valid_digest(&intent.digest)?;
    valid_digest(&intent.key)?;
    if intent.schema != 1
        || intent.expires_at <= intent.created_at
        || intent.expires_at - intent.created_at > MAX_LIFETIME
        || digest(&intent.payload)? != intent.digest
    {
        return Err(invalid("invalid immutable forge intent"));
    }
    Ok(())
}
fn validate_operation(operation: &Operation) -> io::Result<()> {
    valid_id(&operation.operation_id)?;
    validate_intent(&operation.intent)?;
    let valid_phase = match (
        operation.phase,
        operation.outcome,
        operation.receipt_version,
    ) {
        (Phase::Reserved, None, 1) | (Phase::Dispatching, None, 2) => true,
        (Phase::Terminal, Some(Outcome::NotSubmitted), 2) => true,
        (Phase::Terminal, Some(Outcome::Accepted | Outcome::Rejected | Outcome::Unknown), 3) => {
            true
        }
        _ => false,
    };
    let valid_dispatch = match operation.phase {
        Phase::Reserved => operation.dispatch_marked_at.is_none(),
        Phase::Dispatching => operation.dispatch_marked_at.is_some(),
        Phase::Terminal if operation.outcome == Some(Outcome::NotSubmitted) => {
            operation.dispatch_marked_at.is_none()
        }
        Phase::Terminal => operation.dispatch_marked_at.is_some(),
    };
    let valid_recovery = operation.recovery.as_ref().is_none_or(|r| {
        operation.phase == Phase::Terminal
            && r.observed_updated_at <= r.recovered_at
            && r.recovered_at <= operation.updated_at
            && matches!(
                (r.from_phase, r.from_receipt_version, operation.outcome),
                (Phase::Reserved, 1, Some(Outcome::NotSubmitted))
                    | (Phase::Dispatching, 2, Some(Outcome::Unknown))
            )
    });
    if operation.schema != 1
        || !valid_phase
        || !valid_dispatch
        || !valid_recovery
        || operation.updated_at < operation.created_at
        || operation.created_at < operation.intent.created_at
        || operation
            .reason
            .as_ref()
            .is_some_and(|r| r.len() > 1024 || r.chars().any(char::is_control))
    {
        return Err(invalid("invalid forge operation receipt"));
    }
    Ok(())
}
pub fn digest(payload: &Value) -> io::Result<String> {
    Ok(format!("{:x}", Sha256::digest(encode(payload)?)))
}
fn file_name(id: &str) -> String {
    format!("{id}.json")
}
fn record_id(name: &str) -> io::Result<&str> {
    let id = name
        .strip_suffix(".json")
        .ok_or_else(|| invalid("unexpected store entry"))?;
    valid_id(id)?;
    Ok(id)
}
fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| invalid("invalid system clock"))
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn conflict(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}
fn capacity(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, message)
}
fn valid_id(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Err(invalid("invalid forge identity"))
    } else {
        Ok(())
    }
}
fn valid_digest(value: &str) -> io::Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(invalid("invalid forge digest"))
    }
}
fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid("absolute store path required"));
    }
    let mut file = File::from(rustix::fs::open("/", DIR_FLAGS, Mode::empty())?);
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => {
                file = File::from(rustix::fs::openat(&file, name, DIR_FLAGS, Mode::empty())?)
            }
            _ => return Err(invalid("normal store path required")),
        }
    }
    Ok(file)
}
fn private_directory(parent: &File, name: &str) -> io::Result<File> {
    match rustix::fs::mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) => parent.sync_all()?,
        Err(rustix::io::Errno::EXIST) => (),
        Err(e) => return Err(e.into()),
    }
    open_private_directory(parent, name)
}
fn open_private_directory(parent: &File, name: &str) -> io::Result<File> {
    let file = File::from(rustix::fs::openat(parent, name, DIR_FLAGS, Mode::empty())?);
    private_dir_metadata(&file)?;
    Ok(file)
}
fn private_dir_metadata(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
        Err(invalid("forge directory is not private"))
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
        Err(invalid("forge file must be private regular single-link"))
    } else {
        Ok(())
    }
}
fn same_file(a: &File, b: &File) -> io::Result<()> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    if a.dev() == b.dev() && a.ino() == b.ino() {
        Ok(())
    } else {
        Err(invalid("forge store identity changed"))
    }
}
fn same_entry(parent: &File, name: &str, expected: &File) -> io::Result<()> {
    let actual = File::from(rustix::fs::openat(parent, name, READ_FLAGS, Mode::empty())?);
    if expected.metadata()?.is_dir() {
        private_dir_metadata(&actual)?
    } else {
        private_file(&actual)?
    }
    same_file(&actual, expected)
}
fn names(dir: &File, max: usize) -> io::Result<Vec<String>> {
    let mut result = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        if result.len() >= max {
            return Err(capacity("forge store entry limit exceeded"));
        }
        result
            .push(String::from_utf8(bytes.to_vec()).map_err(|_| invalid("non UTF-8 store entry"))?);
    }
    result.sort();
    Ok(result)
}
struct CappedWriter {
    bytes: Vec<u8>,
}
impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > MAX_RECORD_BYTES.saturating_sub(self.bytes.len() as u64) {
            return Err(capacity("forge record exceeds byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let mut writer = CappedWriter { bytes: Vec::new() };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| invalid("invalid or oversized forge record"))?;
    Ok(writer.bytes)
}
fn read_optional<T: DeserializeOwned>(dir: &File, name: &str) -> io::Result<Option<T>> {
    let file = match rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty()) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    private_file(&file)?;
    let before = file.metadata()?;
    if before.len() > MAX_RECORD_BYTES {
        return Err(capacity("forge record exceeds byte budget"));
    }
    let mut bytes = Vec::new();
    (&file).take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(invalid("forge record changed during read"));
    }
    // Atomic replacement may race a lock-free status read. The captured inode
    // remains a coherent earlier receipt; status never makes it executable.
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid("invalid forge record"))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublishStage {
    FileSynced,
    Published,
    DirectorySynced,
}
fn publish_bytes(
    dir: &File,
    name: &str,
    bytes: &[u8],
    replace: bool,
    hook: &mut dyn FnMut(PublishStage) -> io::Result<()>,
) -> io::Result<()> {
    let previous = if replace {
        let file = File::from(rustix::fs::openat(dir, name, READ_FLAGS, Mode::empty())?);
        private_file(&file)?;
        Some(file)
    } else {
        None
    };
    let temporary = format!("pending-{}", ulid::Ulid::new());
    let mut file = File::from(rustix::fs::openat(
        dir,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    let result = (|| {
        private_file(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        hook(PublishStage::FileSynced)?;
        if let Some(previous) = previous {
            same_entry(dir, name, &previous)?;
            rustix::fs::renameat(dir, temporary.as_str(), dir, name)?;
        } else {
            rustix::fs::linkat(dir, temporary.as_str(), dir, name, AtFlags::empty())?;
            rustix::fs::unlinkat(dir, temporary.as_str(), AtFlags::empty())?;
        }
        hook(PublishStage::Published)?;
        dir.sync_all()?;
        hook(PublishStage::DirectorySynced)
    })();
    // A published name is NEVER removed after an uncertain fsync/publication.
    let _ = rustix::fs::unlinkat(dir, temporary.as_str(), AtFlags::empty());
    result
}

#[cfg(test)]
#[path = "forge_write_store_tests.rs"]
mod tests;
