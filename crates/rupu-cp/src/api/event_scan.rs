//! Bounded exact-run recorded-event scanning. This is not a recent-run catalog
//! or a globally chronological event index. Legacy /api/events is unchanged.
use super::{
    event_scan_reader as reader,
    event_scan_snapshot::{Manifest, SnapshotStats},
};
use crate::{
    error::{ApiError, ApiResult},
    query::{self, ListError, ListQuery},
    state::AppState,
};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rupu_orchestrator::executor::Event;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

pub(crate) const CAPABILITY: &str = "events.scan.v1";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const RECORD_RESPONSE_BUDGET: usize = MAX_RESPONSE_BYTES - 64 * 1024;
const SNAPSHOT_TTL: Duration = Duration::from_secs(600);
const MAX_SNAPSHOTS: usize = 128;
const MAX_OWNER_SNAPSHOTS: usize = 8;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/events/scan",
        post(scan).layer(DefaultBodyLimit::max(16 * 1024)),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanRequest {
    #[serde(default)]
    host: Option<String>,
    selection: Selection,
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
}
fn default_limit() -> usize {
    100
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Selection {
    Runs {
        run_ids: Vec<String>,
        #[serde(default)]
        workspace_id: Option<String>,
    },
}
impl ScanRequest {
    fn validate(&mut self) -> ApiResult<()> {
        if self.host.as_deref().is_some_and(|h| h != "local") {
            return Err(failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                "events_scope_unsupported",
                "Only exact local recorded runs are supported",
            ));
        }
        if !(1..=200).contains(&self.limit) || self.q.len() > 8 * 1024 {
            return Err(failure(
                StatusCode::BAD_REQUEST,
                "events_invalid_selection",
                "Limit must be 1–200 and query at most 8 KiB",
            ));
        }
        let Selection::Runs {
            run_ids,
            workspace_id,
        } = &mut self.selection;
        if run_ids.is_empty()
            || run_ids.len() > 20
            || run_ids.iter().any(|id| !valid_id(id))
            || workspace_id.as_ref().is_some_and(|id| !valid_id(id))
        {
            return Err(failure(
                StatusCode::BAD_REQUEST,
                "events_invalid_selection",
                "Select 1–20 exact run IDs and an optional exact workspace ID",
            ));
        }
        run_ids.sort();
        run_ids.dedup();
        if self
            .cursor
            .as_ref()
            .is_some_and(|c| c.is_empty() || c.len() > 4096)
        {
            return Err(invalid_cursor());
        }
        Ok(())
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug)]
pub(crate) enum ScanError {
    Api(ApiError),
    Query(ListError),
}
impl From<ApiError> for ScanError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}
impl IntoResponse for ScanError {
    fn into_response(self) -> Response {
        match self {
            Self::Query(error) => error.into_response(),
            Self::Api(ApiError(status, message)) => {
                let code = message
                    .split_once(':')
                    .map(|(code, _)| code)
                    .filter(|code| code.starts_with("events_"))
                    .unwrap_or("events_scan_failed");
                (status, Json(json!({"error": message, "code": code}))).into_response()
            }
        }
    }
}
fn failure(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError(status, format!("{code}: {message}"))
}
fn invalid_cursor() -> ApiError {
    failure(
        StatusCode::BAD_REQUEST,
        "events_invalid_cursor",
        "Cursor is malformed or not valid on this process",
    )
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(error.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    scope: String,
    snapshot: String,
    evaluation_time_ms: i64,
    run_index: usize,
    position: reader::ReverseState,
}
fn mac() -> Hmac<Sha256> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut value = [0; 32];
        for chunk in value.chunks_mut(8) {
            chunk.copy_from_slice(&ulid::Ulid::new().to_bytes()[8..]);
        }
        value
    });
    Hmac::<Sha256>::new_from_slice(key).expect("32-byte HMAC key")
}
fn encode(cursor: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(internal)?;
    let mut signer = mac();
    signer.update(&bytes);
    let encoded = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(signer.finalize().into_bytes())
    );
    if encoded.len() > 4096 {
        return Err(internal("event cursor exceeded its fixed bound"));
    }
    Ok(encoded)
}
fn decode(token: &str) -> ApiResult<Cursor> {
    if token.len() > 4096 {
        return Err(invalid_cursor());
    }
    let (payload, signature) = token.split_once('.').ok_or_else(invalid_cursor)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor())?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid_cursor())?;
    let mut signer = mac();
    signer.update(&bytes);
    signer
        .verify_slice(&signature)
        .map_err(|_| invalid_cursor())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
    if cursor.version != 1
        || cursor.snapshot.len() > 128
        || cursor.scope.len() > 128
        || cursor.run_index > 20
        || chrono::DateTime::from_timestamp_millis(cursor.evaluation_time_ms).is_none()
    {
        return Err(invalid_cursor());
    }
    Ok(cursor)
}
fn digest(value: &impl Serialize) -> ApiResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(serde_json::to_vec(value).map_err(internal)?)))
}

struct CachedManifest {
    owner: String,
    value: Arc<Manifest>,
    created: Instant,
    used: Instant,
}
#[derive(Default)]
struct SnapshotCache {
    entries: HashMap<String, CachedManifest>,
}
impl SnapshotCache {
    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, e| now.duration_since(e.created) < SNAPSHOT_TTL);
    }
    fn get(&mut self, id: &str, owner: &str) -> ApiResult<Arc<Manifest>> {
        let now = Instant::now();
        self.prune(now);
        let entry = self.entries.get_mut(id).ok_or_else(|| {
            failure(
                StatusCode::GONE,
                "events_snapshot_expired",
                "Refresh the exact run scan",
            )
        })?;
        if entry.owner != owner {
            return Err(invalid_cursor());
        }
        entry.used = now;
        Ok(Arc::clone(&entry.value))
    }
    fn insert(&mut self, id: String, owner: String, value: Arc<Manifest>) {
        let now = Instant::now();
        self.prune(now);
        while self.entries.values().filter(|e| e.owner == owner).count() >= MAX_OWNER_SNAPSHOTS {
            let key = self
                .entries
                .iter()
                .filter(|(_, e)| e.owner == owner)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            if let Some(key) = key {
                self.entries.remove(&key);
            } else {
                break;
            }
        }
        while self.entries.len() >= MAX_SNAPSHOTS {
            let key = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            if let Some(key) = key {
                self.entries.remove(&key);
            } else {
                break;
            }
        }
        self.entries.insert(
            id,
            CachedManifest {
                owner,
                value,
                created: now,
                used: now,
            },
        );
    }
}
fn cache() -> &'static Mutex<SnapshotCache> {
    static CACHE: OnceLock<Mutex<SnapshotCache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}
fn cache_lock() -> ApiResult<std::sync::MutexGuard<'static, SnapshotCache>> {
    cache()
        .lock()
        .map_err(|_| internal("event scan cache unavailable"))
}
fn jobs() -> &'static Arc<tokio::sync::Semaphore> {
    static JOBS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    JOBS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ScanPage {
    version: u8,
    host_id: &'static str,
    scope: Value,
    query: Value,
    snapshot: String,
    order: &'static str,
    limit: usize,
    records: Vec<ScanRecord>,
    next_cursor: Option<String>,
    has_more_scan: bool,
    coverage: Value,
}
#[derive(Debug, Serialize)]
struct ScanRecord {
    id: String,
    run_id: String,
    workspace_id: String,
    byte_offset: u64,
    timestamp_ms: i64,
    timestamp_source: &'static str,
    event: Value,
}
#[derive(Default, Debug, Serialize)]
struct Skipped {
    blank: u64,
    malformed: u64,
    unknown_event: u64,
    oversized: u64,
    owner_mismatch: u64,
}

async fn scan(
    State(state): State<AppState>,
    body: Result<Json<ScanRequest>, JsonRejection>,
) -> Result<Json<ScanPage>, ScanError> {
    let Json(mut request) = body.map_err(|error| {
        failure(
            error.status(),
            "events_invalid_selection",
            "Expected a bounded exact-run scan request",
        )
    })?;
    request.validate()?;
    let query = ListQuery::parse(Some(&request.q), query::events::FIELDS)
        .map_err(|e| ScanError::Query(ListError::Query(e)))?;
    // The existing router authenticates every request. This CP has a single
    // authority, not per-user principals; cursors never bypass its middleware.
    let scope = digest(&(
        &state.global_dir,
        &state.run_store.root,
        "local",
        &request.selection,
        &request.q,
        request.limit,
        "events",
        1,
    ))?;
    let owner = digest(&(&state.global_dir, &state.run_store.root, "local"))?;
    let prior = request.cursor.as_deref().map(decode).transpose()?;
    if prior.as_ref().is_some_and(|p| p.scope != scope) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "events_cursor_scope_mismatch",
            "Run set, workspace, query, page limit or control plane changed",
        )
        .into());
    }
    let permit = Arc::clone(jobs()).try_acquire_owned().map_err(|_| {
        failure(
            StatusCode::TOO_MANY_REQUESTS,
            "events_scan_busy",
            "Two event scans are already running; retry explicitly",
        )
    })?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(Arc::clone(&cancelled));
    let global = state.global_dir.clone();
    let root = state.run_store.root.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        scan_page(
            &global,
            &root,
            request,
            query.as_ref(),
            scope,
            owner,
            prior,
            &cancelled,
        )
    })
    .await
    .map_err(internal)??;
    Ok(Json(result))
}

fn own_timestamp(event: &Event) -> Option<i64> {
    match event {
        Event::RunStarted { started_at, .. } => Some(started_at.timestamp_millis()),
        Event::RunCompleted { finished_at, .. } | Event::RunFailed { finished_at, .. } => {
            Some(finished_at.timestamp_millis())
        }
        _ => None,
    }
}

// Keep owner, selection and continuation preconditions explicit at this boundary.
#[allow(clippy::too_many_arguments)]
fn scan_page(
    global: &std::path::Path,
    root: &std::path::Path,
    request: ScanRequest,
    query: Option<&ListQuery>,
    scope: String,
    owner: String,
    prior: Option<Cursor>,
    cancelled: &AtomicBool,
) -> ApiResult<ScanPage> {
    let Selection::Runs {
        run_ids,
        workspace_id,
    } = &request.selection;
    let is_new = prior.is_none();
    let manifest = if let Some(cursor) = &prior {
        cache_lock()?.get(&cursor.snapshot, &owner)?
    } else {
        Arc::new(Manifest::capture(
            global,
            root,
            run_ids,
            workspace_id.as_deref(),
        )?)
    };
    let mut snapshot_stats = if is_new {
        manifest.capture_stats.clone()
    } else {
        SnapshotStats::default()
    };
    let mut files = manifest.validate_counted(&mut snapshot_stats)?;
    let mut cursor = prior.unwrap_or_else(|| Cursor {
        version: 1,
        scope,
        snapshot: ulid::Ulid::new().to_string(),
        evaluation_time_ms: chrono::Utc::now().timestamp_millis(),
        run_index: 0,
        position: reader::ReverseState::new(
            manifest
                .runs
                .first()
                .map(|r| r.event_stamp.len)
                .unwrap_or(0),
        ),
    });
    if cursor.run_index > manifest.runs.len()
        || (cursor.run_index < manifest.runs.len()
            && cursor.position.offset > manifest.runs[cursor.run_index].event_stamp.len)
    {
        return Err(invalid_cursor());
    }
    let query_now = chrono::DateTime::from_timestamp_millis(cursor.evaluation_time_ms)
        .ok_or_else(invalid_cursor)?;
    let mut records = Vec::new();
    let mut skipped = Skipped::default();
    let mut budget = reader::ScanBudget::new();
    let mut evaluated = 0u64;
    let mut response_bytes = 0usize;
    let mut tail_bytes = 0u64;
    let mut stop_reason = "window_exhausted";
    while cursor.run_index < manifest.runs.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(failure(
                StatusCode::REQUEST_TIMEOUT,
                "events_scan_cancelled",
                "Scan cancelled",
            ));
        }
        if records.len() >= request.limit {
            stop_reason = "record_limit";
            break;
        }
        let source = &manifest.runs[cursor.run_index];
        let before = cursor.position.clone();
        let item = reader::next(
            &mut files[cursor.run_index],
            &mut cursor.position,
            &mut budget,
            cancelled,
        )
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::Interrupted {
                failure(
                    StatusCode::REQUEST_TIMEOUT,
                    "events_scan_cancelled",
                    "Scan cancelled",
                )
            } else {
                failure(
                    StatusCode::CONFLICT,
                    "events_snapshot_changed",
                    "Selected event file could not be read unchanged; refresh",
                )
            }
        })?;
        match item {
            reader::ScanItem::End => {
                cursor.run_index += 1;
                cursor.position = reader::ReverseState::new(
                    manifest
                        .runs
                        .get(cursor.run_index)
                        .map(|r| r.event_stamp.len)
                        .unwrap_or(0),
                );
            }
            reader::ScanItem::BudgetStop => {
                stop_reason = if budget.physical_lines >= reader::MAX_LINES {
                    "line_budget"
                } else {
                    "byte_budget"
                };
                break;
            }
            reader::ScanItem::Blank { .. } => skipped.blank += 1,
            reader::ScanItem::Oversized { .. } => skipped.oversized += 1,
            reader::ScanItem::UnterminatedTail { bytes, .. } => tail_bytes += bytes,
            reader::ScanItem::Record { byte_start, bytes } => {
                let event: Event = match serde_json::from_slice(&bytes) {
                    Ok(event) => event,
                    Err(_) => {
                        skipped.malformed += 1;
                        continue;
                    }
                };
                if matches!(event, Event::Unknown) {
                    skipped.unknown_event += 1;
                    continue;
                }
                let own_ts = own_timestamp(&event);
                let mut event = serde_json::to_value(event).map_err(internal)?;
                if event
                    .get("run_id")
                    .is_some_and(|id| id.as_str() != Some(source.id.as_str()))
                {
                    skipped.owner_mismatch += 1;
                    continue;
                }
                let timestamp = own_ts.unwrap_or(source.fallback_timestamp_ms);
                let object = event
                    .as_object_mut()
                    .ok_or_else(|| internal("event is not an object"))?;
                object.insert("run_id".into(), json!(source.id));
                // Pure recorded attribution, never legacy workflow/subrun enrichment.
                let mut attributed = object.clone();
                attributed.insert("ts".into(), json!(timestamp));
                attributed.insert("project".into(), json!(source.project_name));
                attributed.insert("run_codename".into(), json!(source.run_codename));
                if !source.workflow_name.is_empty() && !source.workflow_name.starts_with("agent:") {
                    attributed.insert("workflow".into(), json!(source.workflow_name));
                }
                evaluated += 1;
                if query.is_some_and(|q| {
                    !rupu_query::matches(
                        &Value::Object(attributed),
                        &q.parsed,
                        query::events::FIELDS,
                        query_now,
                    )
                    .unwrap_or(false)
                }) {
                    continue;
                }
                let record = ScanRecord {
                    id: digest(&(&cursor.snapshot, &source.id, byte_start))?,
                    run_id: source.id.clone(),
                    workspace_id: source.workspace_id.clone(),
                    byte_offset: byte_start,
                    timestamp_ms: timestamp,
                    timestamp_source: if own_ts.is_some() {
                        "event"
                    } else {
                        source.fallback_timestamp_source
                    },
                    event,
                };
                let size = serde_json::to_vec(&record).map_err(internal)?.len() + 1;
                if size > RECORD_RESPONSE_BUDGET {
                    skipped.oversized += 1;
                    continue;
                }
                if response_bytes + size > RECORD_RESPONSE_BUDGET {
                    cursor.position = before;
                    stop_reason = "response_budget";
                    break;
                }
                response_bytes += size;
                records.push(record);
            }
        }
    }
    while cursor.run_index < manifest.runs.len() && cursor.position.finished() {
        cursor.run_index += 1;
        cursor.position = reader::ReverseState::new(
            manifest
                .runs
                .get(cursor.run_index)
                .map(|r| r.event_stamp.len)
                .unwrap_or(0),
        );
    }
    let more = cursor.run_index < manifest.runs.len();
    if !more {
        stop_reason = "window_exhausted";
    }
    // Validate every selected owner and file again, even those not reached yet.
    // This is change detection, not an atomic filesystem snapshot guarantee.
    drop(manifest.validate_counted(&mut snapshot_stats)?);
    let page = ScanPage {
        version: 1,
        host_id: "local",
        scope: json!({"workspace_id": workspace_id, "run_scope": "active", "run_ids": run_ids}),
        query: json!({"q": request.q, "grammar": "events", "version": 1, "evaluation_time_ms": cursor.evaluation_time_ms}),
        snapshot: cursor.snapshot.clone(),
        order: "run_started_desc_record_offset_desc",
        limit: request.limit,
        has_more_scan: more,
        next_cursor: if more { Some(encode(&cursor)?) } else { None },
        coverage: json!({"selection": "explicit_runs", "selected_runs": run_ids.len(), "catalog": null, "window_exhausted": !more, "archive_searched": false, "remote_hosts_searched": false,
            "unavailable_runs": manifest.unavailable, "page": {"event_bytes_read": budget.bytes_read, "metadata_bytes_read": if is_new { manifest.metadata_bytes_read } else { 0 },
            "file_observations": snapshot_stats.file_observations, "root_checks": snapshot_stats.root_checks,
            "physical_lines_completed": budget.physical_lines, "records_evaluated": evaluated, "matches_returned": records.len(), "stop_reason": stop_reason,
            "skipped": skipped, "pending_oversized_record": more && cursor.position.pending_oversized(), "unterminated_tail_bytes": tail_bytes},
            "limitations": ["active_global_run_store_only", "no_subrun_codename_derivation", "no_findings_ledger", "not_global_event_time_order", "append_invalidates_snapshot"]}),
        records,
    };
    if serde_json::to_vec(&page).map_err(internal)?.len() > MAX_RESPONSE_BYTES {
        return Err(internal("bounded event response exceeded envelope reserve"));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(failure(
            StatusCode::REQUEST_TIMEOUT,
            "events_scan_cancelled",
            "Scan cancelled",
        ));
    }
    if is_new && more {
        cache_lock()?.insert(cursor.snapshot, owner, manifest);
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, path::Path};

    fn request(ids: &[&str], q: &str, limit: usize) -> ScanRequest {
        ScanRequest {
            host: Some("local".into()),
            selection: Selection::Runs {
                run_ids: ids.iter().map(|s| s.to_string()).collect(),
                workspace_id: None,
            },
            q: q.into(),
            limit,
            cursor: None,
        }
    }
    fn seed(root: &Path, id: &str, events: &[Value]) {
        let dir = root.join("runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("run.json"), serde_json::to_vec(&json!({"id":id, "workspace_id":"ws_a", "workflow_name":"scan", "workspace_path":"/projects/same-name", "started_at":"2026-11-09T00:00:00Z"})).unwrap()).unwrap();
        let mut file = fs::File::create(dir.join("events.jsonl")).unwrap();
        for event in events {
            serde_json::to_writer(&mut file, event).unwrap();
            file.write_all(b"\n").unwrap();
        }
    }
    fn event(run: &str, step: &str, message: &str) -> Value {
        json!({"type":"step_failed", "run_id":run, "step_id":step, "error":message})
    }
    fn bindings(root: &Path, request: &ScanRequest) -> (String, String) {
        (
            digest(&(
                root,
                root.join("runs"),
                "local",
                &request.selection,
                &request.q,
                request.limit,
                "events",
                1,
            ))
            .unwrap(),
            digest(&(root, root.join("runs"), "local")).unwrap(),
        )
    }
    fn page(root: &Path, mut request: ScanRequest) -> ApiResult<ScanPage> {
        request.validate()?;
        let query = ListQuery::parse(Some(&request.q), query::events::FIELDS).unwrap();
        let (scope, owner) = bindings(root, &request);
        let prior = request.cursor.as_deref().map(decode).transpose()?;
        if prior.as_ref().is_some_and(|p| p.scope != scope) {
            return Err(failure(
                StatusCode::BAD_REQUEST,
                "events_cursor_scope_mismatch",
                "scope changed",
            ));
        }
        scan_page(
            root,
            &root.join("runs"),
            request,
            query.as_ref(),
            scope,
            owner,
            prior,
            &AtomicBool::new(false),
        )
    }
    #[test]
    fn strict_selection_limits_and_no_recent_or_remote_fallback() {
        for body in [
            r#"{"selection":{"kind":"recent"}}"#,
            r#"{"selection":{"kind":"runs","run_ids":["run_a"],"surprise":true}}"#,
            r#"{"selection":{"kind":"runs","run_ids":["run_a"]},"limit":1,"limit":2}"#,
        ] {
            assert!(serde_json::from_str::<ScanRequest>(body).is_err());
        }
        let mut req = request(&["run_b", "run_a", "run_a"], "", 1);
        req.validate().unwrap();
        let Selection::Runs { run_ids, .. } = req.selection;
        assert_eq!(run_ids, ["run_a", "run_b"]);
        for limit in [0, 201, usize::MAX] {
            assert!(request(&["run_a"], "", limit).validate().is_err());
        }
        assert!(request(&["../escape"], "", 1).validate().is_err());
        assert!(request(&["run_a"], &"x".repeat(8193), 1)
            .validate()
            .is_err());
        let mut remote = request(&["run_a"], "", 1);
        remote.host = Some("worker".into());
        assert_eq!(
            remote.validate().unwrap_err().0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    #[test]
    fn exact_cursor_replay_and_byte_offset_paging_do_not_drop_records() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "run_a",
            &[
                event("run_a", "first", "one"),
                event("run_a", "second", "two"),
                event("run_a", "third", "three"),
            ],
        );
        let mut req = request(&["run_a"], "", 1);
        let first = page(tmp.path(), req.clone()).unwrap();
        assert_eq!(first.records[0].event["step_id"], "third");
        req.cursor = first.next_cursor;
        let second = page(tmp.path(), req.clone()).unwrap();
        let replay = page(tmp.path(), req.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&second).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
        assert_eq!(second.records[0].event["step_id"], "second");
        assert!(second.records[0].byte_offset < first.records[0].byte_offset);
        req.cursor = second.next_cursor;
        let third = page(tmp.path(), req).unwrap();
        assert_eq!(third.records[0].event["step_id"], "first");
        assert!(!third.has_more_scan && third.next_cursor.is_none());
    }
    #[test]
    fn cursor_rejects_tampering_query_limit_set_workspace_and_root_changes() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "run_a",
            &[event("run_a", "a", "a"), event("run_a", "b", "b")],
        );
        let req = request(&["run_a"], "", 1);
        let first = page(tmp.path(), req.clone()).unwrap();
        let token = first.next_cursor.unwrap();
        assert!(decode(&(token.clone() + "x")).is_err());
        assert!(decode(&"x".repeat(4097)).is_err());
        for variant in 0..4 {
            let mut changed = req.clone();
            changed.cursor = Some(token.clone());
            match variant {
                0 => changed.q = "group:error".into(),
                1 => changed.limit = 2,
                2 => {
                    let Selection::Runs { run_ids, .. } = &mut changed.selection;
                    run_ids.push("run_b".into());
                }
                _ => {
                    let Selection::Runs { workspace_id, .. } = &mut changed.selection;
                    *workspace_id = Some("ws_b".into());
                }
            }
            let error = page(tmp.path(), changed).unwrap_err();
            assert!(error.1.starts_with("events_cursor_scope_mismatch"));
        }
        let other = tempfile::tempdir().unwrap();
        let mut wrong = req;
        wrong.cursor = Some(token);
        assert!(page(other.path(), wrong)
            .unwrap_err()
            .1
            .starts_with("events_cursor_scope_mismatch"));
    }
    #[test]
    fn sparse_giant_record_returns_advancing_empty_scan_page() {
        let tmp = tempfile::tempdir().unwrap();
        seed(tmp.path(), "run_a", &[event("run_a", "old", "match")]);
        let path = tmp.path().join("runs/run_a/events.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(&vec![b'x'; reader::MAX_SCAN_BYTES as usize + 64 * 1024])
            .unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);
        let mut req = request(&["run_a"], "message:match", 10);
        let first = page(tmp.path(), req.clone()).unwrap();
        assert!(first.records.is_empty() && first.has_more_scan);
        assert!(
            first.coverage["page"]["event_bytes_read"].as_u64().unwrap() <= reader::MAX_SCAN_BYTES
        );
        assert_eq!(first.coverage["page"]["pending_oversized_record"], true);
        req.cursor = first.next_cursor;
        let second = page(tmp.path(), req).unwrap();
        assert_eq!(second.records.len(), 1);
        assert_eq!(second.records[0].event["step_id"], "old");
        assert_eq!(second.coverage["page"]["skipped"]["oversized"], 1);
        assert!(!second.has_more_scan);
    }
    #[test]
    fn response_budget_defers_unreturned_match_without_losing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "x".repeat(245_000);
        seed(
            tmp.path(),
            "run_a",
            &(0..6)
                .map(|i| event("run_a", &format!("step_{i}"), &text))
                .collect::<Vec<_>>(),
        );
        let mut req = request(&["run_a"], "", 200);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..3 {
            let result = page(tmp.path(), req.clone()).unwrap();
            assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_RESPONSE_BYTES);
            for row in &result.records {
                assert!(seen.insert(row.byte_offset));
            }
            if result.next_cursor.is_none() {
                break;
            }
            assert_eq!(result.coverage["page"]["stop_reason"], "response_budget");
            req.cursor = result.next_cursor;
        }
        assert_eq!(seen.len(), 6);
    }
    #[test]
    fn source_owner_overrides_no_embedded_run_and_rejects_wrong_embedded_owner() {
        let tmp = tempfile::tempdir().unwrap();
        seed(
            tmp.path(),
            "run_a",
            &[
                event("run_other", "wrong", "bad"),
                event("run_a", "real", "ok"),
                json!({"type":"unknown_future_event","run_id":"run_a"}),
            ],
        );
        let result = page(tmp.path(), request(&["run_a"], "", 10)).unwrap();
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].run_id, "run_a");
        assert_eq!(result.coverage["page"]["skipped"]["owner_mismatch"], 1);
        assert_eq!(result.coverage["page"]["skipped"]["unknown_event"], 1);
    }
    #[test]
    fn changes_to_selected_not_yet_scanned_file_invalidate_continuation() {
        let tmp = tempfile::tempdir().unwrap();
        seed(tmp.path(), "run_a", &[event("run_a", "a", "a")]);
        seed(tmp.path(), "run_z", &[event("run_z", "z", "z")]);
        let mut req = request(&["run_a", "run_z"], "", 1);
        let first = page(tmp.path(), req.clone()).unwrap();
        assert_eq!(first.records[0].run_id, "run_z");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(tmp.path().join("runs/run_a/events.jsonl"))
            .unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);
        req.cursor = first.next_cursor;
        let error = page(tmp.path(), req).unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert!(error.1.starts_with("events_snapshot_changed"));
    }
    #[test]
    fn relative_time_query_uses_signed_snapshot_time() {
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let event_at = now - chrono::Duration::minutes(30);
        let events = [
            json!({"type":"run_started","run_id":"run_a","event_version":1,"workflow_path":"/recorded/scan.yaml","started_at":event_at.to_rfc3339()}),
            event("run_a", "latest", "a"),
        ];
        seed(tmp.path(), "run_a", &events);
        let mut req = request(&["run_a"], "since:1h", 1);
        let first = page(tmp.path(), req.clone()).unwrap();
        let mut cursor = decode(first.next_cursor.as_deref().unwrap()).unwrap();
        let frozen = chrono::DateTime::from_timestamp_millis(cursor.evaluation_time_ms).unwrap();
        let query = ListQuery::parse(Some(&req.q), query::events::FIELDS)
            .unwrap()
            .unwrap();
        let row = json!({"type":"run_started", "ts":event_at.timestamp_millis()});
        assert!(rupu_query::matches(&row, &query.parsed, query::events::FIELDS, frozen).unwrap());
        assert!(!rupu_query::matches(
            &row,
            &query.parsed,
            query::events::FIELDS,
            frozen + chrono::Duration::hours(2)
        )
        .unwrap());
        req.cursor = Some(encode(&cursor).unwrap());
        let result = page(tmp.path(), req.clone()).unwrap();
        assert_eq!(result.records.len(), 1);
        let replay = page(tmp.path(), req.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
        cursor.evaluation_time_ms += 2 * 60 * 60 * 1000;
        req.cursor = Some(encode(&cursor).unwrap());
        assert!(page(tmp.path(), req).unwrap().records.is_empty());
    }
    #[test]
    fn cache_is_bounded_per_owner_globally_and_expires() {
        let tmp = tempfile::tempdir().unwrap();
        seed(tmp.path(), "run_a", &[]);
        let manifest = Arc::new(
            Manifest::capture(
                tmp.path(),
                &tmp.path().join("runs"),
                &["run_a".into()],
                None,
            )
            .unwrap(),
        );
        let mut cache = SnapshotCache::default();
        for i in 0..20 {
            cache.insert(format!("same_{i}"), "one".into(), Arc::clone(&manifest));
        }
        assert_eq!(cache.entries.len(), MAX_OWNER_SNAPSHOTS);
        for i in 0..150 {
            cache.insert(
                format!("id_{i}"),
                format!("owner_{i}"),
                Arc::clone(&manifest),
            );
        }
        assert_eq!(cache.entries.len(), MAX_SNAPSHOTS);
        cache.insert("expired".into(), "expired_owner".into(), manifest);
        cache.entries.get_mut("expired").unwrap().created =
            Instant::now() - SNAPSHOT_TTL - Duration::from_secs(1);
        assert_eq!(
            cache.get("expired", "expired_owner").unwrap_err().0,
            StatusCode::GONE
        );
    }
    #[tokio::test]
    async fn route_preserves_query_diagnostics_and_bounds_json_body() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let app = routes().with_state(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/scan")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&request(&["run_missing"], "😀 bogus:x", 1)).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
        assert_eq!(body["code"], "unknown_key");
        assert!(body["start"].is_number() && body["end"].is_number());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/events/scan")
                    .header("content-type", "application/json")
                    .body(Body::from(" ".repeat(16 * 1024 + 1)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
