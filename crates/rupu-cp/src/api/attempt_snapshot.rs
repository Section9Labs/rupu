//! Bounded immutable snapshots. Only capture reads ledger bytes; later pages
//! validate file metadata and decode selected rows from the captured bytes.
use super::{AttemptRow, AttemptsResponse, Evidence, Filters};
use crate::error::{ApiError, ApiResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rupu_orchestrator::runs::AttemptRecord;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::VecDeque,
    fs::{self, Metadata, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

const MAX_LEDGER_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 64 * 1024;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 64;
const TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Identity {
    fn of(m: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            created: m.created().ok(),
            #[cfg(unix)]
            device: m.dev(),
            #[cfg(unix)]
            inode: m.ino(),
        }
    }
}

struct Stamp {
    identity: Identity,
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(m: &Metadata) -> Self {
        Self {
            identity: Identity::of(m),
            len: m.len(),
            modified: m.modified().ok(),
        }
    }

    fn validate(&self, m: &Metadata) -> ApiResult<()> {
        if self.identity != Identity::of(m)
            || m.len() < self.len
            || (m.len() == self.len && m.modified().ok() != self.modified)
        {
            return Err(stale("ledger was replaced, truncated or rewritten"));
        }
        Ok(())
    }
}

struct Line {
    start: usize,
    end: usize,
    number: usize,
}

#[derive(Deserialize)]
struct Version {
    v: u32,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Owner {
    pub global_dir: PathBuf,
    pub run_store: PathBuf,
}

struct Snapshot {
    id: String,
    owner: Owner,
    run_id: String,
    filters: Filters,
    created: Instant,
    stamp: Option<Stamp>,
    bytes: Vec<u8>,
    matching: Vec<Line>,
    evidence: Evidence,
}

fn stale(reason: &str) -> ApiError {
    ApiError::conflict(format!(
        "attempt_snapshot_expired: {reason}; restart without cursor"
    ))
}

// Reject symlinks and special files before opening. Validate the opened file's
// identity before reading, so a path replacement cannot change the source.
fn metadata(path: &Path, root: &Path) -> ApiResult<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => {
            super::super::fs_safety::validate_within(path, root)?;
            Ok(Some(m))
        }
        Ok(_) => Err(ApiError::bad_request(
            "attempt ledger must be a regular file",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::internal(format!(
            "cannot inspect attempt ledger: {e}"
        ))),
    }
}

impl Snapshot {
    fn capture(owner: &Owner, root: &Path, run_id: &str, filters: Filters) -> ApiResult<Self> {
        let path = root.join(run_id).join("attempts.jsonl");
        let m = metadata(&path, root)?;
        let mut bytes = Vec::new();
        let stamp = if let Some(m) = m {
            if m.len() > MAX_LEDGER_BYTES as u64 {
                return Err(ApiError(
                    axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                    "attempt_snapshot_too_large: ledger exceeds 16 MiB; no rows were truncated"
                        .into(),
                ));
            }
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(
                    (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
                );
            }
            let mut f = options
                .open(&path)
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let opened = f
                .metadata()
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let stamp = Stamp::of(&m);
            stamp.validate(&opened)?;
            bytes.resize(m.len() as usize, 0);
            f.read_exact(&mut bytes)
                .map_err(|_| stale("ledger changed during capture"))?;
            let current = metadata(&path, root)?.ok_or_else(|| stale("ledger disappeared"))?;
            stamp.validate(&current)?;
            Some(stamp)
        } else {
            None
        };
        let boundary = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let mut evidence = Evidence {
            ledger: if stamp.is_some() {
                "present"
            } else {
                "missing"
            },
            parsing: "clean",
            execution_history_complete: "unknown",
            malformed_rows: 0,
            unsupported_rows: 0,
            oversized_rows: 0,
            incomplete_tail_bytes: bytes.len() - boundary,
            snapshot_bytes: boundary,
        };
        bytes.truncate(boundary);
        let mut matching = Vec::new();
        let mut start = 0;
        for (i, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            let end = start + line.len();
            if line.len() > MAX_ROW_BYTES {
                evidence.oversized_rows += 1;
            } else if !line.iter().all(u8::is_ascii_whitespace) {
                match serde_json::from_slice::<Version>(line) {
                    Ok(version) if version.v != 1 => evidence.unsupported_rows += 1,
                    Ok(_) => match serde_json::from_slice::<AttemptRecord>(line) {
                        Ok(row) if !row.step_id.is_empty() && !row.agent_run_id.is_empty() => {
                            if filters.matches(&row) {
                                matching.push(Line {
                                    start,
                                    end,
                                    number: i + 1,
                                });
                            }
                        }
                        _ => evidence.malformed_rows += 1,
                    },
                    _ => evidence.malformed_rows += 1,
                }
            }
            start = end;
        }
        if evidence.malformed_rows
            + evidence.unsupported_rows
            + evidence.oversized_rows
            + evidence.incomplete_tail_bytes
            > 0
        {
            evidence.parsing = "partial";
        }
        Ok(Self {
            id: ulid::Ulid::new().to_string(),
            owner: owner.clone(),
            run_id: run_id.into(),
            filters,
            created: Instant::now(),
            stamp,
            bytes,
            matching,
            evidence,
        })
    }

    fn weight(&self) -> usize {
        self.bytes.capacity() + self.matching.capacity() * std::mem::size_of::<Line>() + 4096
    }

    fn validate(
        &self,
        owner: &Owner,
        root: &Path,
        run_id: &str,
        filters: &Filters,
    ) -> ApiResult<()> {
        if &self.owner != owner || self.run_id != run_id || self.filters != *filters {
            return Err(ApiError::bad_request(
                "attempt cursor owner, run or filters do not match",
            ));
        }
        if self.created.elapsed() >= TTL {
            return Err(stale("snapshot TTL elapsed"));
        }
        if let Some(stamp) = &self.stamp {
            let current = metadata(&root.join(run_id).join("attempts.jsonl"), root)?
                .ok_or_else(|| stale("ledger disappeared"))?;
            stamp.validate(&current)?;
        }
        Ok(())
    }

    fn rows(&self, start: usize, limit: usize) -> ApiResult<Vec<AttemptRow>> {
        self.matching
            .iter()
            .skip(start)
            .take(limit)
            .map(|line| {
                serde_json::from_slice(&self.bytes[line.start..line.end])
                    .map(|r| AttemptRow::recorded(line.number, r))
                    .map_err(|e| ApiError::internal(format!("invalid captured attempt: {e}")))
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    snapshot: String,
    position: usize,
}

struct Cache {
    snapshots: VecDeque<Snapshot>,
    secret: [u8; 32],
}

impl Default for Cache {
    fn default() -> Self {
        // ULIDs supply 80 random bits each. Use the random portions of four
        // independently generated IDs for a process-local 256-bit MAC key.
        let mut secret = [0; 32];
        for chunk in secret.chunks_mut(8) {
            chunk.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        Self {
            snapshots: VecDeque::new(),
            secret,
        }
    }
}

impl Cache {
    fn mac(&self) -> Hmac<Sha256> {
        Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts a 32-byte key")
    }

    fn encode(&self, cursor: &Cursor) -> ApiResult<String> {
        let bytes = serde_json::to_vec(cursor).map_err(|e| ApiError::internal(e.to_string()))?;
        let mut mac = self.mac();
        mac.update(&bytes);
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&bytes),
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        ))
    }

    fn decode(&self, token: &str) -> ApiResult<Cursor> {
        let invalid = || ApiError::bad_request("invalid attempt cursor");
        let (payload, signature) = token.split_once('.').ok_or_else(invalid)?;
        let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid())?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
        let mut mac = self.mac();
        mac.update(&bytes);
        mac.verify_slice(&signature).map_err(|_| invalid())?;
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    }

    fn insert(&mut self, snapshot: Snapshot) {
        self.snapshots.retain(|s| s.created.elapsed() < TTL);
        while self.snapshots.len() >= MAX_SNAPSHOTS
            || self.snapshots.iter().map(Snapshot::weight).sum::<usize>() + snapshot.weight()
                > MAX_CACHE_BYTES
        {
            if self.snapshots.pop_front().is_none() {
                break;
            }
        }
        self.snapshots.push_back(snapshot);
    }

    fn page(
        &mut self,
        owner: &Owner,
        root: &Path,
        run_id: &str,
        filters: Filters,
        limit: usize,
        token: Option<&str>,
    ) -> ApiResult<AttemptsResponse> {
        let cursor = if let Some(token) = token {
            self.decode(token)?
        } else {
            let snapshot = Snapshot::capture(owner, root, run_id, filters.clone())?;
            let cursor = Cursor {
                snapshot: snapshot.id.clone(),
                position: 0,
            };
            self.insert(snapshot);
            cursor
        };
        let snapshot = self
            .snapshots
            .iter()
            .find(|s| s.id == cursor.snapshot)
            .ok_or_else(|| stale("snapshot was evicted"))?;
        snapshot.validate(owner, root, run_id, &filters)?;
        if cursor.position > snapshot.matching.len() {
            return Err(ApiError::bad_request("invalid attempt cursor position"));
        }
        let rows = snapshot.rows(cursor.position, limit)?;
        let next = cursor.position + rows.len();
        let next_cursor = if next < snapshot.matching.len() {
            Some(self.encode(&Cursor {
                snapshot: snapshot.id.clone(),
                position: next,
            })?)
        } else {
            None
        };
        Ok(AttemptsResponse {
            schema_version: 1,
            workflow_run_id: run_id.into(),
            owner_host_id: "local",
            snapshot: snapshot.id.clone(),
            rows,
            next_cursor,
            recorded_matching_total: snapshot.stamp.as_ref().map(|_| snapshot.matching.len()),
            evidence: snapshot.evidence.clone(),
        })
    }
}

pub(super) fn page(
    owner: &Owner,
    root: &Path,
    run_id: &str,
    filters: Filters,
    limit: usize,
    token: Option<&str>,
) -> ApiResult<AttemptsResponse> {
    // Holding the lock through capture also bounds concurrent capture memory.
    // No network/provider/transcript I/O occurs while the lock is held.
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| ApiError::internal("attempt snapshot cache unavailable"))?
        .page(owner, root, run_id, filters, limit, token)
}

#[cfg(test)]
#[path = "attempt_snapshot_tests.rs"]
mod tests;
