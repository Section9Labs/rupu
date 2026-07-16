use crate::{
    api::run_resolve::{resolve_run_location, RunLocation},
    error::{ApiError, ApiResult},
    host::connector::{HostConnectorError, RunKind, RunListQuery},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse as _, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::future::join_all;
use rupu_orchestrator::{
    runs::{CancelError, CancelOutcome, PauseError, RunStore},
    ApprovalError, RunRecord, RunStatus, RunStoreError,
};
use std::path::PathBuf;
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/workflows", get(list_workflow_runs))
        .route("/api/runs/archived", get(list_archived_runs))
        .route("/api/runs/:id", get(get_run).delete(delete_run))
        .route("/api/runs/:id/log", get(get_run_log))
        .route("/api/runs/:id/usage-timeline", get(get_run_usage_timeline))
        .route("/api/runs/:id/autoflow", get(get_run_autoflow))
        .route("/api/runs/:id/approve", post(approve_run))
        .route("/api/runs/:id/reject", post(reject_run))
        .route("/api/runs/:id/cancel", post(cancel_run))
        .route("/api/runs/:id/pause", post(pause_run))
        .route("/api/runs/:id/resume", post(resume_run))
        .route("/api/runs/:id/archive", post(archive_run))
        .route("/api/runs/:id/restore", post(restore_run))
}

/// Map an [`ApprovalError`] from the store's approve/reject flow to an
/// [`ApiError`]:
/// - `NotFound` → 404
/// - `NotAwaiting` / `Expired` / `NoAwaitingStep` → 409 (the run isn't in a
///   state where the decision can be recorded)
/// - everything else → 500
fn map_approval_err(id: &str, e: ApprovalError) -> ApiError {
    match e {
        ApprovalError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        ApprovalError::NotAwaiting(_)
        | ApprovalError::Expired(_)
        | ApprovalError::NoAwaitingStep => ApiError::conflict(e.to_string()),
        ApprovalError::Store(other) => ApiError::internal(other.to_string()),
    }
}

/// Reload a run and serialize it in the same shape as `GET /api/runs/:id`
/// so the UI can refresh from an approve/reject response.
fn run_response(s: &AppState, id: &str) -> ApiResult<Json<serde_json::Value>> {
    let record = s.run_store.load(id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let steps = s.run_store.read_step_results(id).unwrap_or_default();
    let usage = crate::usage::summarize_run(&s.run_store, id, &s.pricing);
    Ok(Json(
        serde_json::json!({ "run": record, "steps": steps, "usage": usage }),
    ))
}

/// Optional `?host=<id>` query param for control endpoints
/// (`approve` / `reject` / `cancel`).
/// Absent or `"local"` → today's local logic. Remote id → proxy via connector.
#[derive(serde::Deserialize, Default)]
struct RunControlQuery {
    #[serde(default)]
    host: Option<String>,
}

/// Optional body for `POST /api/runs/:id/approve`. `mode` selects the
/// permission mode (`ask` / `bypass` / `readonly`) the resumed run runs
/// under; an absent/empty body leaves it `None` (worker default).
#[derive(serde::Deserialize, Default)]
struct ApproveBody {
    #[serde(default)]
    mode: Option<String>,
}

/// `POST /api/runs/:id/approve[?host=<id>]` — record a web approval decision for
/// a paused (awaiting-approval) run.
///
/// Without `?host=` (or `?host=local`): sets the `resume_requested_at` marker
/// (and the optional `resume_mode`) that a background worker picks up. The run
/// stays `AwaitingApproval` until the worker resumes it.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::approve_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`.
///
/// The JSON body is optional — a bodyless POST is accepted and treated as
/// `mode = None`.
async fn approve_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
    body: Option<Json<ApproveBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        let mode = body.and_then(|b| b.0.mode).unwrap_or_default();
        conn.approve_run(&id, &mode).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(serde_json::json!({ "ok": true, "host_id": host })));
    }
    // Local path: unchanged.
    let now = chrono::Utc::now();
    let mode = body.and_then(|b| b.0.mode);
    s.run_store
        .request_resume_approval(&id, "web", mode.as_deref(), now)
        .map_err(|e| map_approval_err(&id, e))?;
    let mut resp = run_response(&s, &id)?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
}

#[derive(serde::Deserialize)]
struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /api/runs/:id/reject[?host=<id>]` — record a web rejection decision.
///
/// Without `?host=` (or `?host=local`): transitions the run to `Rejected`.
/// With `?host=<remote-id>`: proxies via [`HostConnector::reject_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`.
async fn reject_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
    Json(body): Json<RejectBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.reject_run(&id, body.reason.as_deref())
            .await
            .map_err(|e| match e {
                HostConnectorError::NotFound(m) => ApiError::not_found(m),
                HostConnectorError::Invalid(m) => ApiError::bad_request(m),
                other => ApiError::internal(other.to_string()),
            })?;
        return Ok(Json(serde_json::json!({ "ok": true, "host_id": host })));
    }
    // Local path: unchanged.
    let now = chrono::Utc::now();
    let reason = body.reason.unwrap_or_default();
    s.run_store
        .reject(&id, "web", &reason, now)
        .map_err(|e| map_approval_err(&id, e))?;
    let mut resp = run_response(&s, &id)?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
}

/// Optional body for `POST /api/runs/:id/cancel`.
#[derive(serde::Deserialize, Default)]
struct CancelBody {
    #[serde(default)]
    reason: Option<String>,
}

/// Map a [`CancelError`] to an [`ApiError`]:
/// - `AlreadyTerminal` → 409 (the run is already finished)
/// - `NotFound` → 404
/// - `Store` → 500
fn map_cancel_err(id: &str, e: CancelError) -> ApiError {
    match e {
        CancelError::AlreadyTerminal(_) => ApiError::conflict(e.to_string()),
        CancelError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        CancelError::Store(other) => ApiError::internal(other),
    }
}

/// `POST /api/runs/:id/cancel[?host=<id>]` — cancel an in-flight run.
///
/// Without `?host=` (or `?host=local`): a `Pending`/`Running` run is marked
/// `Cancelled` (and its live runner TERM'd); a run paused at an approval gate is
/// rejected. Terminal runs yield 409. The JSON body is optional.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::cancel_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`.
async fn cancel_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
    body: Option<Json<CancelBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.cancel_run(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(serde_json::json!({ "ok": true, "host_id": host })));
    }
    // Local path: unchanged.
    let now = chrono::Utc::now();
    let reason = body
        .and_then(|b| b.0.reason)
        .unwrap_or_else(|| "Cancelled from control plane".to_string());
    let _outcome: CancelOutcome = s
        .run_store
        .cancel(&id, "web", &reason, now)
        .map_err(|e| map_cancel_err(&id, e))?;
    let mut resp = run_response(&s, &id)?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
}

/// Map a [`rupu_orchestrator::runs::PauseError`] from `RunStore::pause` to
/// an [`ApiError`]:
/// - `NotFound` → 404
/// - `AlreadyTerminal` / `NotRunning` → 409 (the run isn't in a state that
///   can be cooperatively paused)
/// - `Store` → 500
fn map_pause_err(id: &str, e: PauseError) -> ApiError {
    match e {
        PauseError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        PauseError::AlreadyTerminal(_) | PauseError::NotRunning(_) => {
            ApiError::conflict(format!("run {id} is not running"))
        }
        PauseError::Store(msg) => ApiError::internal(msg),
    }
}

/// `POST /api/runs/:id/pause[?host=<id>]` — cooperatively pause an
/// in-flight run.
///
/// Without `?host=` (or `?host=local`): a `Pending`/`Running` run is marked
/// `Paused` (non-terminal — resumable via `/resume`). Any other status
/// (already paused, awaiting approval, or terminal) yields 409.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::pause_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`.
async fn pause_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.pause_run(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::conflict(m),
            HostConnectorError::Unsupported(m) => ApiError::not_available(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(serde_json::json!({ "ok": true, "host_id": host })));
    }
    // Local path: mirrors cancel_run's local branch — operate on the store
    // directly rather than through the connector. Unlike cancel, a
    // cooperative pause also needs the marker file written so a *detached*
    // `rupu workflow run <id>` subprocess (the shape `cp serve` launches)
    // actually learns it was paused — the subprocess polls the marker, it
    // does not re-read its own record status. Mirrors
    // `LocalHostConnector::pause_run`.
    let now = chrono::Utc::now();
    s.run_store
        .pause(&id, now)
        .map_err(|e| map_pause_err(&id, e))?;
    s.run_store
        .set_pause_marker(&id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut resp = run_response(&s, &id)?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
}

/// `POST /api/runs/:id/resume[?host=<id>]` — resume a `Paused` run.
///
/// **Launcher-gated** (501 on a read-only deploy): the actual re-entry into
/// `run_workflow` happens in a background worker that only runs inside
/// `rupu cp serve`, so a deploy with no `RunLauncher` configured has no way
/// to ever consume the resume request — reporting success there would be a
/// silent no-op.
///
/// Without `?host=` (or `?host=local`): a `Paused` run gets its
/// `resume_requested_at` marker set (mirrors `approve`'s marker-only
/// design) for the background worker to pick up and re-enter
/// `run_workflow` with the persisted checkpoint (+ mid-step seed, when
/// present). Any other status yields 409.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::resume_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`.
async fn resume_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    s.launcher
        .as_ref()
        .ok_or_else(|| ApiError::not_available("resuming a paused run requires `rupu cp serve`"))?;

    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.resume_run(&id).await.map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Invalid(m) => ApiError::conflict(m),
            HostConnectorError::Unsupported(m) => ApiError::not_available(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(serde_json::json!({ "ok": true, "host_id": host })));
    }
    // Local path.
    let record = s.run_store.load(&id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    if record.status != RunStatus::Paused {
        return Err(ApiError::conflict(format!(
            "run {id} is `{}`, not `paused`",
            record.status.as_str()
        )));
    }
    let now = chrono::Utc::now();
    s.run_store
        .request_resume_approval(&id, "web", None, now)
        .map_err(|e| map_approval_err(&id, e))?;
    let mut resp = run_response(&s, &id)?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
}

/// Trigger provenance for the wire.
///
/// Thin wrapper over `RunRecord::trigger()` — kept so existing call sites read
/// unchanged. The classification itself lives in `rupu-orchestrator`, beside the
/// fields it reads.
pub(crate) fn trigger_of(r: &RunRecord) -> &'static str {
    r.trigger().as_str()
}

// ── Host-aware helpers ────────────────────────────────────────────────────────

/// Upper-bound on rows fetched from each host during a fan-out list.
/// Prevents unbounded merges while staying well above any realistic run count.
const FAN_OUT_LIMIT: usize = 10_000;

/// Resolve a `host_id` string to a live connector, mapping unknown host → 404.
pub(crate) fn resolve_host(
    s: &AppState,
    host_id: &str,
) -> ApiResult<Arc<dyn crate::host::connector::HostConnector>> {
    s.hosts.resolve(host_id).map_err(|e| match e {
        HostConnectorError::NotFound(_) => ApiError::not_found(format!("host {host_id} not found")),
        other => ApiError::internal(other.to_string()),
    })
}

/// Concurrently call `list_runs` on every registered host, tag each row with
/// its `host_id`, merge, and sort newest-first. A per-host failure produces an
/// empty contribution plus a warning — it never fails the whole merge.
async fn fan_out_list_runs(
    s: &AppState,
    kind: RunKind,
    lifecycle: Option<String>,
) -> Vec<serde_json::Value> {
    let hosts = s.hosts.list_hosts();
    let futs: Vec<_> = hosts
        .into_iter()
        .map(|h| {
            let registry = Arc::clone(&s.hosts);
            let lifecycle = lifecycle.clone();
            async move {
                let host_id = h.id;
                let params = RunListQuery {
                    kind,
                    offset: 0,
                    limit: FAN_OUT_LIMIT,
                    lifecycle,
                };
                let conn = match registry.resolve(&host_id) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            host_id = %host_id,
                            error = %e,
                            "run fan-out: could not resolve connector; skipping host"
                        );
                        return Vec::new();
                    }
                };
                match conn.list_runs(params).await {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|mut v| {
                            v["host_id"] = serde_json::json!(&host_id);
                            v
                        })
                        .collect(),
                    Err(e) => {
                        tracing::warn!(
                            host_id = %host_id,
                            error = %e,
                            "run fan-out: list_runs failed; skipping host"
                        );
                        Vec::new()
                    }
                }
            }
        })
        .collect();

    let all: Vec<Vec<serde_json::Value>> = join_all(futs).await;
    let mut merged: Vec<serde_json::Value> = all.into_iter().flatten().collect();
    // Sort newest-first by `started_at` (ISO-8601 strings compare lexicographically).
    merged.sort_by(|a, b| {
        let ta = a["started_at"].as_str().unwrap_or("");
        let tb = b["started_at"].as_str().unwrap_or("");
        tb.cmp(ta)
    });
    merged
}

/// Slim list DTO for `GET /api/runs` and `GET /api/runs/workflows`.
///
/// The full record (including step results) is available at
/// `GET /api/runs/:id`.
#[derive(serde::Serialize)]
pub(crate) struct RunListRow {
    pub(crate) id: String,
    pub(crate) workflow_name: String,
    pub(crate) status: RunStatus,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    pub(crate) finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) trigger: &'static str,
    pub(crate) usage: crate::usage::UsageSummary,
    pub(crate) turns: u64,
    pub(crate) duration_ms: Option<u64>,
}

impl From<&RunRecord> for RunListRow {
    fn from(r: &RunRecord) -> Self {
        Self {
            id: r.id.clone(),
            workflow_name: r.workflow_name.clone(),
            status: r.status,
            started_at: r.started_at,
            finished_at: r.finished_at,
            trigger: trigger_of(r),
            usage: crate::usage::UsageSummary::default(),
            turns: 0,
            duration_ms: None,
        }
    }
}

impl RunListRow {
    /// Build a row with its usage summary, turn count, and duration filled from
    /// the run's transcripts (and the run record's wall-clock when available).
    pub(crate) fn with_usage(
        r: &RunRecord,
        store: &rupu_orchestrator::runs::RunStore,
        pricing: &rupu_config::PricingConfig,
    ) -> Self {
        let mut row = Self::from(r);
        let m = crate::usage::run_metrics(store, &r.id, pricing);
        row.usage = m.usage;
        row.turns = m.turns;
        // Prefer the run record's wall-clock when finished; else the transcript duration.
        row.duration_ms = match r.finished_at {
            Some(fin) => {
                let ms = (fin - r.started_at).num_milliseconds().max(0);
                Some(ms as u64)
            }
            None => m.duration_ms,
        };
        row
    }
}

/// Shared run-listing logic used by both HTTP handlers and host connectors
/// ([`crate::host::local::LocalHostConnector`],
/// [`crate::host::tunnel::TunnelHostConnector`]).
///
/// Filters by `workflow_only` (true = exclude event/cron-triggered runs), the
/// optional `lifecycle` group, and the optional `worker_id` (pass `Some(id)`
/// to scope results to a specific tunnel node; `None` returns all runs).
/// Sorts newest-first and paginates.
pub(crate) fn query_run_rows(
    store: &rupu_orchestrator::runs::RunStore,
    offset: usize,
    limit: usize,
    lifecycle: Option<&str>,
    workflow_only: bool,
    worker_id: Option<&str>,
    pricing: &rupu_config::PricingConfig,
) -> Result<Vec<RunListRow>, rupu_orchestrator::RunStoreError> {
    let mut runs = store.list()?;
    if workflow_only {
        runs.retain(|r| r.event.is_none() && r.source_wake_id.is_none());
    }
    if let Some(lc) = lifecycle {
        runs.retain(|r| in_lifecycle(r.status, Some(lc)));
    }
    if let Some(wid) = worker_id {
        runs.retain(|r| r.worker_id.as_deref() == Some(wid));
    }
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let page = crate::pagination::PageQuery {
        offset: Some(offset),
        limit: Some(limit),
    };
    let page_runs = crate::pagination::paginate(runs, &page);
    Ok(page_runs
        .iter()
        .map(|r| RunListRow::with_usage(r, store, pricing))
        .collect())
}

/// Shared run-detail builder used by both HTTP handlers and
/// [`crate::host::local::LocalHostConnector`].
///
/// Returns the `{ run, steps, usage }` JSON object `GET /api/runs/:id` produces.
pub(crate) fn query_run_detail(
    store: &rupu_orchestrator::runs::RunStore,
    id: &str,
    pricing: &rupu_config::PricingConfig,
) -> Result<serde_json::Value, rupu_orchestrator::RunStoreError> {
    let record = store.load(id)?;
    let steps = store.read_step_results(id).unwrap_or_default();
    let usage = crate::usage::summarize_run(store, id, pricing);
    Ok(serde_json::json!({ "run": record, "steps": steps, "usage": usage }))
}

/// Query params for `GET /api/runs`: offset/limit paging plus an optional
/// `?host=<id>` to scope to a single host (omitting fans out across all hosts).
#[derive(serde::Deserialize, Default)]
struct RunsListQuery {
    offset: Option<usize>,
    limit: Option<usize>,
    /// When present, restrict to this host only; absent → fan-out all hosts.
    host: Option<String>,
}

impl RunsListQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }
}

/// `GET /api/runs[?host=<id>]`
///
/// Without `?host=`: fan-out across every registered host concurrently, tag
/// each row with `host_id`, merge newest-first, paginate.
///
/// With `?host=<id>`: list only that host's runs (tagged with `host_id`).
/// Unknown host id → 404.
async fn list_runs(
    State(s): State<AppState>,
    Query(q): Query<RunsListQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let page = q.page();
    if let Some(host_id) = &q.host {
        let conn = resolve_host(&s, host_id)?;
        let params = RunListQuery {
            kind: RunKind::All,
            offset: page.offset(),
            limit: page.limit(),
            lifecycle: None,
        };
        let rows = conn
            .list_runs(params)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let tagged: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|mut v| {
                v["host_id"] = serde_json::json!(host_id);
                v
            })
            .collect();
        return Ok(Json(tagged));
    }
    // Fan-out: collect all hosts → merge → paginate.
    let rows = fan_out_list_runs(&s, RunKind::All, None).await;
    Ok(Json(crate::pagination::paginate(rows, &page)))
}

#[derive(serde::Deserialize)]
struct WorkflowRunsQuery {
    // Flat fields, NOT `#[serde(flatten)] PageQuery` — serde_urlencoded (axum
    // `Query`) cannot deserialize integers through a flattened struct
    // ("invalid type: string, expected usize"), so offset/limit are inlined.
    offset: Option<usize>,
    limit: Option<usize>,
    /// Optional lifecycle group: `active` | `completed` | `failed`.
    lifecycle: Option<String>,
    /// When present, restrict to this host; absent → fan-out all hosts.
    #[serde(default)]
    host: Option<String>,
}

impl WorkflowRunsQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }
}

/// Does this run's status fall in the given lifecycle group? `None` group → all.
fn in_lifecycle(status: RunStatus, group: Option<&str>) -> bool {
    match group {
        Some("active") => matches!(
            status,
            RunStatus::Running
                | RunStatus::Pending
                | RunStatus::AwaitingApproval
                | RunStatus::Paused
        ),
        Some("completed") => matches!(status, RunStatus::Completed),
        Some("failed") => matches!(
            status,
            RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled
        ),
        _ => true,
    }
}

/// `GET /api/runs/workflows[?host=<id>]` — manual/direct runs only (no event or
/// cron wake), with the same fan-out / single-host routing as `list_runs`.
async fn list_workflow_runs(
    State(s): State<AppState>,
    Query(q): Query<WorkflowRunsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let page = q.page();
    if let Some(host_id) = &q.host {
        let conn = resolve_host(&s, host_id)?;
        let params = RunListQuery {
            kind: RunKind::Workflow,
            offset: page.offset(),
            limit: page.limit(),
            lifecycle: q.lifecycle.clone(),
        };
        let rows = conn
            .list_runs(params)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let tagged: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|mut v| {
                v["host_id"] = serde_json::json!(host_id);
                v
            })
            .collect();
        return Ok(Json(tagged));
    }
    let rows = fan_out_list_runs(&s, RunKind::Workflow, q.lifecycle.clone()).await;
    Ok(Json(crate::pagination::paginate(rows, &page)))
}

/// Optional `?host=<id>` query param for `GET /api/runs/:id`,
/// `GET /api/runs/:id/log`, `GET /api/runs/:id/graph`, and
/// `GET /api/runs/:id/usage-timeline`.
#[derive(serde::Deserialize, Default)]
pub(crate) struct RunDetailQuery {
    /// When present and not `"local"`, proxy the request to the named host.
    pub(crate) host: Option<String>,
}

/// Map a [`RunStoreError`] to 404 (not found) or 500 (anything else) — the
/// mapping shared by every run-detail endpoint's local-store read path.
pub(crate) fn run_not_found_or_internal(id: &str, e: RunStoreError) -> ApiError {
    match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    }
}

/// Map a [`HostConnectorError`] from a proxied run-detail read to an
/// [`ApiError`] — fail-closed on an unreachable host (a clear error, never a
/// panic/500-with-no-context).
fn host_connector_err(id: &str, host_id: &str, e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        HostConnectorError::Unreachable(m) => {
            ApiError::internal(format!("host {host_id} unreachable: {m}"))
        }
        other => ApiError::internal(other.to_string()),
    }
}

/// Proxy `GET /api/runs/:id` to a resolved host. Shared by the explicit
/// `?host=` branch and the resolver's [`RunLocation::Host`] branch.
async fn get_run_from_host(s: &AppState, host_id: &str, id: &str) -> ApiResult<serde_json::Value> {
    let conn = resolve_host(s, host_id)?;
    conn.get_run(id)
        .await
        .map_err(|e| host_connector_err(id, host_id, e))
}

/// Build a `RunRecord`-shaped JSON value (plus a sibling `cycle_id`) for a
/// [`RunLocation::Unpersisted`] run — no `run.json` was ever written (the
/// autoflow dispatch failed before/without persisting one), so the
/// structural fields the schema requires but the history doesn't carry
/// (`workspace_id`, `workspace_path`, `transcript_dir`, `started_at`) are
/// filled with an explicit empty/best-effort placeholder rather than
/// silently defaulting — the point is to surface the failure, not pretend a
/// real run executed. Shared by `get_run` and `run_graph` so both
/// endpoints' `"run"` key stays byte-for-byte the same shape.
///
/// `error_message` is only populated for a terminal-failure `status`
/// (`Failed`) — a synthesized `Running`/`AwaitingApproval` record has no
/// failure yet, so showing one would misrepresent an in-flight/awaiting run
/// as broken.
///
/// `issue_ref` is the resolver's full stable ref (e.g.
/// `github:owner/repo/issues/42`, from [`super::run_resolve::RunLocation::Unpersisted`]'s
/// `issue_ref` field) — not the bare display number.
pub(crate) fn synthesize_unpersisted_run(
    id: &str,
    cycle_id: &str,
    status: RunStatus,
    failure: &str,
    workflow_name: &str,
    issue_ref: Option<&str>,
) -> serde_json::Value {
    let now = chrono::Utc::now();
    let error_message = matches!(status, RunStatus::Failed).then(|| failure.to_string());
    let record = RunRecord {
        id: id.to_string(),
        workflow_name: workflow_name.to_string(),
        status,
        inputs: Default::default(),
        event: None,
        workspace_id: String::new(),
        workspace_path: PathBuf::new(),
        transcript_dir: PathBuf::new(),
        started_at: now,
        finished_at: Some(now),
        error_message,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: issue_ref.map(str::to_string),
        issue: None,
        parent_run_id: None,
        backend_id: None,
        worker_id: None,
        artifact_manifest_path: None,
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        final_output: None,
    };
    let mut v = serde_json::to_value(&record).unwrap_or_else(|_| serde_json::json!({ "id": id }));
    v["cycle_id"] = serde_json::json!(cycle_id);
    v
}

/// `GET /api/runs/:id[?host=<id>]`
///
/// An explicit `?host=<remote-id>` takes precedence over the resolver and
/// proxies unchanged (today's behavior for callers who already know the
/// host). Otherwise, dispatches on [`resolve_run_location`]:
/// - `Global` → the local store (unchanged).
/// - `ProjectLocal` → a project's own `.rupu/runs/` store, same DTO shape.
/// - `Host` → proxy to the resolved host.
/// - `Unpersisted` → synthesize a failed/blocked record instead of 404ing.
/// - `NotFound` → 404.
async fn get_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunDetailQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(host_id) = q.host.as_deref().filter(|h| *h != "local") {
        return get_run_from_host(&s, host_id, &id).await.map(Json);
    }

    match resolve_run_location(&s, &id).await {
        RunLocation::Global => {
            let detail = query_run_detail(&s.run_store, &id, &s.pricing)
                .map_err(|e| run_not_found_or_internal(&id, e))?;
            Ok(Json(detail))
        }
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            let detail = query_run_detail(&store, &id, &s.pricing)
                .map_err(|e| run_not_found_or_internal(&id, e))?;
            Ok(Json(detail))
        }
        RunLocation::Host { host_id } => get_run_from_host(&s, &host_id, &id).await.map(Json),
        RunLocation::Unpersisted {
            cycle_id,
            status,
            failure,
            workflow_name,
            issue_ref,
            ..
        } => {
            let run = synthesize_unpersisted_run(
                &id,
                &cycle_id,
                status,
                &failure,
                &workflow_name,
                issue_ref.as_deref(),
            );
            Ok(Json(serde_json::json!({
                "run": run,
                "steps": [],
                "usage": crate::usage::UsageSummary::default(),
            })))
        }
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {id} not found"))),
    }
}

/// Proxy `GET /api/runs/:id/log` (as `stream_run_events`) to a resolved host.
/// Shared by the explicit `?host=` branch and the resolver's
/// [`RunLocation::Host`] branch.
async fn get_run_log_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
) -> Result<Response, ApiError> {
    let conn = resolve_host(s, host_id)?;
    let stream = conn.stream_run_events(id).await.map_err(|e| match e {
        HostConnectorError::NotFound(_) => {
            ApiError::not_found(format!("run {id} not found on host {host_id}"))
        }
        HostConnectorError::Unreachable(m) => {
            ApiError::internal(format!("host {host_id} unreachable: {m}"))
        }
        other => ApiError::internal(other.to_string()),
    })?;
    crate::api::events::proxy_event_byte_stream(stream)
}

/// Verify the run exists in `store`, then tail its `events.jsonl`. Shared by
/// the `Global` and `ProjectLocal` branches of `get_run_log`.
async fn tail_local_log(store: &RunStore, id: &str) -> Result<Response, ApiError> {
    store
        .load(id)
        .map_err(|e| run_not_found_or_internal(id, e))?;
    let events_path = store.events_path(id);
    let sse = crate::sse::tail_events_sse(events_path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(sse.into_response())
}

/// `GET /api/runs/:id/log[?host=<id>]` — tail the run's `events.jsonl` as a
/// live SSE stream.
///
/// An explicit `?host=<remote-id>` takes precedence over the resolver
/// (unchanged proxy behavior). Otherwise dispatches on
/// [`resolve_run_location`]: `Global`/`ProjectLocal` tail the resolved
/// store's `events.jsonl`; `Host` proxies; `Unpersisted` has no
/// `events.jsonl` anywhere (the run never persisted one) so it returns an
/// empty-but-OK SSE stream rather than erroring; `NotFound` → 404.
///
/// The stream stays open while the run is in progress and emits each
/// [`rupu_orchestrator::executor::Event`] as a JSON `data:` line.
async fn get_run_log(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunDetailQuery>,
) -> Result<Response, ApiError> {
    if let Some(host_id) = q.host.as_deref().filter(|h| *h != "local") {
        return get_run_log_from_host(&s, host_id, &id).await;
    }

    match resolve_run_location(&s, &id).await {
        RunLocation::Global => tail_local_log(&s.run_store, &id).await,
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            tail_local_log(&store, &id).await
        }
        RunLocation::Host { host_id } => get_run_log_from_host(&s, &host_id, &id).await,
        RunLocation::Unpersisted { .. } => Ok(crate::sse::empty_events_sse().into_response()),
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {id} not found"))),
    }
}

/// Proxy `GET /api/runs/:id/usage-timeline` to a resolved host. Shared by the
/// explicit `?host=` branch and the resolver's [`RunLocation::Host`] branch.
async fn usage_timeline_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
) -> ApiResult<serde_json::Value> {
    let conn = resolve_host(s, host_id)?;
    conn.proxy_get_json(&format!("/api/runs/{id}/usage-timeline"))
        .await
        .map_err(|e| match e {
            HostConnectorError::NotFound(m) => ApiError::not_found(m),
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })
}

/// Build the per-turn usage-timeline series for a run in `store`. Shared by
/// the `Global` and `ProjectLocal` branches of `get_run_usage_timeline`.
fn build_usage_timeline_json(store: &RunStore, id: &str) -> ApiResult<serde_json::Value> {
    store
        .load(id)
        .map_err(|e| run_not_found_or_internal(id, e))?;
    let steps = store.read_step_results(id).unwrap_or_default();
    let mut labeled: Vec<(String, std::path::PathBuf)> = Vec::new();
    for st in &steps {
        labeled.push((st.step_id.clone(), st.transcript_path.clone()));
        for item in &st.items {
            labeled.push((st.step_id.clone(), item.transcript_path.clone()));
        }
    }
    let series = crate::usage::turn_series(&labeled);
    serde_json::to_value(series).map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/runs/:id/usage-timeline[?host=<id>]` — ordered per-turn token
/// series across every transcript the run produced (step results + fan-out
/// items), labeled by step id.
///
/// An explicit `?host=<remote-id>` takes precedence over the resolver
/// (unchanged proxy behavior). Otherwise dispatches on
/// [`resolve_run_location`]: `Global`/`ProjectLocal` read the resolved
/// store; `Host` proxies; `Unpersisted` has no transcripts anywhere, so it
/// returns an empty (but 200 OK) series; `NotFound` → 404.
async fn get_run_usage_timeline(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunDetailQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(host_id) = q.host.as_deref().filter(|h| *h != "local") {
        return usage_timeline_from_host(&s, host_id, &id).await.map(Json);
    }

    match resolve_run_location(&s, &id).await {
        RunLocation::Global => build_usage_timeline_json(&s.run_store, &id).map(Json),
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            build_usage_timeline_json(&store, &id).map(Json)
        }
        RunLocation::Host { host_id } => {
            usage_timeline_from_host(&s, &host_id, &id).await.map(Json)
        }
        RunLocation::Unpersisted { .. } => Ok(Json(
            serde_json::to_value(Vec::<crate::usage::TurnPoint>::new())
                .map_err(|e| ApiError::internal(e.to_string()))?,
        )),
        RunLocation::NotFound => Err(ApiError::not_found(format!("run {id} not found"))),
    }
}

/// `GET /api/runs/:id/autoflow` — autoflow-history context for a run:
/// which entity/claim/cycle produced it, prior cycles for the same entity,
/// and (when known) which project/host it ran under.
///
/// 404 when the run has no autoflow-history trail — a plain, non-autoflow
/// run. This is the caller's signal to not render an Autoflow panel at all,
/// distinct from "run not found" (which the run-detail endpoints already
/// cover).
async fn get_run_autoflow(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    let ctx = crate::api::run_resolve::autoflow_run_context(&s.global_dir, &id)
        .ok_or_else(|| ApiError::not_found(format!("run {id} has no autoflow context")))?;

    let prior_cycles: Vec<_> = ctx
        .issue_ref
        .as_deref()
        .map(|iref| crate::api::run_resolve::entity_cycles(&s.global_dir, iref))
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.cycle_id != ctx.cycle_id)
        .collect();

    let claim_store = rupu_workspace::AutoflowClaimStore {
        root: s.global_dir.join("autoflows").join("claims"),
    };
    let claim = claim_store
        .list()
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.last_run_id.as_deref() == Some(id.as_str()))
        .map(crate::api::autoflow_claims::ClaimRow::from);

    Ok(Json(serde_json::json!({
        "repo_ref": ctx.repo_ref,
        "issue_ref": ctx.issue_ref,
        "entity": ctx.entity,
        "workflow_name": ctx.workflow_name,
        "status": ctx.status,
        "failure": ctx.failure,
        "cycle_id": ctx.cycle_id,
        "workspace_path": ctx.workspace_path,
        "host_id": ctx.host_id,
        "claim": claim,
        "prior_cycles": prior_cycles,
    })))
}

/// Reject any `id` that could be used as a path-traversal vector.
///
/// The axum `Path` extractor percent-decodes the segment, so `..%2F..%2Fx`
/// arrives as `../../x`. We refuse ids that are empty or that contain `/`,
/// `\`, or the `..` component — a valid ULID-style run id never contains any
/// of those characters.
pub(crate) fn validate_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(ApiError::bad_request(format!("invalid run id: {id:?}")));
    }
    Ok(())
}

/// Map a [`RunStoreError`] to an [`ApiError`]:
/// - `NotFound` → 404
/// - `NotTerminal` / `AlreadyExists` → 409
/// - `Io` / `Json` → 500
fn map_run_store_err(id: &str, e: RunStoreError) -> ApiError {
    match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        RunStoreError::NotTerminal(_) => {
            ApiError::conflict(format!("run {id} is not terminal — cancel it first"))
        }
        RunStoreError::AlreadyExists(_) => {
            ApiError::conflict(format!("run {id} already exists in the target scope"))
        }
        RunStoreError::Io(err) => ApiError::internal(err.to_string()),
        RunStoreError::Json(err) => ApiError::internal(err.to_string()),
    }
}

/// `POST /api/runs/:id/archive` — move a terminal run to the archive scope.
///
/// Non-terminal runs yield 409. The run's directory (including transcripts)
/// is renamed into `<global>/runs-archive/<id>`.
async fn archive_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    s.run_store
        .archive(&id)
        .map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "id": id, "archived": true }),
    ))
}

/// `POST /api/runs/:id/restore` — move an archived run back to the active scope.
///
/// Returns 404 if the run is not in the archive.
async fn restore_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    s.run_store
        .restore(&id)
        .map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "id": id, "archived": false }),
    ))
}

/// `DELETE /api/runs/:id` — permanently remove a run from either scope.
///
/// Non-terminal runs in the active scope yield 409. Archived runs are
/// already terminal, so the guard is skipped for them (load returns
/// `NotFound` for archived runs; the guard is omitted, and delete
/// resolves the archive scope).
async fn delete_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    // Guard: refuse to delete a non-terminal active run.
    // `load` only checks the active scope; archived runs are always terminal.
    if let Ok(rec) = s.run_store.load(&id) {
        if !rec.status.is_terminal() {
            return Err(ApiError::conflict(format!(
                "run {id} is not terminal — cancel it first"
            )));
        }
    }
    s.run_store
        .delete(&id)
        .map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "id": id, "deleted": true }),
    ))
}

/// Optional `?kind=<workflow|…>` filter for `GET /api/runs/archived`.
/// Absent → return all archived runs (backward-compatible).
#[derive(serde::Deserialize, Default)]
struct ArchivedQuery {
    #[serde(default)]
    kind: Option<String>,
}

/// `GET /api/runs/archived[?kind=workflow]` — list archived runs, newest-first.
///
/// Returns the same wire shape as the local path of `GET /api/runs`: each row
/// is a [`RunListRow`] serialized to JSON with `"host_id": "local"` injected,
/// matching the field added by [`fan_out_list_runs`] and the single-host path
/// of `list_runs`.
///
/// When `?kind=workflow` is present, only manually-dispatched runs (no event
/// payload and no cron wake id) are returned — mirroring the predicate used by
/// `list_workflow_runs` / `query_run_rows(workflow_only = true)`.
async fn list_archived_runs(
    State(s): State<AppState>,
    Query(q): Query<ArchivedQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let mut records = s
        .run_store
        .list_archived()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if q.kind.as_deref() == Some("workflow") {
        records.retain(|r| r.event.is_none() && r.source_wake_id.is_none());
    }
    let mut rows = Vec::with_capacity(records.len());
    for r in &records {
        let row = RunListRow::with_usage(r, &s.run_store, &s.pricing);
        let mut v = serde_json::to_value(row).map_err(|e| ApiError::internal(e.to_string()))?;
        v["host_id"] = serde_json::json!("local");
        rows.push(v);
    }
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build an `AppState` backed by a fresh tempdir run store.
    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
    }

    /// An `awaiting_approval` run record paused at `step_id`.
    fn awaiting_record(id: &str, step_id: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "wf".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: PathBuf::from("/tmp/proj"),
            transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting_step_id: Some(step_id.into()),
            approval_prompt: Some("approve?".into()),
            awaiting_since: Some(chrono::Utc::now()),
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            final_output: None,
        }
    }

    #[tokio::test]
    async fn approve_awaiting_run_sets_resume_marker_and_stays_awaiting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_app", "gate"), "name: x\n")
            .unwrap();

        let resp = approve_run(
            State(s.clone()),
            Path("run_app".into()),
            Query(RunControlQuery { host: None }),
            None,
        )
        .await
        .expect("approve should succeed");
        // Marker-only: the endpoint records the approval but leaves the
        // run AwaitingApproval for the background worker to approve+resume.
        let body = resp.0;
        assert_eq!(
            body["run"]["status"],
            serde_json::json!("awaiting_approval")
        );
        assert_eq!(body["host_id"], "local");

        let loaded = s.run_store.load("run_app").unwrap();
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
        assert!(loaded.resume_requested_at.is_some());
        // Awaited gate stays intact so the worker can recover which gate
        // to resume.
        assert_eq!(loaded.awaiting_step_id.as_deref(), Some("gate"));
    }

    #[tokio::test]
    async fn reject_awaiting_run_sets_rejected_with_reason() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_rej", "gate"), "name: x\n")
            .unwrap();

        let body = RejectBody {
            reason: Some("not safe".into()),
        };
        let resp = reject_run(
            State(s.clone()),
            Path("run_rej".into()),
            Query(RunControlQuery { host: None }),
            Json(body),
        )
        .await
        .expect("reject should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("rejected"));
        assert_eq!(resp.0["host_id"], "local");

        let loaded = s.run_store.load("run_rej").unwrap();
        assert_eq!(loaded.status, RunStatus::Rejected);
        assert_eq!(loaded.error_message.as_deref(), Some("rejected: not safe"));
        assert!(loaded.finished_at.is_some());
    }

    #[tokio::test]
    async fn approve_non_awaiting_run_is_conflict() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let mut rec = awaiting_record("run_done", "gate");
        rec.status = RunStatus::Completed;
        rec.awaiting_step_id = None;
        s.run_store.create(rec, "name: x\n").unwrap();

        let err = approve_run(
            State(s),
            Path("run_done".into()),
            Query(RunControlQuery { host: None }),
            None,
        )
        .await
        .expect_err("approve on completed run should fail");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn reject_unknown_run_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = reject_run(
            State(s),
            Path("nope".into()),
            Query(RunControlQuery { host: None }),
            Json(RejectBody { reason: None }),
        )
        .await
        .expect_err("reject on missing run should 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn approve_with_bypass_mode_stashes_resume_mode() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_mode", "gate"), "name: x\n")
            .unwrap();

        let body = ApproveBody {
            mode: Some("bypass".into()),
        };
        let _ = approve_run(
            State(s.clone()),
            Path("run_mode".into()),
            Query(RunControlQuery { host: None }),
            Some(Json(body)),
        )
        .await
        .expect("approve should succeed");

        let loaded = s.run_store.load("run_mode").unwrap();
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
        assert_eq!(loaded.resume_mode.as_deref(), Some("bypass"));
        assert!(loaded.resume_requested_at.is_some());
    }

    #[tokio::test]
    async fn approve_with_no_body_leaves_resume_mode_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_nobody", "gate"), "name: x\n")
            .unwrap();

        let _ = approve_run(
            State(s.clone()),
            Path("run_nobody".into()),
            Query(RunControlQuery { host: None }),
            None,
        )
        .await
        .expect("bodyless approve should succeed");

        let loaded = s.run_store.load("run_nobody").unwrap();
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
        assert_eq!(loaded.resume_mode, None);
        assert!(loaded.resume_requested_at.is_some());
    }

    #[tokio::test]
    async fn cancel_running_run_marks_cancelled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let mut rec = awaiting_record("run_cancel", "gate");
        rec.status = RunStatus::Running;
        rec.awaiting_step_id = None;
        rec.approval_prompt = None;
        rec.awaiting_since = None;
        rec.runner_pid = None;
        s.run_store.create(rec, "name: x\n").unwrap();

        let resp = cancel_run(
            State(s.clone()),
            Path("run_cancel".into()),
            Query(RunControlQuery { host: None }),
            None,
        )
        .await
        .expect("cancel should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("cancelled"));
        assert_eq!(resp.0["host_id"], "local");

        let loaded = s.run_store.load("run_cancel").unwrap();
        assert_eq!(loaded.status, RunStatus::Cancelled);
        assert_eq!(
            loaded.error_message.as_deref(),
            Some("Cancelled from control plane")
        );
        assert!(loaded.finished_at.is_some());
    }

    #[tokio::test]
    async fn cancel_terminal_run_is_conflict() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let mut rec = awaiting_record("run_term", "gate");
        rec.status = RunStatus::Completed;
        rec.awaiting_step_id = None;
        s.run_store.create(rec, "name: x\n").unwrap();

        let body = CancelBody {
            reason: Some("too late".into()),
        };
        let err = cancel_run(
            State(s),
            Path("run_term".into()),
            Query(RunControlQuery { host: None }),
            Some(Json(body)),
        )
        .await
        .expect_err("cancel on completed run should fail");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn cancel_unknown_run_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = cancel_run(
            State(s),
            Path("ghost".into()),
            Query(RunControlQuery { host: None }),
            None,
        )
        .await
        .expect_err("cancel on missing run should 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// Never actually invoked in these tests — `resume_run`'s launcher gate
    /// only checks `launcher.is_some()`. Mirrors `api/config.rs`'s
    /// `DummyLauncher` / `api/workflows.rs`'s `MockLauncher`.
    struct DummyLauncher;

    #[async_trait::async_trait]
    impl crate::launcher::RunLauncher for DummyLauncher {
        async fn launch(
            &self,
            _req: crate::launcher::LaunchRequest,
        ) -> Result<String, crate::launcher::LaunchError> {
            Ok("run_dummy".into())
        }
    }

    /// A `test_state` with a launcher installed — marks the deployment as a
    /// writable `cp serve` so launcher-gated endpoints (like `resume`) pass
    /// the gate.
    fn writable_state(tmp: &tempfile::TempDir) -> AppState {
        test_state(tmp).with_launcher(Some(Arc::new(DummyLauncher)))
    }

    /// A `Running` run record, suitable as the target of a `pause` test.
    fn running_record(id: &str) -> RunRecord {
        let mut rec = awaiting_record(id, "gate");
        rec.status = RunStatus::Running;
        rec.awaiting_step_id = None;
        rec.approval_prompt = None;
        rec.awaiting_since = None;
        rec
    }

    /// A `Paused` run record, suitable as the target of a `resume` test.
    fn paused_record(id: &str, step_id: &str) -> RunRecord {
        let mut rec = awaiting_record(id, step_id);
        rec.status = RunStatus::Paused;
        rec.approval_prompt = None;
        rec
    }

    #[tokio::test]
    async fn pause_running_local_run_sets_paused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(running_record("run_pause"), "name: x\n")
            .unwrap();

        let resp = pause_run(
            State(s.clone()),
            Path("run_pause".into()),
            Query(RunControlQuery { host: None }),
        )
        .await
        .expect("pause should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("paused"));
        assert_eq!(resp.0["host_id"], "local");

        let loaded = s.run_store.load("run_pause").unwrap();
        assert_eq!(loaded.status, RunStatus::Paused);
        // The marker is the ONLY delivery channel to a detached subprocess
        // (it polls the marker, it does not re-read its own record status) —
        // without it this would be a fake pause that runs to completion.
        assert!(s.run_store.pause_marker_exists("run_pause"));
    }

    #[tokio::test]
    async fn pause_terminal_run_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_pause_done"), "name: x\n")
            .unwrap();

        let err = pause_run(
            State(s),
            Path("run_pause_done".into()),
            Query(RunControlQuery { host: None }),
        )
        .await
        .expect_err("pausing a completed run should fail");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn resume_requires_launcher() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No launcher installed — read-only deploy.
        let s = test_state(&tmp);
        s.run_store
            .create(paused_record("run_resume_nolauncher", "gate"), "name: x\n")
            .unwrap();

        let err = resume_run(
            State(s),
            Path("run_resume_nolauncher".into()),
            Query(RunControlQuery { host: None }),
        )
        .await
        .expect_err("resume without a launcher should be unavailable");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn resume_non_paused_run_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = writable_state(&tmp);
        s.run_store
            .create(running_record("run_resume_running"), "name: x\n")
            .unwrap();

        let err = resume_run(
            State(s),
            Path("run_resume_running".into()),
            Query(RunControlQuery { host: None }),
        )
        .await
        .expect_err("resuming a running (non-paused) run should conflict");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn resume_paused_run_sets_marker_and_stays_paused() {
        // With a launcher present, resuming a genuinely `Paused` run is
        // marker-only (mirrors `approve`'s design) — the background worker
        // (a separate process/tokio task, not exercised by this unit test)
        // is what actually re-enters `run_workflow`. This test locks the
        // marker-setting contract so a future regression doesn't silently
        // turn `/resume` into a no-op.
        let tmp = tempfile::TempDir::new().unwrap();
        let s = writable_state(&tmp);
        s.run_store
            .create(paused_record("run_resume_ok", "gate"), "name: x\n")
            .unwrap();

        let resp = resume_run(
            State(s.clone()),
            Path("run_resume_ok".into()),
            Query(RunControlQuery { host: None }),
        )
        .await
        .expect("resume should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("paused"));
        assert_eq!(resp.0["host_id"], "local");

        let loaded = s.run_store.load("run_resume_ok").unwrap();
        assert_eq!(loaded.status, RunStatus::Paused);
        assert!(loaded.resume_requested_at.is_some());
        // A background worker's `list_pending_resume` (rupu-orchestrator) is
        // what actually picks this up and spawns `rupu workflow resume` —
        // exercised at the orchestrator layer (see
        // `rupu-orchestrator/src/runs.rs`'s `list_pending_resume` tests) and
        // end-to-end in a later task (T9); not re-driven here.
        let pending = s.run_store.list_pending_resume(chrono::Utc::now()).unwrap();
        assert!(pending.iter().any(|r| r.id == "run_resume_ok"));
    }

    #[tokio::test]
    async fn resume_unknown_run_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = writable_state(&tmp);
        let err = resume_run(
            State(s),
            Path("ghost".into()),
            Query(RunControlQuery { host: None }),
        )
        .await
        .expect_err("resume on missing run should 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// A completed run record suitable for archive / delete tests.
    fn terminal_record(id: &str) -> RunRecord {
        let mut rec = awaiting_record(id, "gate");
        rec.status = RunStatus::Completed;
        rec.awaiting_step_id = None;
        rec.approval_prompt = None;
        rec.awaiting_since = None;
        rec.finished_at = Some(chrono::Utc::now());
        rec
    }

    #[tokio::test]
    async fn archive_then_delete_run_flow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let rec = terminal_record("run_01CPFLOW");
        let id = rec.id.clone();
        s.run_store.create(rec, "name: x\n").unwrap();

        // archive — run moves from active → archive scope
        let _ = archive_run(State(s.clone()), Path(id.clone()))
            .await
            .expect("archive ok");
        assert_eq!(s.run_store.list().unwrap().len(), 0);
        assert_eq!(s.run_store.list_archived().unwrap().len(), 1);

        // delete (from archive)
        let _ = delete_run(State(s.clone()), Path(id.clone()))
            .await
            .expect("delete ok");
        let err = delete_run(State(s.clone()), Path(id.clone()))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_archived_runs_injects_host_id_local() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let rec = terminal_record("run_01ARCHIVEDROW");
        s.run_store.create(rec, "name: x\n").unwrap();
        s.run_store.archive("run_01ARCHIVEDROW").unwrap();

        let resp = list_archived_runs(State(s), Query(ArchivedQuery { kind: None }))
            .await
            .expect("list_archived_runs should succeed");
        let rows = resp.0;
        assert_eq!(rows.len(), 1, "expected one archived row");
        assert_eq!(
            rows[0]["host_id"],
            serde_json::json!("local"),
            "archived row must carry host_id=local to match list_runs wire shape"
        );
    }

    #[tokio::test]
    async fn list_archived_runs_kind_workflow_excludes_event_runs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        // Workflow run (event: None, source_wake_id: None) — must be returned.
        let wf = terminal_record("run_01WF");
        s.run_store.create(wf, "name: x\n").unwrap();
        s.run_store.archive("run_01WF").unwrap();

        // Non-workflow run (event payload set) — must be excluded.
        let mut ev = terminal_record("run_01EV");
        ev.event = Some(serde_json::json!({"type": "push"}));
        s.run_store.create(ev, "name: x\n").unwrap();
        s.run_store.archive("run_01EV").unwrap();

        // Without kind filter: both rows.
        let all = list_archived_runs(State(s.clone()), Query(ArchivedQuery { kind: None }))
            .await
            .expect("no-filter should succeed");
        assert_eq!(all.0.len(), 2, "unfiltered should return both");

        // With kind=workflow: only the workflow run.
        let wf_only = list_archived_runs(
            State(s),
            Query(ArchivedQuery {
                kind: Some("workflow".into()),
            }),
        )
        .await
        .expect("kind=workflow should succeed");
        assert_eq!(wf_only.0.len(), 1, "kind=workflow should exclude event run");
        assert_eq!(wf_only.0[0]["id"], serde_json::json!("run_01WF"));
    }

    #[tokio::test]
    async fn archive_run_traversal_id_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = archive_run(State(s.clone()), Path("../../etc".into()))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        // No filesystem side-effects: archive dir stays empty.
        assert_eq!(s.run_store.list_archived().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn restore_run_traversal_id_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = restore_run(State(s), Path("../../etc".into()))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_run_traversal_id_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = delete_run(State(s), Path("../../etc".into()))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn archive_running_run_conflicts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let mut rec = terminal_record("run_01RUN");
        rec.status = RunStatus::Running;
        let id = rec.id.clone();
        s.run_store.create(rec, "name: x\n").unwrap();
        let err = archive_run(State(s.clone()), Path(id)).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[test]
    fn in_lifecycle_groups_cancelled_with_failed() {
        assert!(in_lifecycle(RunStatus::Cancelled, Some("failed")));
        assert!(!in_lifecycle(RunStatus::Cancelled, Some("active")));
    }

    #[test]
    fn run_list_row_serializes_usage() {
        let row = RunListRow {
            id: "r1".into(),
            workflow_name: "wf".into(),
            status: RunStatus::Completed,
            started_at: chrono::Utc::now(),
            finished_at: None,
            trigger: "manual",
            usage: crate::usage::UsageSummary::default(),
            turns: 0,
            duration_ms: None,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("usage").is_some());
        assert_eq!(v["usage"]["priced"], serde_json::Value::Bool(false));
        assert!(v.get("turns").is_some());
        assert!(v.get("duration_ms").is_some());
    }

    #[test]
    fn in_lifecycle_groups_statuses() {
        assert!(in_lifecycle(RunStatus::Running, Some("active")));
        assert!(in_lifecycle(RunStatus::Paused, Some("active")));
        assert!(in_lifecycle(RunStatus::Completed, Some("completed")));
        assert!(in_lifecycle(RunStatus::Failed, Some("failed")));
        assert!(in_lifecycle(RunStatus::Rejected, Some("failed")));
        assert!(!in_lifecycle(RunStatus::Completed, Some("active")));
        assert!(in_lifecycle(RunStatus::Completed, None)); // no filter → all
    }

    // Regression: `#[serde(flatten)]` on a `PageQuery` made axum's `Query`
    // (serde_urlencoded) reject numeric `limit`/`offset` with
    // "invalid type: string, expected usize". Flat fields fix it.
    #[test]
    fn workflow_runs_query_deserializes_numeric_params() {
        let uri: axum::http::Uri = "http://x/?limit=200&lifecycle=active".parse().unwrap();
        let Query(q) = Query::<WorkflowRunsQuery>::try_from_uri(&uri).unwrap();
        assert_eq!(q.limit, Some(200));
        assert_eq!(q.offset, None);
        assert_eq!(q.lifecycle.as_deref(), Some("active"));

        let uri2: axum::http::Uri = "http://x/?offset=20&limit=20".parse().unwrap();
        let Query(q2) = Query::<WorkflowRunsQuery>::try_from_uri(&uri2).unwrap();
        assert_eq!(q2.offset, Some(20));
        assert_eq!(q2.limit, Some(20));
    }

    // ── Location-aware run endpoints (T2) ───────────────────────────────

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root` — mirrors `run_resolve.rs`'s test
    /// helper of the same name (private to that module, so duplicated here).
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &std::path::Path) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    /// Write a one-event autoflow cycle history file recording `run_id`
    /// against `issue_ref`, optionally with a `CycleFailed` sibling event
    /// (`failure_detail`) and/or a raw (untyped) `host_id` on the
    /// `RunLaunched` event — mirrors `run_resolve.rs`'s test helpers
    /// (private to that module, so a minimal version is duplicated here).
    #[allow(clippy::too_many_arguments)]
    fn write_cycle_with_run(
        tmp: &tempfile::TempDir,
        day: &str,
        cycle_id: &str,
        run_id: &str,
        status: &str,
        issue_ref: &str,
        workflow: &str,
        failure_detail: Option<&str>,
        host_id: Option<&str>,
    ) {
        use rupu_runtime::{
            AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
        };

        let mut cycle = AutoflowCycleRecord {
            version: AutoflowCycleRecord::VERSION,
            cycle_id: cycle_id.into(),
            mode: AutoflowCycleMode::Tick,
            worker_id: Some("worker_local".into()),
            worker_name: Some("local".into()),
            repo_filter: None,
            started_at: format!("{day}T10:00:00Z"),
            finished_at: format!("{day}T10:00:05Z"),
            workflow_count: 1,
            polled_event_count: 0,
            webhook_event_count: 0,
            ran_cycles: 1,
            skipped_cycles: 0,
            failed_cycles: usize::from(failure_detail.is_some()),
            cleaned_claims: 0,
            events: Vec::new(),
        };
        cycle.events.push(AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RunLaunched,
            issue_ref: Some(issue_ref.into()),
            issue_display_ref: Some("42".into()),
            repo_ref: Some("github:Section9Labs/rupu".into()),
            source_ref: None,
            workflow: Some(workflow.into()),
            run_id: Some(run_id.into()),
            wake_id: None,
            wake_event_id: None,
            status: Some(status.into()),
            detail: None,
        });
        if let Some(detail) = failure_detail {
            cycle.events.push(AutoflowCycleEvent {
                kind: AutoflowCycleEventKind::CycleFailed,
                issue_ref: Some(issue_ref.into()),
                repo_ref: Some("github:Section9Labs/rupu".into()),
                workflow: Some(workflow.into()),
                detail: Some(detail.into()),
                ..AutoflowCycleEvent::default()
            });
        }

        let dir = tmp
            .path()
            .join("autoflows")
            .join("history")
            .join("cycles")
            .join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let mut value = serde_json::to_value(&cycle).unwrap();
        if let Some(hid) = host_id {
            value["events"][0]["host_id"] = serde_json::Value::String(hid.to_string());
        }
        std::fs::write(
            dir.join(format!("{cycle_id}.json")),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn get_run_global_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_01GLOBAL"), "name: x\n")
            .unwrap();

        let resp = get_run(
            State(s),
            Path("run_01GLOBAL".into()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("global run should be found exactly as before");
        assert_eq!(resp.0["run"]["id"], serde_json::json!("run_01GLOBAL"));
        assert_eq!(resp.0["run"]["status"], serde_json::json!("completed"));
    }

    #[tokio::test]
    async fn get_run_unpersisted_returns_failed_record_not_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        write_cycle_with_run(
            &tmp,
            "2026-07-01",
            "afc_unpersisted",
            "run_01KWYZ2QY4XYZ",
            "blocked",
            "github:Section9Labs/rupu/issues/42",
            "issue-supervisor-dispatch",
            Some("401 invalid x-api-key"),
            None,
        );

        let resp = get_run(
            State(s),
            Path("run_01KWYZ2QY4XYZ".into()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("unpersisted autoflow run should synthesize a record, not 404");

        let body = resp.0;
        assert_eq!(body["run"]["status"], serde_json::json!("failed"));
        assert_eq!(
            body["run"]["error_message"],
            serde_json::json!("401 invalid x-api-key")
        );
        assert_eq!(
            body["run"]["workflow_name"],
            serde_json::json!("issue-supervisor-dispatch")
        );
        assert_eq!(
            body["run"]["cycle_id"],
            serde_json::json!("afc_unpersisted")
        );
        assert_eq!(
            body["run"]["issue_ref"],
            serde_json::json!("github:Section9Labs/rupu/issues/42"),
            "the synthesized record's issue_ref must be the resolver's full \
             stable ref, not the bare display number"
        );
    }

    /// FIX 2: a synthesized run whose status is NOT a terminal failure (here
    /// `running`, from an `AutoflowClaimRecord`/history status that hasn't
    /// failed) must not carry an `error_message` — showing one would
    /// misrepresent an in-flight run as broken.
    #[tokio::test]
    async fn get_run_unpersisted_running_has_no_error_message() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        write_cycle_with_run(
            &tmp,
            "2026-07-01",
            "afc_running",
            "run_still_running",
            "running",
            "github:Section9Labs/rupu/issues/42",
            "issue-supervisor-dispatch",
            None,
            None,
        );

        let resp = get_run(
            State(s),
            Path("run_still_running".into()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("unpersisted running autoflow run should synthesize a record, not 404");

        let body = resp.0;
        assert_eq!(body["run"]["status"], serde_json::json!("running"));
        assert!(
            body["run"]["error_message"].is_null(),
            "a synthesized non-failed record must not carry a failure message: {body:?}"
        );
    }

    #[tokio::test]
    async fn get_run_project_local_reads_project_store() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let proj = tempfile::TempDir::new().unwrap();
        let proj_store = RunStore::new(proj.path().join(".rupu").join("runs"));
        proj_store
            .create(terminal_record("run_proj_x"), "name: wf\nsteps: []\n")
            .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let resp = get_run(
            State(s),
            Path("run_proj_x".into()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("project-local run should be found via the resolver");
        assert_eq!(resp.0["run"]["id"], serde_json::json!("run_proj_x"));
        assert_eq!(resp.0["run"]["status"], serde_json::json!("completed"));
    }

    /// Fake `HostConnector` used only to exercise the `Host` proxy branch
    /// without any real network. Only `get_run`/`proxy_get_json` are
    /// exercised by these tests; every other method panics loudly if
    /// accidentally called, rather than silently no-opping.
    struct FakeHostConnector {
        run_json: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl crate::host::connector::HostConnector for FakeHostConnector {
        async fn info(&self) -> Result<crate::host::connector::HostInfo, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn launch_run(
            &self,
            _req: crate::launcher::LaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn launch_agent(
            &self,
            _req: crate::agent_launcher::AgentLaunchRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn start_session(
            &self,
            _req: crate::session_starter::SessionStartRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn send_session_turn(
            &self,
            _req: crate::session_sender::SendMessageRequest,
        ) -> Result<String, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn list_runs(
            &self,
            _params: RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            Ok(self.run_json.clone())
        }
        async fn approve_run(&self, _run_id: &str, _mode: &str) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn reject_run(
            &self,
            _run_id: &str,
            _reason: Option<&str>,
        ) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn cancel_run(&self, _run_id: &str) -> Result<(), HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn stream_run_events(
            &self,
            _run_id: &str,
        ) -> Result<crate::host::connector::EventByteStream, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn get_transcript(
            &self,
            _path: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            Ok(self.run_json.clone())
        }
    }

    #[tokio::test]
    async fn get_run_host_proxies() {
        let tmp = tempfile::TempDir::new().unwrap();

        // History records this run as having run on host `host_fake` — the
        // forward-looking `host_id` signal on an autoflow history event
        // (see `run_resolve.rs`'s module doc). No current writer sets this;
        // the test injects it directly to exercise the resolver's `Host`
        // branch.
        write_cycle_with_run(
            &tmp,
            "2026-07-03",
            "afc_hostproxy",
            "run_hostproxy",
            "running",
            "github:Section9Labs/rupu/issues/7",
            "issue-supervisor-dispatch",
            None,
            Some("host_fake"),
        );

        let fake_run_json = serde_json::json!({
            "run": { "id": "run_hostproxy", "status": "running" },
            "steps": [],
            "usage": {},
        });
        let fake: Arc<dyn crate::host::connector::HostConnector> = Arc::new(FakeHostConnector {
            run_json: fake_run_json.clone(),
        });

        // `HostRegistry::resolve` only special-cases the literal id
        // `"local"`; any other id is looked up in the `HostStore` and built
        // via `build_connector`. A `HostTransport::Local` entry under a
        // distinct id resolves to the SAME injected connector as
        // `Host::local()` itself would — exactly the seam this test uses to
        // inject a fake connector with zero real network.
        let host_store = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        host_store
            .save(&rupu_workspace::Host {
                id: "host_fake".into(),
                name: "fake".into(),
                transport: rupu_workspace::HostTransport::Local,
                token_hash: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_seen_at: None,
            })
            .unwrap();
        let registry = Arc::new(crate::host::registry::HostRegistry::new(
            host_store,
            Arc::clone(&fake),
        ));
        let s = test_state(&tmp).with_hosts(registry);

        let resp = get_run(
            State(s),
            Path("run_hostproxy".into()),
            Query(RunDetailQuery { host: None }),
        )
        .await
        .expect("host-resolved run should proxy, not 404");
        assert_eq!(resp.0, fake_run_json);
    }

    #[tokio::test]
    async fn autoflow_endpoint_returns_context() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        write_cycle_with_run(
            &tmp,
            "2026-07-04",
            "afc_ctx_old",
            "run_old",
            "complete",
            "github:Section9Labs/rupu/issues/9",
            "issue-supervisor-dispatch",
            None,
            None,
        );
        write_cycle_with_run(
            &tmp,
            "2026-07-05",
            "afc_ctx_new",
            "run_ctx",
            "blocked",
            "github:Section9Labs/rupu/issues/9",
            "issue-supervisor-dispatch",
            Some("boom"),
            None,
        );

        let resp = get_run_autoflow(State(s), Path("run_ctx".into()))
            .await
            .expect("autoflow run should return a context, not 404");
        let body = resp.0;
        assert_eq!(body["cycle_id"], serde_json::json!("afc_ctx_new"));
        assert_eq!(body["failure"], serde_json::json!("boom"));
        assert_eq!(
            body["issue_ref"],
            serde_json::json!("github:Section9Labs/rupu/issues/9")
        );
        let prior = body["prior_cycles"].as_array().unwrap();
        assert_eq!(
            prior.len(),
            1,
            "the current cycle must not appear in its own prior list"
        );
        assert_eq!(prior[0]["cycle_id"], serde_json::json!("afc_ctx_old"));
    }

    #[tokio::test]
    async fn autoflow_endpoint_404_for_non_autoflow_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_plain"), "name: x\n")
            .unwrap();

        let err = get_run_autoflow(State(s), Path("run_plain".into()))
            .await
            .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }
}
