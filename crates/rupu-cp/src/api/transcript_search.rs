//! Finite run-family search. Captured membership, bounded scan, direct locators.
use super::pages::{capture::search as membership, Owner};
use crate::{
    api::{
        fs_open::open_anchored_regular_file,
        transcript::{
            self,
            page::{self, PageQuery, Stamp},
        },
    },
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

pub(crate) const CAPABILITY: &str = "run.transcript_search.v1";
pub(crate) const AGENTIFLOW_CAPABILITY: &str = "agentiflow.transcript_search.v1";
const SCAN_BYTES: u64 = 8 * 1024 * 1024;
const RESPONSE_BYTES: usize = 1024 * 1024;
const RECORDS: usize = 10_000;
const TTL: Duration = Duration::from_secs(600);
#[derive(Clone, Deserialize, schemars::JsonSchema)]
pub(crate) struct SearchQuery {
    /// Exact owning workspace ID. Required, never inferred from selection.
    pub project: String,
    /// Literal case-sensitive string-value match, 1..256 UTF-8 bytes.
    pub q: String,
    pub host: Option<String>,
    /// Default 20, range 1..50.
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Coverage {
    members_total: usize,
    members_completed: usize,
    bytes_scanned: u64,
    records_scanned: u64,
    malformed_records: u64,
    oversized_records: u64,
    unavailable_transcripts: usize,
    unverified_transcripts: usize,
    remote_members: usize,
    membership_malformed_rows: usize,
    membership_incomplete_tail_bytes: usize,
    history_complete: String,
    consistency: String,
}
#[derive(Serialize)]
pub(crate) struct SearchPage {
    schema_version: u32,
    owner_host_id: &'static str,
    root_run_id: String,
    root_type: &'static str,
    project_id: String,
    query: String,
    snapshot: String,
    hits: Vec<Hit>,
    next_cursor: Option<String>,
    has_more: bool,
    coverage: Coverage,
}
#[derive(Serialize)]
struct Hit {
    run_id: String,
    lead_round: Option<u32>,
    authorization_run_id: String,
    transcript_path: String,
    provenance: Vec<Edge>,
    position: u64,
    excerpt: String,
    first_run_start: serde_json::Value,
    page_cursor: Locator,
}
#[derive(Serialize)]
struct Edge {
    parent_run_id: String,
    run_id: String,
    source: &'static str,
}
#[derive(Serialize)]
struct Locator {
    snapshot: String,
    token: String,
    position: u64,
}
struct Snapshot {
    id: String,
    owner: Owner,
    root: String,
    project: String,
    query: String,
    scope: Scope,
    captured: membership::Captured,
    files: Vec<Option<(PathBuf, Stamp)>>,
    created: Instant,
    weight: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct Cursor {
    snapshot: String,
    member: usize,
    offset: u64,
    position: u64,
    coverage: Coverage,
}
#[derive(Default)]
struct Cache {
    rows: VecDeque<Arc<Snapshot>>,
}
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static BUSY: AtomicBool = AtomicBool::new(false);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
struct Cancellation(Arc<AtomicBool>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(Default::default)
}
fn stale(reason: &str) -> ApiError {
    ApiError::conflict(format!(
        "transcript_search_changed: {reason}; restart without cursor"
    ))
}
fn large(reason: &str) -> ApiError {
    ApiError(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("transcript_search_limit: {reason}"),
    )
}
fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(e.to_string())
}
fn mac() -> Hmac<Sha256> {
    static SECRET: OnceLock<[u8; 32]> = OnceLock::new();
    let key = SECRET.get_or_init(|| {
        let mut key = [0; 32];
        for p in key.chunks_mut(16) {
            p.copy_from_slice(&ulid::Ulid::new().to_bytes());
        }
        key
    });
    Hmac::<Sha256>::new_from_slice(key).expect("MAC key")
}
fn encode(cursor: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(internal)?;
    let mut sign = mac();
    sign.update(&bytes);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(sign.finalize().into_bytes())
    ))
}
fn decode(token: &str) -> ApiResult<Cursor> {
    let invalid = || ApiError::bad_request("invalid transcript search cursor");
    if token.len() > 8192 {
        return Err(invalid());
    }
    let (body, sig) = token.split_once('.').ok_or_else(invalid)?;
    let body = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
    let sig = URL_SAFE_NO_PAD.decode(sig).map_err(|_| invalid())?;
    let mut sign = mac();
    sign.update(&body);
    sign.verify_slice(&sig).map_err(|_| invalid())?;
    serde_json::from_slice(&body).map_err(|_| invalid())
}
fn owner(state: &AppState) -> ApiResult<Owner> {
    let global = state.global_dir.canonicalize().map_err(internal)?;
    let runs = if state.run_store.root.exists() {
        state.run_store.root.canonicalize().map_err(internal)?
    } else {
        global.join("runs")
    };
    Ok(Owner { global, runs })
}
#[derive(Clone, PartialEq, Eq)]
struct Scope {
    roots: Vec<PathBuf>,
    registry: Option<Stamp>,
    identities: Vec<RootIdentity>,
}
#[derive(Clone, PartialEq, Eq)]
struct RootIdentity {
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
fn root_identity(path: &std::path::Path) -> ApiResult<RootIdentity> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| stale("allowed root disappeared"))?;
    if !metadata.is_dir() {
        return Err(stale("allowed root is not a directory"));
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(RootIdentity {
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}
fn scope(state: &AppState, project: &str) -> ApiResult<Scope> {
    let global = state.global_dir.canonicalize().map_err(internal)?;
    let path = global.join("workspaces").join(format!("{project}.toml"));
    let mut roots = vec![global];
    let registry = match open_anchored_regular_file(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(stale("project registration unavailable")),
        Ok(mut file) => {
            let stamp = Stamp::of(&file.metadata().map_err(internal)?)?;
            if stamp.len > 65536 {
                return Err(large("project registration exceeds 64 KiB"));
            }
            let mut text = String::new();
            file.by_ref()
                .take(65537)
                .read_to_string(&mut text)
                .map_err(internal)?;
            if text.len() > 65536 {
                return Err(large("project registration exceeds 64 KiB"));
            }
            page::check_stamp(&path, &file, &stamp)?;
            #[derive(Deserialize)]
            struct Project {
                id: String,
                path: String,
            }
            let record: Project =
                toml::from_str(&text).map_err(|_| stale("invalid project registration"))?;
            if record.id != project {
                return Err(stale("project registration identity mismatch"));
            }
            roots.push(
                PathBuf::from(record.path)
                    .canonicalize()
                    .map_err(|_| stale("registered project directory unavailable"))?,
            );
            Some(stamp)
        }
    };
    let identities = roots
        .iter()
        .map(|p| root_identity(p))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Scope {
        roots,
        registry,
        identities,
    })
}
fn resolve(state: &AppState, scope: &Scope, raw: &std::path::Path) -> ApiResult<PathBuf> {
    let path = transcript::validate_transcript_path(&raw.to_string_lossy(), &scope.roots)
        .map_err(ApiError::bad_request)?;
    if !path.exists() {
        // Archived runs retain their original recorded locators. Mapping this
        // exact prefix does not authorize a hit: member+project RunStart proof
        // and the captured file stamp are still mandatory.
        if let Ok(relative) = path.strip_prefix(&state.run_store.root) {
            let archived = state.run_store.archive_root().join(relative);
            if archived.exists() {
                return transcript::validate_transcript_path(
                    &archived.to_string_lossy(),
                    &scope.roots,
                )
                .map_err(ApiError::bad_request);
            }
        }
    }
    Ok(path)
}
fn validate(root: &str, q: &SearchQuery) -> ApiResult<()> {
    if root.is_empty()
        || root.len() > 256
        || !root
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || q.project.is_empty()
        || q.project.len() > 256
        || !q
            .project
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ApiError::bad_request("invalid search owner"));
    }
    if q.q.is_empty() || q.q.len() > 256 || q.limit.is_some_and(|v| !(1..=50).contains(&v)) {
        return Err(ApiError::bad_request(
            "search needs 1..256 query bytes and limit 1..50",
        ));
    }
    if q.host.as_deref().is_some_and(|h| h != "local") {
        return Err(ApiError::not_available(
            "run family search requires a direct local owner",
        ));
    }
    Ok(())
}
pub(crate) async fn get(
    State(state): State<AppState>,
    Path(root): Path<String>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchPage>> {
    validate(&root, &q)?;
    BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "transcript search busy; retry later".into(),
            )
        })?;
    let permit = Permit;
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = Cancellation(cancel.clone());
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        search(&state, &root, &q, &cancel)
    })
    .await
    .map_err(internal)?;
    drop(guard);
    result.map(Json)
}
fn search(
    state: &AppState,
    root: &str,
    q: &SearchQuery,
    cancel: &AtomicBool,
) -> ApiResult<SearchPage> {
    validate(root, q)?;
    membership::cancelled(cancel)?;
    let current_scope = scope(state, &q.project)?;
    let current_owner = owner(state)?;
    let (snapshot, cursor) = if let Some(token) = &q.cursor {
        let cursor = decode(token)?;
        let cache = cache()
            .lock()
            .map_err(|_| internal("search cache unavailable"))?;
        let s = cache
            .rows
            .iter()
            .find(|s| s.id == cursor.snapshot)
            .cloned()
            .ok_or_else(|| stale("capture expired or evicted"))?;
        if s.created.elapsed() >= TTL {
            return Err(stale("capture expired"));
        }
        if s.owner != current_owner || s.root != root || s.project != q.project || s.query != q.q {
            return Err(ApiError::bad_request(
                "search cursor belongs to another root, owner, project or query",
            ));
        }
        if s.scope != current_scope {
            return Err(stale("project registration scope changed"));
        }
        (s, cursor)
    } else {
        let captured = membership::capture(current_owner.clone(), root, cancel)?;
        if captured
            .proof
            .project_id
            .as_ref()
            .is_some_and(|id| id != &q.project)
        {
            return Err(ApiError::conflict("search root belongs to another project"));
        }
        let mut files = Vec::new();
        for member in &captured.members {
            membership::cancelled(cancel)?;
            let file = match &member.path {
                None => None,
                Some(raw) => {
                    let path = resolve(state, &current_scope, raw)?;
                    match open_anchored_regular_file(&path) {
                        Ok(file) => {
                            let stamp = Stamp::of(&file.metadata().map_err(internal)?)?;
                            page::check_stamp(&path, &file, &stamp)?;
                            Some((path, stamp))
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                        Err(e) => {
                            return Err(ApiError::bad_request(format!(
                                "cannot open recorded search member: {e}"
                            )))
                        }
                    }
                }
            };
            files.push(file);
        }
        if captured.proof.project_id.is_none() {
            let (path, stamp) = files
                .first()
                .and_then(Option::as_ref)
                .ok_or_else(|| stale("root project proof unavailable"))?;
            let mut file = open_anchored_regular_file(path).map_err(internal)?;
            if prologue(&mut file, path, stamp, root, &q.project)?
                .0
                .is_none()
            {
                return Err(stale(
                    "root first RunStart does not prove requested project",
                ));
            }
        }
        let weight = serde_json::to_vec(&captured.members)
            .map_err(internal)?
            .len()
            + files
                .iter()
                .map(|f| f.as_ref().map_or(0, |(path, _)| path.as_os_str().len()))
                .sum::<usize>()
            + files.len() * 1024
            + 65536;
        if weight > 16 * 1024 * 1024 {
            return Err(large("captured membership exceeds 16 MiB"));
        }
        let coverage = Coverage {
            members_total: captured.members.len(),
            remote_members: captured.remote,
            membership_malformed_rows: captured.malformed,
            membership_incomplete_tail_bytes: captured.incomplete,
            history_complete: "unknown".into(),
            consistency: "captured_observation".into(),
            ..Default::default()
        };
        let s = Arc::new(Snapshot {
            id: ulid::Ulid::new().to_string(),
            owner: current_owner,
            root: root.into(),
            project: q.project.clone(),
            query: q.q.clone(),
            scope: current_scope,
            captured,
            files,
            created: Instant::now(),
            weight,
        });
        let cursor = Cursor {
            snapshot: s.id.clone(),
            member: 0,
            offset: 0,
            position: 0,
            coverage,
        };
        let mut cache = cache()
            .lock()
            .map_err(|_| internal("search cache unavailable"))?;
        cache.rows.retain(|s| s.created.elapsed() < TTL);
        while cache.rows.len() >= 16
            || cache.rows.iter().map(|s| s.weight).sum::<usize>() + s.weight > 64 * 1024 * 1024
        {
            cache.rows.pop_front();
        }
        cache.rows.push_back(s.clone());
        (s, cursor)
    };
    snapshot.captured.proof.validate_search()?;
    if snapshot.captured.proof.project_id.is_none() {
        let (path, stamp) = snapshot.files[0]
            .as_ref()
            .ok_or_else(|| stale("root project proof missing"))?;
        let file =
            open_anchored_regular_file(path).map_err(|_| stale("root project proof moved"))?;
        page::check_stamp(path, &file, stamp)?;
    }
    let result = scan(state, &snapshot, cursor, q.limit.unwrap_or(20), cancel)?;
    snapshot.captured.proof.validate_search()?;
    if scope(state, &q.project)? != snapshot.scope {
        return Err(stale("project registration changed during search"));
    }
    Ok(result)
}

// Only the FIRST physical nonblank record can establish ownership. A later
// run_start never repairs a malformed or oversized prologue.
fn prologue(
    file: &mut File,
    path: &std::path::Path,
    stamp: &Stamp,
    run: &str,
    project: &str,
) -> ApiResult<(Option<serde_json::Value>, u64)> {
    page::check_stamp(path, file, stamp)?;
    file.seek(SeekFrom::Start(0)).map_err(internal)?;
    let mut reader = BufReader::with_capacity(4096, file.take(256 * 1024));
    let mut consumed = 0;
    let mut proof = None;
    while consumed < stamp.len && consumed < 256 * 1024 {
        let line = page::read_line(&mut reader, stamp.len - consumed).map_err(internal)?;
        consumed += line.consumed;
        if !line.complete || line.oversized {
            break;
        }
        if line.blank {
            continue;
        }
        if let Ok(event @ rupu_transcript::Event::RunStart { .. }) =
            serde_json::from_slice::<rupu_transcript::Event>(&line.bytes)
        {
            if let rupu_transcript::Event::RunStart {
                ref run_id,
                ref workspace_id,
                ..
            } = event
            {
                let raw: serde_json::Value = serde_json::from_slice(&line.bytes).map_err(internal)?;
                let aliases_match = (raw["run_id"].is_null() || raw["run_id"].as_str() == Some(run))
                    && (raw["workspace_id"].is_null() || raw["workspace_id"].as_str() == Some(project))
                    && [&raw["host_id"], &raw["data"]["host_id"]].iter()
                        .all(|host| host.is_null() || host.as_str() == Some("local"));
                if run_id == run && workspace_id == project && aliases_match {
                    proof = Some(serde_json::to_value(event).map_err(internal)?);
                }
            }
        }
        break;
    }
    page::check_stamp(path, reader.get_ref().get_ref(), stamp)?;
    // Account for buffer read-ahead as well as consumed records.
    let bytes = 256 * 1024 - reader.get_ref().limit();
    Ok((proof, bytes))
}
fn match_excerpt(value: &serde_json::Value, q: &str) -> Option<String> {
    match value {
        serde_json::Value::String(text) => text.find(q).map(|at| {
            let start = text[..at]
                .char_indices()
                .rev()
                .nth(80)
                .map_or(0, |(i, _)| i);
            let excerpt: String = text[start..].chars().take(320).collect();
            format!(
                "{}{}{}",
                if start > 0 { "…" } else { "" },
                excerpt,
                if start + excerpt.len() < text.len() {
                    "…"
                } else {
                    ""
                }
            )
        }),
        serde_json::Value::Array(rows) => rows.iter().find_map(|v| match_excerpt(v, q)),
        serde_json::Value::Object(rows) => rows.values().find_map(|v| match_excerpt(v, q)),
        _ => None,
    }
}
fn provenance(s: &Snapshot, mut index: usize) -> Vec<Edge> {
    let mut rows = Vec::new();
    while let Some(parent) = s.captured.members[index].parent {
        let row = &s.captured.members[index];
        rows.push(Edge {
            parent_run_id: s.captured.members[parent].run_id.clone(),
            run_id: row.run_id.clone(),
            source: row.source,
        });
        index = parent;
    }
    rows.reverse();
    rows
}
fn finish(s: &Snapshot, c: &Cursor, hits: Vec<Hit>) -> ApiResult<SearchPage> {
    let has_more = c.member < s.files.len();
    Ok(SearchPage {
        schema_version: 1,
        owner_host_id: "local",
        root_run_id: s.root.clone(),
        root_type: s.captured.proof.kind,
        project_id: s.project.clone(),
        query: s.query.clone(),
        snapshot: s.id.clone(),
        hits,
        next_cursor: has_more.then(|| encode(c)).transpose()?,
        has_more,
        coverage: c.coverage.clone(),
    })
}
fn advance(c: &mut Cursor) {
    c.member += 1;
    c.offset = 0;
    c.position = 0;
    c.coverage.members_completed = c.member;
}
fn scan(
    state: &AppState,
    s: &Snapshot,
    mut c: Cursor,
    limit: usize,
    cancel: &AtomicBool,
) -> ApiResult<SearchPage> {
    if c.member > s.files.len() {
        return Err(ApiError::bad_request("invalid search member position"));
    }
    let mut hits = Vec::new();
    let mut bytes = 0;
    let mut records = 0;
    'members: while c.member < s.files.len()
        && bytes < SCAN_BYTES
        && records < RECORDS
        && hits.len() < limit
    {
        membership::cancelled(cancel)?;
        let Some((path, stamp)) = &s.files[c.member] else {
            c.coverage.unavailable_transcripts += 1;
            advance(&mut c);
            continue;
        };
        let member = &s.captured.members[c.member];
        if resolve(state, &s.scope, path).map_err(|_| stale("member scope changed"))? != *path {
            return Err(stale("member locator changed"));
        }
        if c.offset > stamp.len {
            return Err(stale("invalid member byte offset"));
        }
        if bytes + 256 * 1024 > SCAN_BYTES {
            break;
        }
        let mut file =
            open_anchored_regular_file(path).map_err(|_| stale("member disappeared or moved"))?;
        let (proof, read) = prologue(&mut file, path, stamp, &member.run_id, &s.project)?;
        bytes += read;
        c.coverage.bytes_scanned += read;
        let Some(proof) = proof else {
            c.coverage.unverified_transcripts += 1;
            advance(&mut c);
            continue;
        };
        file.seek(SeekFrom::Start(c.offset)).map_err(internal)?;
        let allowance = SCAN_BYTES - bytes;
        let mut reader = BufReader::with_capacity(4096, file.take(allowance));
        while c.offset < stamp.len && records < RECORDS && hits.len() < limit {
            membership::cancelled(cancel)?;
            let previous = c.clone();
            let line = page::read_line(&mut reader, stamp.len - c.offset).map_err(internal)?;
            if !line.complete {
                page::check_stamp(path, reader.get_ref().get_ref(), stamp)?;
                if c.offset == previous.offset
                    && allowance - reader.get_ref().limit() == allowance
                    && records == 0
                {
                    return Err(large("no complete record within scan budget"));
                }
                // At a partial boundary retry the same record with a fresh budget.
                break;
            }
            c.offset += line.consumed;
            records += 1;
            if line.blank {
                continue;
            }
            c.position += 1;
            c.coverage.records_scanned += 1;
            if line.oversized {
                c.coverage.oversized_records += 1;
                continue;
            }
            match serde_json::from_slice::<rupu_transcript::Event>(&line.bytes) {
                Err(_) => c.coverage.malformed_records += 1,
                Ok(event) => {
                    let event = serde_json::to_value(event).map_err(internal)?;
                    if let Some(excerpt) = match_excerpt(&event, &s.query) {
                        let query = PageQuery {
                            path: path.to_string_lossy().into_owned(),
                            host: Some("local".into()),
                            run: Some(s.root.clone()),
                            ..Default::default()
                        };
                        let (snapshot, token) = page::search_locator(
                            state,
                            &query,
                            path,
                            stamp.clone(),
                            previous.offset,
                            previous.position,
                        )?;
                        let hit = Hit {
                            run_id: member.run_id.clone(),
                            lead_round: member.lead_round,
                            authorization_run_id: s.root.clone(),
                            transcript_path: query.path,
                            provenance: provenance(s, c.member),
                            position: c.position,
                            excerpt,
                            first_run_start: proof.clone(),
                            page_cursor: Locator {
                                snapshot,
                                token,
                                position: c.position,
                            },
                        };
                        hits.push(hit);
                        // Reserve enough for the final coverage and cursor. Do
                        // not consume a hit that must move to the following page.
                        if serde_json::to_vec(&hits).map_err(internal)?.len()
                            > RESPONSE_BYTES - 16384
                        {
                            hits.pop();
                            c = previous;
                            if hits.is_empty() {
                                return Err(large(
                                    "one hit proof and provenance exceeds response budget",
                                ));
                            }
                            let read = allowance - reader.get_ref().limit();
                            c.coverage.bytes_scanned += read;
                            page::check_stamp(path, reader.get_ref().get_ref(), stamp)?;
                            break 'members;
                        }
                    }
                }
            }
        }
        let read = allowance - reader.get_ref().limit();
        bytes += read;
        c.coverage.bytes_scanned += read;
        page::check_stamp(path, reader.get_ref().get_ref(), stamp)?;
        if c.offset == stamp.len {
            advance(&mut c);
        } else {
            break;
        }
    }
    let result = finish(s, &c, hits)?;
    if serde_json::to_vec(&result).map_err(internal)?.len() > RESPONSE_BYTES {
        return Err(large("response envelope exceeds 1 MiB"));
    }
    Ok(result)
}

#[cfg(test)]
#[path = "transcript_search_tests.rs"]
mod tests;
