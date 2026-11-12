use super::types::*;
use super::validation::{FINAL_RESERVE_BYTES, MAX_RECORD_BYTES};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) const DATABASE_NAME: &str = "work-queue.sqlite3";
const APPLICATION_ID: i64 = 0x52575131;

#[cfg(test)]
thread_local! { pub(crate) static INIT_CRASH: std::cell::Cell<u8> = const {std::cell::Cell::new(0)}; }

#[cfg(test)]
fn initialization_crash(stage: u8) {
    if INIT_CRASH.with(|value| value.get()) == stage {
        // Subprocess tests intentionally bypass Drop and SQLite rollback.
        std::process::exit(0);
    }
}

pub(crate) struct Store {
    pub conn: Connection,
    path: PathBuf,
    identity: (u64, u64),
    directory_identity: (u64, u64),
}

#[derive(Clone)]
pub(crate) struct Meta {
    pub queue_id: Option<String>,
    pub revision: u64,
    pub items: u64,
    pub events: u64,
    pub receipts: u64,
    pub open_items: u64,
    pub max_bytes: u64,
}

pub(crate) fn io_error(e: impl std::fmt::Display) -> WorkError {
    WorkError::Persistence(e.to_string())
}
pub(crate) fn sql_error(e: rusqlite::Error) -> WorkError {
    use rusqlite::ErrorCode;
    match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => WorkError::Busy,
        Some(
            ErrorCode::DatabaseCorrupt
            | ErrorCode::NotADatabase
            | ErrorCode::SchemaChanged
            | ErrorCode::ConstraintViolation,
        ) => WorkError::Corrupt(e.to_string()),
        Some(ErrorCode::ReadOnly) => {
            WorkError::ReadOnly("database requires writable recovery or is read-only".into())
        }
        // SQLITE_FULL also means filesystem exhaustion, not necessarily the
        // configured page ceiling. Do not invent capacity counters here.
        Some(ErrorCode::DiskFull) => WorkError::Persistence(
            "Queue storage is full; inspect filesystem and database capacity".into(),
        ),
        _ => WorkError::Persistence(e.to_string()),
    }
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<String, WorkError> {
    let text = serde_json::to_string(value).map_err(io_error)?;
    if text.len() > MAX_RECORD_BYTES {
        return Err(WorkError::InvalidInput(
            "Queue record exceeds bounded storage envelope".into(),
        ));
    }
    Ok(text)
}
pub(crate) fn decode<T: DeserializeOwned>(text: &str) -> Result<T, WorkError> {
    if text.len() > MAX_RECORD_BYTES {
        return Err(WorkError::Corrupt("oversized Queue record".into()));
    }
    serde_json::from_str(text).map_err(|e| WorkError::Corrupt(e.to_string()))
}

fn identity(meta: &fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

fn directory(path: &Path, create: bool) -> Result<bool, WorkError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(true),
        Ok(_) => Err(WorkError::Denied(
            "Queue directory is redirected or not a directory".into(),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !create => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| WorkError::Denied("missing Queue parent".into()))?;
            directory(parent, true)?;
            match fs::create_dir(path) {
                Ok(()) => {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                        .map_err(io_error)?;
                    fs::File::open(parent)
                        .and_then(|f| f.sync_all())
                        .map_err(io_error)?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(io_error(e)),
            }
            directory(path, false)
        }
        Err(e) => Err(io_error(e)),
    }
}

fn regular(path: &Path, maximum: u64) -> Result<Option<fs::Metadata>, WorkError> {
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.is_file()
                && !meta.file_type().is_symlink()
                && meta.nlink() == 1
                && meta.len() <= maximum =>
        {
            Ok(Some(meta))
        }
        Ok(_) => Err(WorkError::Denied(
            "Queue file is linked, special or exceeds its storage bound".into(),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}

impl Store {
    pub fn open(space: &Path, writable: bool) -> Result<Option<Self>, WorkError> {
        Self::open_mode(space, writable, writable)
    }

    /// Recover an existing hot rollback journal under the caller's owner/Bus
    /// locks. Never initializes missing storage merely to look for a receipt.
    pub fn recover_existing(space: &Path) -> Result<Option<Self>, WorkError> {
        Self::open_mode(space, true, false)
    }

    fn open_mode(space: &Path, writable: bool, create: bool) -> Result<Option<Self>, WorkError> {
        if !directory(space, create)? {
            return Ok(None);
        }
        let board = space.join("board");
        if !directory(&board, create)? {
            return Ok(None);
        }
        // Canonicalize only after rejecting the selected directories themselves.
        // CP/runtime additionally pin the owner and hold its lifecycle lease.
        let board = board.canonicalize().map_err(io_error)?;
        let directory_identity = identity(&fs::metadata(&board).map_err(io_error)?);
        let path = board.join(DATABASE_NAME);
        for suffix in ["-wal", "-shm"] {
            if fs::symlink_metadata(board.join(format!("{DATABASE_NAME}{suffix}"))).is_ok() {
                return Err(WorkError::Denied(
                    "unexpected Queue WAL sidecar; no automatic migration".into(),
                ));
            }
        }
        // A legitimate rollback journal can be needed for crash recovery. Never
        // reject it merely for existing, and never manually remove it.
        regular(
            &board.join(format!("{DATABASE_NAME}-journal")),
            DEFAULT_DATABASE_MAX_BYTES + 1024 * 1024,
        )?;
        if regular(&path, DEFAULT_DATABASE_MAX_BYTES)?.is_none() {
            if !create {
                return Ok(None);
            }
            initialize(&board)?;
        }
        let metadata =
            regular(&path, DEFAULT_DATABASE_MAX_BYTES)?.ok_or(WorkError::OwnerChanged)?;
        let flags = if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let conn = Connection::open_with_flags(
            &path,
            flags | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        configure(&conn, writable)?;
        let application: i64 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(sql_error)?;
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(sql_error)?;
        if application != APPLICATION_ID {
            return Err(WorkError::Corrupt("not a Board Queue database".into()));
        }
        if version != WORK_QUEUE_SCHEMA_VERSION {
            return Err(WorkError::UnsupportedSchema { version });
        }
        let store = Self {
            conn,
            path,
            identity: identity(&metadata),
            directory_identity,
        };
        store.check_location()?;
        let meta = read_meta(&store.conn)?;
        if writable {
            let page_size: u64 = store
                .conn
                .pragma_query_value(None, "page_size", |r| number(r, 0))
                .map_err(sql_error)?;
            let effective: u64 = store
                .conn
                .pragma_update_and_check(
                    None,
                    "max_page_count",
                    (meta.max_bytes / page_size) as i64,
                    |r| number(r, 0),
                )
                .map_err(sql_error)?;
            if effective > meta.max_bytes / page_size {
                return Err(WorkError::Corrupt(
                    "Queue database already exceeds configured capacity".into(),
                ));
            }
        }
        Ok(Some(store))
    }

    pub fn check_location(&self) -> Result<(), WorkError> {
        let meta =
            regular(&self.path, DEFAULT_DATABASE_MAX_BYTES)?.ok_or(WorkError::OwnerChanged)?;
        let parent =
            fs::symlink_metadata(self.path.parent().expect("database parent")).map_err(io_error)?;
        if !parent.is_dir()
            || identity(&meta) != self.identity
            || identity(&parent) != self.directory_identity
        {
            return Err(WorkError::OwnerChanged);
        }
        Ok(())
    }
}

fn configure(conn: &Connection, writable: bool) -> Result<(), WorkError> {
    conn.busy_timeout(Duration::from_millis(250))
        .map_err(sql_error)?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")
        .map_err(sql_error)?;
    if !writable {
        conn.pragma_update(None, "query_only", true)
            .map_err(sql_error)?;
        return Ok(());
    }
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))
        .map_err(sql_error)?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(WorkError::Persistence(
            "Queue rollback-journal mode could not be established".into(),
        ));
    }
    conn.pragma_update(None, "synchronous", "EXTRA")
        .map_err(sql_error)?;
    let synchronous: i64 = conn
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(sql_error)?;
    if synchronous != 3 {
        return Err(WorkError::Persistence(
            "Queue EXTRA durability could not be established".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        conn.pragma_update(None, "fullfsync", true)
            .map_err(sql_error)?;
        let enabled: i64 = conn
            .pragma_query_value(None, "fullfsync", |row| row.get(0))
            .map_err(sql_error)?;
        if enabled != 1 {
            return Err(WorkError::Persistence("Queue fullfsync unavailable".into()));
        }
    }
    Ok(())
}

/// Publish only a complete, closed, synced schema. The caller holds the Bus
/// lock, and NOREPLACE additionally prevents clobbering any unexpected final
/// file. A process exit can leave an unpublished staging file; readers never
/// mistake it for a Queue or attempt to reconstruct/repair the final database.
fn initialize(board: &Path) -> Result<(), WorkError> {
    use rustix::fs::{Mode, OFlags, RenameFlags};
    let directory = fs::File::from(
        rustix::fs::open(
            board,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?,
    );
    let name = format!(".work-queue-init-{}.sqlite3", ulid::Ulid::new());
    let path = board.join(&name);
    let file = fs::File::from(
        rustix::fs::openat(
            &directory,
            name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(io_error)?,
    );
    let original_identity = identity(&file.metadata().map_err(io_error)?);
    let result = (|| {
        #[cfg(test)]
        initialization_crash(1);
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sql_error)?;
        configure(&conn, true)?;
        init(&conn)?;
        let meta = read_meta(&conn)?;
        if meta.queue_id.is_some() || meta.revision != 0 {
            return Err(WorkError::Corrupt(
                "initial Queue schema is not empty".into(),
            ));
        }
        conn.close().map_err(|(_, error)| sql_error(error))?;
        file.sync_all().map_err(io_error)?;
        let staged = regular(&path, DEFAULT_DATABASE_MAX_BYTES)?.ok_or(WorkError::OwnerChanged)?;
        if identity(&staged) != original_identity
            || identity(&fs::metadata(board).map_err(io_error)?)
                != identity(&directory.metadata().map_err(io_error)?)
        {
            return Err(WorkError::OwnerChanged);
        }
        #[cfg(test)]
        initialization_crash(3);
        rustix::fs::renameat_with(
            &directory,
            name.as_str(),
            &directory,
            DATABASE_NAME,
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            if error == rustix::io::Errno::EXIST {
                WorkError::OwnerChanged
            } else {
                io_error(error)
            }
        })?;
        #[cfg(test)]
        initialization_crash(4);
        directory.sync_all().map_err(io_error)?;
        #[cfg(test)]
        initialization_crash(5);
        Ok(())
    })();
    // Only names exclusively allocated by this invocation are cleanup targets.
    // A published final database is never removed after an uncertain sync.
    for suffix in ["", "-journal"] {
        let temporary = format!("{name}{suffix}");
        let _ = rustix::fs::unlinkat(&directory, temporary.as_str(), rustix::fs::AtFlags::empty());
    }
    result
}

pub(crate) fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn init(conn: &Connection) -> Result<(), WorkError> {
    conn.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), queue_id TEXT, revision INTEGER NOT NULL CHECK(revision>=0), item_count INTEGER NOT NULL CHECK(item_count>=0), event_count INTEGER NOT NULL CHECK(event_count>=0), receipt_count INTEGER NOT NULL CHECK(receipt_count>=0), open_items INTEGER NOT NULL CHECK(open_items>=0), max_bytes INTEGER NOT NULL CHECK(max_bytes>0)) STRICT;
        INSERT INTO meta VALUES(1,NULL,0,0,0,0,0,2147483648);
        CREATE TABLE items (id TEXT PRIMARY KEY, creation_sequence INTEGER NOT NULL UNIQUE, revision INTEGER NOT NULL CHECK(revision>0), state TEXT NOT NULL, claimant TEXT NOT NULL, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=262144), summary TEXT NOT NULL CHECK(length(CAST(summary AS BLOB))<=262144)) STRICT;
        CREATE INDEX items_state_sequence ON items(state,creation_sequence DESC);
        CREATE INDEX items_claimant_sequence ON items(claimant,creation_sequence DESC);
        CREATE INDEX items_state_claimant_sequence ON items(state,claimant,creation_sequence DESC);
        CREATE TABLE attempts (id TEXT PRIMARY KEY, item_id TEXT NOT NULL REFERENCES items(id), number INTEGER NOT NULL, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=262144), UNIQUE(item_id,number)) STRICT;
        CREATE TABLE events (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, item_id TEXT NOT NULL REFERENCES items(id), item_revision INTEGER NOT NULL, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=262144), UNIQUE(item_id,item_revision)) STRICT;
        CREATE INDEX events_item_sequence ON events(item_id,sequence);
        CREATE TABLE receipts (actor TEXT NOT NULL, request_id TEXT NOT NULL, digest TEXT NOT NULL, payload TEXT NOT NULL CHECK(length(CAST(payload AS BLOB))<=262144), PRIMARY KEY(actor,request_id)) STRICT;
        CREATE TABLE counts (state TEXT NOT NULL, claimant TEXT NOT NULL, count INTEGER NOT NULL CHECK(count>=0), PRIMARY KEY(state,claimant)) STRICT;
        PRAGMA application_id=1381454129;
        PRAGMA user_version=1;").map_err(sql_error)?;
    #[cfg(test)]
    initialization_crash(2);
    conn.execute_batch("COMMIT;").map_err(sql_error)?;
    Ok(())
}

pub(crate) fn read_meta(conn: &Connection) -> Result<Meta, WorkError> {
    let meta=conn.query_row("SELECT queue_id,revision,item_count,event_count,receipt_count,open_items,max_bytes FROM meta WHERE singleton=1",[],|r|Ok(Meta {queue_id:r.get(0)?,revision:number(r,1)?,items:number(r,2)?,events:number(r,3)?,receipts:number(r,4)?,open_items:number(r,5)?,max_bytes:number(r,6)?})).map_err(sql_error)?;
    if meta.max_bytes > DEFAULT_DATABASE_MAX_BYTES
        || meta.max_bytes == 0
        || meta.revision != meta.events
        || meta.events != meta.receipts
        || meta.open_items > meta.items
        || meta.items > meta.events
        || (meta.queue_id.is_none() && meta.revision != 0)
    {
        return Err(WorkError::Corrupt(
            "Queue metadata invariants failed".into(),
        ));
    }
    Ok(meta)
}

pub(crate) fn snapshot(conn: &Connection, meta: &Meta) -> Result<WorkSnapshot, WorkError> {
    let pages: u64 = conn
        .pragma_query_value(None, "page_count", |r| number(r, 0))
        .map_err(sql_error)?;
    let page_size: u64 = conn
        .pragma_query_value(None, "page_size", |r| number(r, 0))
        .map_err(sql_error)?;
    let bytes = pages
        .checked_mul(page_size)
        .ok_or_else(|| WorkError::Corrupt("invalid Queue page count".into()))?;
    let reserved = meta.open_items.saturating_mul(FINAL_RESERVE_BYTES);
    let remaining = meta
        .max_bytes
        .saturating_sub(bytes)
        .saturating_sub(reserved);
    Ok(WorkSnapshot {
        schema_version: WORK_QUEUE_SCHEMA_VERSION,
        availability: if meta.queue_id.is_some() {
            WorkAvailability::Available
        } else {
            WorkAvailability::NotInitialized
        },
        queue_id: meta.queue_id.clone(),
        revision: meta.revision,
        capacity: WorkCapacity {
            item_count: meta.items,
            event_count: meta.events,
            receipt_count: meta.receipts,
            database_bytes: bytes,
            database_max_bytes: meta.max_bytes,
            admission: if remaining >= FINAL_RESERVE_BYTES {
                WorkAdmissionStatus::Accepting
            } else {
                WorkAdmissionStatus::CapacityLimited
            },
            reserved_bytes: reserved,
            remaining_bytes: remaining,
        },
    })
}

pub(crate) fn empty_snapshot() -> WorkSnapshot {
    WorkSnapshot {
        schema_version: WORK_QUEUE_SCHEMA_VERSION,
        availability: WorkAvailability::NotInitialized,
        queue_id: None,
        revision: 0,
        capacity: WorkCapacity {
            item_count: 0,
            event_count: 0,
            receipt_count: 0,
            database_bytes: 0,
            database_max_bytes: DEFAULT_DATABASE_MAX_BYTES,
            admission: WorkAdmissionStatus::Accepting,
            reserved_bytes: 0,
            remaining_bytes: DEFAULT_DATABASE_MAX_BYTES,
        },
    }
}

pub(crate) fn item(conn: &Connection, id: &str) -> Result<WorkItem, WorkError> {
    let row: Option<(String, u64, u64, String)> = conn
        .query_row(
            "SELECT payload,revision,creation_sequence,state FROM items WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, number(r, 1)?, number(r, 2)?, r.get(3)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let (payload, revision, sequence, state) =
        row.ok_or_else(|| WorkError::NotFound { work_id: id.into() })?;
    let item: WorkItem = decode(&payload)?;
    if item.work_id != id
        || item.revision != revision
        || item.creation_sequence != sequence
        || item.state.key() != state
        || item.latest_event.work_id != id
        || item.latest_event.item_revision != revision
        || item.latest_event.to_state != item.state
        || item.event_count != revision
    {
        return Err(WorkError::Corrupt("Queue item invariants failed".into()));
    }
    Ok(item)
}
