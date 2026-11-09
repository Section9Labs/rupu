//! Immutable, process-local child catalog snapshots. Captures are serialized;
//! continuation requests only copy a bounded page of already materialized DTOs.
use super::{ChildDto, ChildrenQuery};
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[path = "children_capture.rs"]
mod capture;
#[cfg(test)]
#[path = "children_pages_tests.rs"]
mod tests;

const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SNAPSHOTS: usize = 64;
const TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Default, Serialize)]
pub(super) struct Evidence {
    consistency: &'static str,
    history_complete: &'static str,
    source_scan: &'static str,
    malformed_rows: usize,
    incomplete_tail_bytes: usize,
    run_directories_scanned: usize,
    directory_entries_scanned: usize,
    bytes_read: usize,
    source_rows: usize,
    missing_legacy_transcripts: usize,
    remote_step_roots_not_walked: usize,
}

#[derive(Serialize)]
pub(super) struct Page {
    schema_version: u32,
    root_type: &'static str,
    owner_host_id: &'static str,
    root_run_id: String,
    snapshot: String,
    children: Vec<ChildDto>,
    next_cursor: Option<String>,
    snapshot_matching_total: usize,
    evidence: Evidence,
}

#[derive(Clone, PartialEq, Eq)]
struct Owner {
    global: PathBuf,
    runs: PathBuf,
}

struct Snapshot {
    id: String,
    owner: Owner,
    run_id: String,
    parent: Option<String>,
    proof: capture::RootProof,
    created: Instant,
    rows: Vec<ChildDto>,
    weights: Vec<usize>,
    weight: usize,
    evidence: Evidence,
}

fn stale(reason: &str) -> ApiError {
    ApiError::conflict(format!(
        "children_snapshot_expired: {reason}; restart without cursor"
    ))
}
fn too_large(reason: &str) -> ApiError {
    ApiError(
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        format!("children_snapshot_too_large: {reason}; no partial result was returned"),
    )
}
fn row_weight(row: &ChildDto) -> ApiResult<usize> {
    // JSON bytes cover owned string bytes; extra space covers DTO fields and
    // allocation bookkeeping, including the parallel page-weight index.
    serde_json::to_vec(row)
        .map(|v| v.len() + std::mem::size_of::<ChildDto>() + 1024)
        .map_err(|e| ApiError::internal(e.to_string()))
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    snapshot: String,
    position: usize,
}

struct Cache {
    snapshots: VecDeque<Snapshot>,
    secret: [u8; 32],
    next_id: u64,
}
impl Default for Cache {
    fn default() -> Self {
        let mut secret = [0; 32];
        for chunk in secret.chunks_mut(8) {
            chunk.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        Self {
            snapshots: VecDeque::new(),
            secret,
            next_id: 0,
        }
    }
}
impl Cache {
    fn encode(&self, cursor: &Cursor) -> ApiResult<String> {
        let bytes = serde_json::to_vec(cursor).map_err(|e| ApiError::internal(e.to_string()))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("valid MAC key");
        mac.update(&bytes);
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&bytes),
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        ))
    }
    fn decode(&self, token: &str) -> ApiResult<Cursor> {
        let invalid = || ApiError::bad_request("invalid children cursor");
        if token.len() > 1024 {
            return Err(invalid());
        }
        let (body, signature) = token.split_once('.').ok_or_else(invalid)?;
        let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("valid MAC key");
        mac.update(&bytes);
        mac.verify_slice(&signature).map_err(|_| invalid())?;
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    }
    fn insert(&mut self, mut snapshot: Snapshot) -> ApiResult<Cursor> {
        if snapshot.weight > MAX_SNAPSHOT_BYTES {
            return Err(too_large("materialized rows exceed 16 MiB"));
        }
        self.snapshots.retain(|s| s.created.elapsed() < TTL);
        while self.snapshots.len() >= MAX_SNAPSHOTS
            || self.snapshots.iter().map(|s| s.weight).sum::<usize>() + snapshot.weight
                > MAX_CACHE_BYTES
        {
            self.snapshots.pop_front();
        }
        // A monotonic process-local sequence cannot collide, including with
        // evicted snapshots. The process-local MAC key separates restarts.
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| ApiError::internal("children snapshot identity exhausted"))?;
        snapshot.id = format!("children-{:016x}", self.next_id);
        let cursor = Cursor {
            snapshot: snapshot.id.clone(),
            position: 0,
        };
        self.snapshots.push_back(snapshot);
        Ok(cursor)
    }
    fn page(
        &self,
        owner: &Owner,
        run: &str,
        parent: &Option<String>,
        limit: usize,
        cursor: Cursor,
    ) -> ApiResult<Page> {
        let s = self
            .snapshots
            .iter()
            .find(|s| s.id == cursor.snapshot)
            .ok_or_else(|| stale("snapshot was evicted or process restarted"))?;
        if s.created.elapsed() >= TTL {
            return Err(stale("snapshot expired"));
        }
        if &s.owner != owner || s.run_id != run || &s.parent != parent {
            return Err(ApiError::bad_request(
                "children cursor belongs to a different owner, run or parent filter",
            ));
        }
        s.proof.validate()?;
        if cursor.position > s.rows.len() {
            return Err(ApiError::bad_request("invalid children cursor position"));
        }
        let mut end = cursor.position;
        let mut bytes = 0;
        while end < s.rows.len() && end - cursor.position < limit {
            let weight = s.weights[end];
            if bytes + weight > MAX_PAGE_BYTES {
                break;
            }
            bytes += weight;
            end += 1;
        }
        if end == cursor.position && end < s.rows.len() {
            return Err(too_large("one row exceeds the page byte budget"));
        }
        let next_cursor = (end < s.rows.len())
            .then(|| {
                self.encode(&Cursor {
                    snapshot: s.id.clone(),
                    position: end,
                })
            })
            .transpose()?;
        Ok(Page {
            schema_version: 1,
            root_type: s.proof.kind,
            owner_host_id: "local",
            root_run_id: run.into(),
            snapshot: s.id.clone(),
            children: s.rows[cursor.position..end].to_vec(),
            next_cursor,
            snapshot_matching_total: s.rows.len(),
            evidence: s.evidence.clone(),
        })
    }
}

pub(super) fn get(state: &AppState, run: &str, q: ChildrenQuery) -> ApiResult<Page> {
    if let Some(parent) = &q.parent_run_id {
        capture::valid_id(parent)?;
    }
    let owner = Owner {
        global: state
            .global_dir
            .canonicalize()
            .map_err(|e| ApiError::internal(e.to_string()))?,
        runs: if state.run_store.root.exists() {
            state
                .run_store
                .root
                .canonicalize()
                .map_err(|e| ApiError::internal(e.to_string()))?
        } else {
            let parent = state
                .run_store
                .root
                .parent()
                .ok_or_else(|| ApiError::internal("run store has no parent"))?;
            let name = state
                .run_store
                .root
                .file_name()
                .ok_or_else(|| ApiError::internal("run store has no name"))?;
            parent
                .canonicalize()
                .map_err(|e| ApiError::internal(e.to_string()))?
                .join(name)
        },
    };
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    static CAPTURE: OnceLock<Mutex<()>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    let cursor = if let Some(token) = q.cursor.as_deref() {
        cache
            .lock()
            .map_err(|_| ApiError::internal("children cache unavailable"))?
            .decode(token)?
    } else {
        // Bound transient parsing memory as well as retained cache memory.
        let _capture = CAPTURE
            .get_or_init(|| Mutex::new(()))
            .try_lock()
            .map_err(|_| {
                ApiError(
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    "children capture busy; try again later".into(),
                )
            })?;
        let snapshot = capture::capture(owner.clone(), run, q.parent_run_id.clone())?;
        cache
            .lock()
            .map_err(|_| ApiError::internal("children cache unavailable"))?
            .insert(snapshot)?
    };
    cache
        .lock()
        .map_err(|_| ApiError::internal("children cache unavailable"))?
        .page(
            &owner,
            run,
            &q.parent_run_id,
            q.limit.unwrap_or(50).clamp(1, 200),
            cursor,
        )
}
