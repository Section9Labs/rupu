//! Finite recorded transcript pages. Cursors seek directly to an authenticated
//! byte offset and reject any file change rather than combining snapshots.
use crate::{
    api::{fs_open::open_regular_file, local_only::LocalOnlyQuery},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, Metadata},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const CAPABILITY: &str = "transcript.pages.v1";

const MAX_RECORDS: usize = 200;
const MAX_LINE_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct PageQuery {
    /// Recorded transcript path, as returned by the owning run or session.
    pub path: String,
    /// Omit or use `local`; remote snapshot paging is not supported.
    pub host: Option<String>,
    /// Owning authorization run. Bound into the cursor, as with transcript reads.
    pub run: Option<String>,
    /// Maximum nonblank records (default 100, clamped to 1..200).
    pub limit: Option<usize>,
    /// Opaque next_cursor from the same path, run, host and unchanged file.
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Page {
    version: u8,
    path: String,
    host_id: &'static str,
    run: Option<String>,
    snapshot: String,
    records: Vec<Record>,
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct Record {
    /// One-based physical nonblank line position, including invalid records.
    position: u64,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub(crate) len: u64,
    modified: u128,
    created: Option<u128>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl Stamp {
    pub(crate) fn of(metadata: &Metadata) -> ApiResult<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let nanos = |time: SystemTime| time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos());
        Ok(Self {
            len: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(nanos)
                .ok_or_else(|| ApiError::internal("transcript modification time unavailable"))?,
            created: metadata.created().ok().and_then(nanos),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Cursor {
    version: u8,
    scope: String,
    file: String,
    stamp: Stamp,
    offset: u64,
    position: u64,
}

/// Same process-local random-key pattern as attempt_snapshot::Cache. No token
/// is trusted across CP restarts and no bearer credential enters a cursor.
fn mac() -> Hmac<Sha256> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let secret = KEY.get_or_init(|| {
        let mut secret = [0; 32];
        for chunk in secret.chunks_mut(8) {
            chunk.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        secret
    });
    Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts a 32-byte key")
}

fn encode(cursor: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(internal)?;
    let mut signer = mac();
    signer.update(&bytes);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes())
    ))
}
fn decode(token: &str) -> ApiResult<Cursor> {
    let invalid = || ApiError::bad_request("invalid transcript page cursor");
    if token.len() > 16 * 1024 {
        return Err(invalid());
    }
    let (payload, signature) = token.split_once('.').ok_or_else(invalid)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid())?;
    let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
    let mut verifier = mac();
    verifier.update(&bytes);
    verifier.verify_slice(&signature).map_err(|_| invalid())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if cursor.version != 1 || cursor.offset > cursor.stamp.len {
        return Err(invalid());
    }
    Ok(cursor)
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}
fn stale() -> ApiError {
    ApiError::conflict("transcript_snapshot_changed: reload without a cursor")
}
fn scan_limit() -> ApiError {
    ApiError(
        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        "transcript_page_scan_limit: no complete record within the 8 MiB page scan budget".into(),
    )
}

pub(crate) async fn get_page(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Page>> {
    LocalOnlyQuery {
        host: query.host.clone(),
    }
    .require_local("recorded transcript paging")?;
    if query.path.len() > 8192
        || query.run.as_ref().is_some_and(|r| {
            r.is_empty() || r.len() > 256 || crate::path_guard::unsafe_segment(r).is_some()
        })
    {
        return Err(ApiError::bad_request(
            "invalid transcript path or authorization run",
        ));
    }
    // Validate again on every continuation: a cursor never bypasses current
    // allowed roots, workspace registration, or path containment checks.
    super::super::runs::blocking(move || {
        let path = super::validate_local(&state, &query.path).map_err(|e| {
            if query.cursor.is_some() {
                stale()
            } else {
                e
            }
        })?;
        let scope = serde_json::to_vec(&(
            &state.global_dir,
            &state.run_store.root,
            &query.path,
            "local",
            &query.run,
        ))
        .map_err(internal)?;
        let scope = URL_SAFE_NO_PAD.encode(Sha256::digest(scope));
        let page = read_page(&path, &query, scope)?;
        if super::validate_local(&state, &query.path).map_err(|_| stale())? != path {
            return Err(stale());
        }
        Ok(page)
    })
    .await
    .map(Json)
}

fn read_page(path: &Path, query: &PageQuery, scope: String) -> ApiResult<Page> {
    let prior = query.cursor.as_deref().map(decode).transpose()?;
    if prior.as_ref().is_some_and(|p| p.scope != scope) {
        return Err(ApiError::bad_request(
            "transcript cursor belongs to another path, run or control plane",
        ));
    }
    if prior
        .as_ref()
        .is_some_and(|p| p.file != path.to_string_lossy())
    {
        return Err(stale());
    }
    let mut file = open_regular_file(path).map_err(|e| {
        if prior.is_some() {
            stale()
        } else if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("recorded transcript is not available yet")
        } else {
            ApiError::bad_request(format!("cannot open regular transcript: {e}"))
        }
    })?;
    let stamp = Stamp::of(&file.metadata().map_err(internal)?)?;
    if prior.as_ref().is_some_and(|p| p.stamp != stamp) {
        return Err(stale());
    }
    let mut cursor = prior.unwrap_or(Cursor {
        version: 1,
        scope,
        file: path.to_string_lossy().into_owned(),
        stamp: stamp.clone(),
        offset: 0,
        position: 0,
    });
    let mut signer = mac();
    signer.update(&serde_json::to_vec(&(&cursor.scope, &cursor.file, &stamp)).map_err(internal)?);
    let snapshot = URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes());
    let mut page = Page {
        version: 1,
        path: query.path.clone(),
        host_id: "local",
        run: query.run.clone(),
        snapshot,
        records: Vec::new(),
        next_cursor: None,
        has_more: false,
    };
    file.seek(SeekFrom::Start(cursor.offset))
        .map_err(internal)?;
    // Take is below the buffer: even read-ahead cannot exceed the work budget.
    let mut reader = BufReader::with_capacity(16 * 1024, file.take(MAX_SCAN_BYTES));
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_RECORDS);
    while cursor.offset < stamp.len && page.records.len() < limit {
        let before = cursor.clone();
        let line = read_line(&mut reader, stamp.len - cursor.offset).map_err(internal)?;
        if !line.complete {
            // File changes win over a limit classification. Otherwise return
            // preceding complete records, or an explicit limit error; never
            // invent an event from a partially scanned oversized line.
            check_stamp(path, reader.get_ref().get_ref(), &stamp)?;
            if page.records.is_empty() {
                return Err(scan_limit());
            }
            break;
        }
        cursor.offset += line.consumed;
        if line.blank {
            continue;
        }
        cursor.position += 1;
        let (status, event) = if line.oversized {
            ("oversized", None)
        } else {
            match serde_json::from_slice::<rupu_transcript::Event>(&line.bytes) {
                Ok(event) => (
                    "event",
                    Some(serde_json::to_value(event).map_err(internal)?),
                ),
                Err(_) => ("malformed", None),
            }
        };
        page.records.push(Record {
            position: cursor.position,
            status,
            event,
        });
        set_continuation(&mut page, &cursor)?;
        if serde_json::to_vec(&page).map_err(internal)?.len() > MAX_RESPONSE_BYTES {
            if page.records.len() == 1 {
                let record = &mut page.records[0];
                record.status = "oversized";
                record.event = None;
            } else {
                page.records.pop();
                cursor = before;
            }
            break;
        }
    }
    set_continuation(&mut page, &cursor)?;
    // Check both the handle and the name: rename/replacement must fail even
    // though the already-open descriptor still points at readable old bytes.
    check_stamp(path, reader.get_ref().get_ref(), &stamp)?;
    if serde_json::to_vec(&page).map_err(internal)?.len() > MAX_RESPONSE_BYTES {
        return Err(ApiError::bad_request(
            "transcript page envelope exceeds its byte limit",
        ));
    }
    Ok(page)
}

pub(crate) fn check_stamp(path: &Path, file: &std::fs::File, stamp: &Stamp) -> ApiResult<()> {
    if Stamp::of(&file.metadata().map_err(|_| stale())?)? != *stamp
        || Stamp::of(&fs::symlink_metadata(path).map_err(|_| stale())?)? != *stamp
    {
        return Err(stale());
    }
    Ok(())
}

fn set_continuation(page: &mut Page, cursor: &Cursor) -> ApiResult<()> {
    page.has_more = cursor.offset < cursor.stamp.len;
    page.next_cursor = page.has_more.then(|| encode(cursor)).transpose()?;
    Ok(())
}

pub(crate) struct Line {
    pub(crate) bytes: Vec<u8>,
    pub(crate) consumed: u64,
    pub(crate) blank: bool,
    pub(crate) oversized: bool,
    pub(crate) complete: bool,
}

/// Match str::trim's Unicode whitespace semantics across buffer boundaries,
/// retaining at most one incomplete UTF-8 scalar. Invalid UTF-8 is nonblank.
#[derive(Default)]
struct BlankLine {
    pending: Vec<u8>,
    nonblank: bool,
}
impl BlankLine {
    fn feed(&mut self, bytes: &[u8]) {
        if self.nonblank {
            return;
        }
        let mut joined = std::mem::take(&mut self.pending);
        joined.extend_from_slice(bytes);
        match std::str::from_utf8(&joined) {
            Ok(text) => self.nonblank = !text.trim().is_empty(),
            Err(error) => {
                let valid = std::str::from_utf8(&joined[..error.valid_up_to()])
                    .expect("validated UTF-8 prefix");
                self.nonblank = !valid.trim().is_empty() || error.error_len().is_some();
                if !self.nonblank {
                    self.pending
                        .extend_from_slice(&joined[error.valid_up_to()..]);
                }
            }
        }
    }
    fn is_blank(&self) -> bool {
        !self.nonblank && self.pending.is_empty()
    }
}

/// Drain one physical line without retaining an oversized payload. A finite
/// snapshot's final unterminated line is returned as event/malformed explicitly.
pub(crate) fn read_line(reader: &mut impl BufRead, remaining: u64) -> std::io::Result<Line> {
    let mut out = Line {
        bytes: Vec::new(),
        consumed: 0,
        blank: true,
        oversized: false,
        complete: false,
    };
    let mut blank = BlankLine::default();
    while out.consumed < remaining {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let size = available
            .len()
            .min((remaining - out.consumed).min(usize::MAX as u64) as usize);
        let slice = &available[..size];
        let end = slice.iter().position(|b| *b == b'\n').map(|i| i + 1);
        let take = end.unwrap_or(size);
        blank.feed(&slice[..take]);
        if out.bytes.len() + take <= MAX_LINE_BYTES && !out.oversized {
            out.bytes.extend_from_slice(&slice[..take]);
        } else {
            out.oversized = true;
            out.bytes.clear();
        }
        out.consumed += take as u64;
        reader.consume(take);
        if end.is_some() {
            out.complete = true;
            break;
        }
    }
    out.complete |= out.consumed == remaining;
    out.blank = blank.is_blank();
    Ok(out)
}

#[cfg(test)]
#[path = "transcript_page_tests.rs"]
mod tests;

/// Search may mint a direct, existing pager cursor only after proving the exact
/// recorded member, first RunStart, and unchanged file under the root scope.
pub(crate) fn search_locator(
    state: &AppState,
    query: &PageQuery,
    path: &Path,
    stamp: Stamp,
    offset: u64,
    position: u64,
) -> ApiResult<(String, String)> {
    let scope = serde_json::to_vec(&(
        &state.global_dir,
        &state.run_store.root,
        &query.path,
        "local",
        &query.run,
    ))
    .map_err(internal)?;
    let scope = URL_SAFE_NO_PAD.encode(Sha256::digest(scope));
    let cursor = Cursor {
        version: 1,
        scope,
        file: path.to_string_lossy().into_owned(),
        stamp,
        offset,
        position,
    };
    let mut signer = mac();
    signer.update(
        &serde_json::to_vec(&(&cursor.scope, &cursor.file, &cursor.stamp)).map_err(internal)?,
    );
    Ok((
        URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes()),
        encode(&cursor)?,
    ))
}
