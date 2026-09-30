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
        .route("/api/runs/:id/usage", get(get_run_usage))
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
/// - `NotAwaiting` / `Expired` / `NoAwaitingStep` / `AmbiguousGate` /
///   `GateNotFound` → 409 (the run isn't in a state where the decision can
///   be recorded as-is — `AmbiguousGate` fires when `?gate=` is omitted on a
///   run with >1 parked gate, its `candidates` list embedded in the message
///   body so the UI can prompt; `GateNotFound` fires when `?gate=<id>` names
///   a step that isn't currently parked. Both are rupu-orchestrator Task
///   5b-1's multi-gate awaiting-set additions (spec §7), wired up to the
///   `?gate=` query param by Task 5b-2b.)
/// - everything else → 500
fn map_approval_err(id: &str, e: ApprovalError) -> ApiError {
    match e {
        ApprovalError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        ApprovalError::NotAwaiting(_)
        | ApprovalError::Expired(_)
        | ApprovalError::ExpiredRejected { .. }
        | ApprovalError::NoAwaitingStep
        | ApprovalError::AmbiguousGate { .. }
        | ApprovalError::GateNotFound { .. }
        | ApprovalError::GateAlreadyDecided { .. } => ApiError::conflict(e.to_string()),
        ApprovalError::Store(other) => ApiError::internal(other.to_string()),
    }
}

/// Reload a run and serialize it in the same shape as `GET /api/runs/:id`
/// so the UI can refresh from an approve/reject response. The usage fold runs
/// off the executor and never fails the (already-recorded) mutation.
async fn run_response(s: &AppState, id: &str) -> ApiResult<Json<serde_json::Value>> {
    let record = s.run_store.load(id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let steps = s.run_store.read_step_results(id).unwrap_or_default();
    let u = crate::usage::run_usage_blocking(Arc::clone(&s.run_store), id.to_string()).await;
    let usage = crate::usage::summarize_run_usage(&u, &s.pricing);
    let mut out = serde_json::json!({ "run": record, "steps": steps, "usage": usage });
    crate::codename_legacy::fill_detail_steps(&s.run_store, &record, &mut out);
    Ok(Json(out))
}

/// Run a synchronous request body — run-store reads plus the usage fold,
/// which does file IO under a per-run `std::sync::Mutex` — on tokio's
/// blocking pool instead of an executor thread. Only a panicked body errors
/// (a 500, as it would have been inline).
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("request task failed: {e}")))?
}

/// Optional `?host=<id>` / `?gate=<step_id>` query params for control
/// endpoints (`approve` / `reject` / `cancel` / `pause`).
/// Absent `host` (or `"local"`) → today's local logic. Remote id → proxy
/// via connector.
///
/// `gate` (Task 5b-2b, spec §7) targets a specific parked gate on
/// `approve`/`reject` for a genuinely multi-gate `AwaitingApproval` run
/// (two concurrent paths each hitting a gate). Absent → the sole-gate
/// back-compat behavior [`RunStore::approve_gate`]/[`RunStore::reject_gate`]/
/// [`RunStore::request_resume_approval`] already guarantee: identical to
/// today for a record with at most one parked gate, `ApprovalError::
/// AmbiguousGate` for a genuine multi-gate record. Ignored by `cancel`/
/// `pause` (neither operates on a specific gate).
#[derive(serde::Deserialize, Default)]
struct RunControlQuery {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    gate: Option<String>,
}

/// Optional body for `POST /api/runs/:id/approve`. `mode` selects the
/// permission mode (`ask` / `bypass` / `readonly`) the resumed run runs
/// under; an absent/empty body leaves it `None` (worker default).
#[derive(serde::Deserialize, Default)]
struct ApproveBody {
    #[serde(default)]
    mode: Option<String>,
}

/// `POST /api/runs/:id/approve[?host=<id>][&gate=<step_id>]` — record a web
/// approval decision for a paused (awaiting-approval) run.
///
/// Without `?host=` (or `?host=local`): records the approval of the named
/// gate and sets the `resume_requested_at` marker (and the optional
/// `resume_mode`) asking `cp serve`'s resume worker for a runner
/// ([`RunStore::request_resume_approval`]). Path-scoped (spec §7): only the
/// gate's own path is released; sibling gates stay parked (the run stays
/// `AwaitingApproval` while any is) and approvable, and two approvals in a
/// row are both kept. A runner already executing the run applies it
/// instead.
///
/// `?gate=<step_id>` (Task 5b-2b, spec §7) targets a specific parked gate on
/// a run that has batch-parked more than one (two concurrent DAG paths each
/// hitting a gate). Omitted → the sole-gate back-compat behavior
/// [`RunStore::request_resume_approval`] already guarantees: identical to
/// today for a run with at most one parked gate; a 409 listing every parked
/// gate id (`ApprovalError::AmbiguousGate`, see [`map_approval_err`]) for a
/// genuine multi-gate run. A `gate` naming a step that isn't currently
/// parked also 409s (`ApprovalError::GateNotFound`) rather than a silent
/// no-op or a 500.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::approve_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`. `gate` is NOT threaded
/// through the remote-host path — no cross-host per-gate approval protocol
/// exists yet (every `HostConnector` impl's `approve_run` signature is
/// still gate-id-less); out of scope for this task (see `host/local.rs`'s
/// doc on its own `approve_run`/`reject_run`).
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
    // Local path: unchanged for `gate: None` on a <=1-gate run (see doc).
    let now = chrono::Utc::now();
    let mode = body.and_then(|b| b.0.mode);
    // On the blocking pool: the gate methods take the run lock.
    let (rid, gate) = (id.clone(), q.gate.clone());
    s.run_store
        .blocking(move |store| {
            store.request_resume_approval(&rid, "web", mode.as_deref(), now, gate.as_deref())
        })
        .await
        .map_err(|e| map_approval_err(&id, e))?;
    let mut resp = run_response(&s, &id).await?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
}

#[derive(serde::Deserialize)]
struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /api/runs/:id/reject[?host=<id>][&gate=<step_id>]` — record a web
/// rejection decision.
///
/// Without `?host=` (or `?host=local`): records the rejection of the named
/// gate ([`RunStore::request_resume_rejection`]). On a DAG run the rejection
/// is path-scoped (spec §7): only the gate's own path is pruned and its
/// `on_reject` chain runs, every other path carries on, and sibling gates
/// stay parked and approvable — applied by a runner, which the marker asks
/// `cp serve`'s resume worker for (the CP itself has no execution runtime).
/// A legacy single-cursor run's reject finalizes it `Rejected` here.
/// `?gate=<step_id>` (Task 5b-2b) targets one parked gate on a multi-gate
/// run. Omitted → the sole-gate back-compat behavior: identical to today
/// for a run with at most one parked gate; a 409 listing every parked gate
/// id for a genuine multi-gate run. A `gate` naming a step that isn't
/// currently parked, or one already approved, also 409s rather than a
/// silent no-op or a 500 (see [`map_approval_err`]).
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::reject_run`] and
/// returns `{ "ok": true, "host_id": "<id>" }`. `gate` is NOT threaded
/// through the remote-host path (see `approve_run`'s doc on the same gap).
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
    // Local path: unchanged for `gate: None` on a <=1-gate run (see doc).
    let now = chrono::Utc::now();
    let reason = body.reason.unwrap_or_default();
    // On the blocking pool: the gate methods take the run lock.
    let (rid, gate) = (id.clone(), q.gate.clone());
    s.run_store
        .blocking(move |store| {
            store.request_resume_rejection(&rid, "web", &reason, now, gate.as_deref())
        })
        .await
        .map_err(|e| map_approval_err(&id, e))?;
    let mut resp = run_response(&s, &id).await?;
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
    // On the blocking pool: `cancel` waits (bounded) for the run lock.
    let _outcome: CancelOutcome = {
        let (run_id, reason) = (id.clone(), reason.clone());
        s.run_store
            .blocking(move |store| store.cancel(&run_id, "web", &reason, now))
            .await
            .map_err(|e| map_cancel_err(&id, e))?
    };
    let mut resp = run_response(&s, &id).await?;
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
    // Under the run lock, on the blocking pool (its wait blocks the
    // thread): a cancel that lands meanwhile is refused, never overwritten.
    let run_id = id.clone();
    s.run_store
        .blocking(move |store| store.pause(&run_id, now))
        .await
        .map_err(|e| map_pause_err(&id, e))?;
    s.run_store
        .set_pause_marker(&id)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut resp = run_response(&s, &id).await?;
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
/// With `?host=<remote-id>`: the same `paused` rule, against the host's own
/// status for the run ([`get_run_from_host`]: the host first, its local
/// mirror when the host can't answer); then proxies via
/// [`HostConnector::resume_run`] and returns `{ "ok": true, "host_id":
/// "<id>" }`. A run the page still offered "Resume" for but that finished
/// (or was cancelled) since is refused here — the host's resume would take
/// it as a retry and run it again.
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
        let detail = get_run_from_host(&s, host, &id).await?;
        let status = detail["run"]["status"].as_str().unwrap_or("unknown");
        if status != RunStatus::Paused.as_str() {
            return Err(ApiError::conflict(format!(
                "run {id} is `{status}`, not `paused`"
            )));
        }
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
    // On the blocking pool: the gate methods take the run lock.
    let rid = id.clone();
    s.run_store
        .blocking(move |store| store.request_resume_approval(&rid, "web", None, now, None))
        .await
        .map_err(|e| map_approval_err(&id, e))?;
    let mut resp = run_response(&s, &id).await?;
    resp.0["host_id"] = serde_json::json!("local");
    Ok(resp)
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

/// Map a connector failure on a single-host LIST path (`?host=<remote-id>`)
/// to the status the web's per-host loader reads (spec
/// `2026-10-01-rupu-cp-progressive-per-host-loading-design.md` §7.1):
///
/// - `Unsupported` / `Invalid` → 501: the host cannot serve this listing (an
///   old remote rupu, a transport with no such surface). The web shows the
///   host as *unavailable*, with this reason.
/// - everything else, `NotFound` included → 502: the host gave no usable
///   answer. The web shows it as *offline*, with this reason.
///
/// A 404 on these paths comes only from [`resolve_host`] (an unknown host id =
/// the host was removed from the registry), and the web treats it as exactly
/// that. A connector `NotFound` on a LIST call cannot mean that — a reachable
/// remote that answers a list route with HTTP 404 surfaces as `NotFound` — so
/// it maps to 502, never 404.
///
/// Was a bare 500 for all of them, which cannot tell "down" from "too old".
pub(crate) fn host_list_error(e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::Unsupported(_) | HostConnectorError::Invalid(_) => {
            ApiError::not_available(e.to_string())
        }
        other => ApiError::bad_gateway(other.to_string()),
    }
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
                            crate::codename::inject_codename_row(&mut v, "id", None);
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

/// One row of the runs list.
///
/// `pub` (not `pub(crate)`) because `rupu-cli`'s `run list` emits `Vec<RunListRow>`
/// verbatim as its JSON contract, and SSH `list_runs` returns those rows
/// unmodified. That makes the remote path byte-identical to the local one by
/// CONSTRUCTION. A hand-written mapper here previously omitted `usage` / `turns`
/// / `duration_ms`, which the web UI reads unguarded — one such row blanked the
/// entire app. Do not reintroduce a parallel shape.
#[derive(serde::Serialize)]
pub struct RunListRow {
    pub id: String,
    pub workflow_name: String,
    pub status: RunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger: &'static str,
    pub usage: crate::usage::UsageSummary,
    pub turns: u64,
    pub duration_ms: Option<u64>,
    /// Crew codename — stored, or derived for a legacy run (`codename_derived`).
    pub codename: String,
    pub codename_derived: bool,
}

impl From<&RunRecord> for RunListRow {
    fn from(r: &RunRecord) -> Self {
        let (codename, codename_derived) =
            crate::codename::named(r.codename.as_deref(), &r.id, None);
        Self {
            codename,
            codename_derived,
            id: r.id.clone(),
            workflow_name: r.workflow_name.clone(),
            status: r.status,
            started_at: r.started_at,
            finished_at: r.finished_at,
            trigger: r.trigger_str(),
            usage: crate::usage::UsageSummary::default(),
            turns: 0,
            duration_ms: None,
        }
    }
}

impl RunListRow {
    /// Build a row with its usage summary, turn count, and duration filled from
    /// the run's transcripts (and the run record's wall-clock when available).
    ///
    /// `pub` for the same reason the struct is: `rupu-cli`'s `run list` calls
    /// this directly to build its `Vec<RunListRow>` JSON contract.
    pub fn with_usage(
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
///
/// `pub` (not `pub(crate)`) so a consumer outside this crate CAN reuse it —
/// but note `lifecycle` is a 3-value GROUP vocabulary (`"active"` |
/// `"completed"` | `"failed"`, see `in_lifecycle` in this module), NOT the
/// 8-value exact `RunStatus::as_str()` vocabulary (`running` / `pending` /
/// `awaiting_approval` / `paused` / `completed` / `failed` / `rejected` /
/// `cancelled`). `rupu-cli`'s `run list --status <exact-status>`
/// (`crates/rupu-cli/src/cmd/run.rs`) deliberately does NOT delegate to this
/// function for that reason: passing an exact status through as `lifecycle`
/// would either silently redefine `"failed"` to include `rejected`/
/// `cancelled`, or — for every status that isn't one of the three group
/// names (`running`, `paused`, `pending`, `awaiting_approval`, `rejected`,
/// `cancelled`) — silently no-op the filter entirely, since
/// `in_lifecycle`'s `_ => true` fallback matches everything. See that
/// module's doc comment on `list()` for the full reasoning.
// perf & interaction arc, Plan 5 Task 5 added the trailing `range` param,
// tipping this over clippy's default 7-argument threshold. Bundling these
// into a params struct would touch every existing call site for no real
// clarity gain (they're all positional today, hardly ambiguous) — allow
// is the proportionate fix here.
#[allow(clippy::too_many_arguments)]
pub fn query_run_rows(
    store: &rupu_orchestrator::runs::RunStore,
    offset: usize,
    limit: usize,
    lifecycle: Option<&str>,
    workflow_only: bool,
    worker_id: Option<&str>,
    pricing: &rupu_config::PricingConfig,
    range: &crate::pagination::DateRangeQuery,
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
    // `RunRecord::started_at` is a non-optional `DateTime<Utc>` — no
    // "unknown timestamp" case to worry about here, unlike the string-typed
    // timestamps `DateRangeQuery::contains_str` guards against elsewhere.
    runs.retain(|r| range.contains(r.started_at));
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
///
/// `pub` (not `pub(crate)`) because `rupu-cli`'s `run show` emits this
/// function's output verbatim as its JSON contract. That is deliberate: SSH
/// `get_run` shells `rupu run show` and returns the result, so the remote path
/// yields byte-identical data to the local `mirror_get_run` path — which calls
/// this same function. Hand-building a parallel detail shape in the CLI would
/// let the two drift silently.
pub fn query_run_detail(
    store: &rupu_orchestrator::runs::RunStore,
    id: &str,
    pricing: &rupu_config::PricingConfig,
) -> Result<serde_json::Value, rupu_orchestrator::RunStoreError> {
    let record = store.load(id)?;
    let steps = store.read_step_results(id).unwrap_or_default();
    let usage = crate::usage::summarize_run(store, id, pricing);
    let mut out = serde_json::json!({ "run": record, "steps": steps, "usage": usage });
    crate::codename::inject_codename(&mut out["run"], &record.id, None);
    // Legacy runs: derive names below the run level too (steps, units,
    // panelists, fixers, parallel sub-steps, findings).
    crate::codename_legacy::fill_detail_steps(store, &record, &mut out);
    Ok(out)
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
        let rows = conn.list_runs(params).await.map_err(host_list_error)?;
        let tagged: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|mut v| {
                v["host_id"] = serde_json::json!(host_id);
                crate::codename::inject_codename_row(&mut v, "id", None);
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
    /// Optional RFC-3339 date-range bounds on `started_at` (perf &
    /// interaction arc, Plan 5 Task 5) — see
    /// `crate::pagination::DateRangeQuery`'s doc comment for the
    /// closed-boundary / lenient-parse contract. Only honored on the
    /// `host=local` fast path below (see `list_workflow_runs`'s doc
    /// comment) — a remote-host or fan-out request degrades to unfiltered,
    /// a deferred limitation of the cross-host `RunListQuery` protocol.
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
}

impl WorkflowRunsQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }

    fn range(&self) -> crate::pagination::DateRangeQuery {
        crate::pagination::DateRangeQuery {
            since: self.since.clone(),
            until: self.until.clone(),
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
///
/// **`host=local` bypasses the generic `HostConnector`/`RunListQuery` path**
/// (perf & interaction arc, Plan 5 Task 5) and calls `query_run_rows`
/// directly instead — the only way to thread `since`/`until` through to the
/// filter-before-paginate site without extending `RunListQuery` (the
/// cross-host connector protocol every `HostConnector` impl — HTTP/SSH/
/// Tunnel, not just local — shares) for a date-range feature this task only
/// needs against the local store. Behavior-preserving for every existing
/// caller that never sets `since`/`until`: `query_run_rows` with an inactive
/// `DateRangeQuery` retains everything, identical to what
/// `LocalHostConnector::list_runs` → `query_run_rows` already produced. A
/// remote `host=<id>` or an omitted `host` (fan-out) still goes through the
/// connector trait as before and does not honor `since`/`until` — see
/// `WorkflowRunsQuery.since`'s doc comment.
async fn list_workflow_runs(
    State(s): State<AppState>,
    Query(q): Query<WorkflowRunsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let page = q.page();
    if let Some(host_id) = &q.host {
        if host_id == "local" {
            let store = Arc::clone(&s.run_store);
            let pricing = s.pricing.clone();
            let lifecycle = q.lifecycle.clone();
            let range = q.range();
            let rows = blocking(move || {
                query_run_rows(
                    &store,
                    page.offset(),
                    page.limit(),
                    lifecycle.as_deref(),
                    true, // workflow_only
                    None,
                    &pricing,
                    &range,
                )
                .map_err(|e| ApiError::internal(e.to_string()))
            })
            .await?;
            let tagged: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let mut v = serde_json::to_value(r).unwrap();
                    v["host_id"] = serde_json::json!("local");
                    v
                })
                .collect();
            return Ok(Json(tagged));
        }
        let conn = resolve_host(&s, host_id)?;
        let params = RunListQuery {
            kind: RunKind::Workflow,
            offset: page.offset(),
            limit: page.limit(),
            lifecycle: q.lifecycle.clone(),
        };
        let rows = conn.list_runs(params).await.map_err(host_list_error)?;
        let tagged: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|mut v| {
                v["host_id"] = serde_json::json!(host_id);
                crate::codename::inject_codename_row(&mut v, "id", None);
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
///
/// Remote-first: the remote's own answer is richer while it is reachable, so
/// it is always tried before anything else. Only on error, and only for a
/// transport whose runs are mirrored into our own `RunStore`
/// ([`HostConnector::serves_runs_from_local_mirror`]), do we fall back to
/// that local mirror — this is what keeps a freshly-launched SSH run from
/// 404ing on "does not support `rupu run show`" for the first few seconds
/// before the remote has written its own record, and keeps working against
/// hosts whose `rupu` predates the `run show` command entirely.
async fn get_run_from_host(s: &AppState, host_id: &str, id: &str) -> ApiResult<serde_json::Value> {
    let conn = resolve_host(s, host_id)?;
    match conn.get_run(id).await {
        Ok(mut v) => {
            // An older remote's run record carries no codename; fill it.
            if let Some(run) = v.get_mut("run") {
                crate::codename::inject_codename_row(run, "id", None);
            }
            Ok(v)
        }
        Err(e) => {
            if conn.serves_runs_from_local_mirror() && s.run_store.load(id).is_ok() {
                return local_run_detail(Arc::clone(&s.run_store), s, id).await;
            }
            Err(host_connector_err(id, host_id, e))
        }
    }
}

/// [`query_run_detail`] against `store`, off the async executor.
async fn local_run_detail(
    store: Arc<RunStore>,
    s: &AppState,
    id: &str,
) -> ApiResult<serde_json::Value> {
    let pricing = s.pricing.clone();
    let id = id.to_string();
    blocking(move || {
        query_run_detail(&store, &id, &pricing).map_err(|e| run_not_found_or_internal(&id, e))
    })
    .await
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
        awaiting: Vec::new(),
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
        resume_gate_id: None,
        resume_approver: None,
        resume_rerequested_at: None,
        reject_cleanup_pending: None,
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
        gate_decisions: Vec::new(),
        codename: Some(rupu_codename::crew_for(id)),
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
        RunLocation::Global => local_run_detail(Arc::clone(&s.run_store), &s, &id)
            .await
            .map(Json),
        RunLocation::ProjectLocal { path } => {
            let store = Arc::new(RunStore::new(path.join(".rupu").join("runs")));
            local_run_detail(store, &s, &id).await.map(Json)
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
    let store = std::sync::Arc::new(RunStore::new(store.root.clone()));
    let sse = crate::sse::tail_events_sse(store, id)
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
    // Same mirror rule as `graph.rs`'s `run_graph_from_host`: a transport
    // whose runs live in our RunStore builds the series from those local
    // artifacts (off the executor) rather than proxying a GET it cannot serve.
    if conn.serves_runs_from_local_mirror() {
        let store = Arc::clone(&s.run_store);
        let id = id.to_string();
        return blocking(move || build_usage_timeline_json(&store, &id)).await;
    }
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

/// Build the per-turn usage-timeline series for a run in `store`: the run's
/// usage-fold points ([`crate::usage::run_usage`]) — every LLM call anywhere
/// in the run, in-flight steps and mirrored/dispatched transcripts included.
/// Shared by the `Global` and `ProjectLocal` branches of
/// `get_run_usage_timeline`.
fn build_usage_timeline_json(store: &RunStore, id: &str) -> ApiResult<serde_json::Value> {
    store
        .load(id)
        .map_err(|e| run_not_found_or_internal(id, e))?;
    serde_json::to_value(&crate::usage::run_usage(store, id).points)
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/runs/:id/usage-timeline[?host=<id>]` — ordered per-turn token
/// series of every LLM call the run made (ledger rows, then transcripts with
/// no ledger row: step results, fan-out items, in-flight steps, dispatched
/// sub-runs), labeled by step id.
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
        RunLocation::Global => {
            let store = Arc::clone(&s.run_store);
            blocking(move || build_usage_timeline_json(&store, &id))
                .await
                .map(Json)
        }
        RunLocation::ProjectLocal { path } => {
            let store = RunStore::new(path.join(".rupu").join("runs"));
            blocking(move || build_usage_timeline_json(&store, &id))
                .await
                .map(Json)
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

/// Query for `GET /api/runs/:id/usage`: the optional `?host=` plus the
/// client's incremental cursor (`since` = how many points it holds, `epoch` =
/// the series epoch they came from).
#[derive(Debug, serde::Deserialize, Default)]
pub(crate) struct RunUsageQuery {
    #[serde(default)]
    pub(crate) host: Option<String>,
    #[serde(default)]
    pub(crate) since: Option<usize>,
    /// Accepts the string form the server emits (see [`RunUsageResponse::epoch`]).
    #[serde(default, deserialize_with = "de_opt_u64_from_str")]
    pub(crate) epoch: Option<u64>,
}

/// `GET /api/runs/:id/usage` — a run's live usage (spec 2026-09-29 §5.3).
#[derive(Debug, serde::Serialize)]
pub struct RunUsageResponse {
    pub summary: crate::usage::UsageSummary,
    /// Per step id (`""` = unattributed).
    pub steps: std::collections::BTreeMap<String, crate::usage::UsageSummary>,
    pub turns: u64,
    pub partial: bool,
    /// Serialized as a STRING: `RunUsage.epoch` is a u64 around 1.8e18
    /// (UNIX-nanos base), beyond JS's 2^53 safe-integer range — a JSON number
    /// would round and two epochs would compare equal in the browser.
    #[serde(serialize_with = "ser_u64_as_string")]
    pub epoch: u64,
    /// Index of `points[0]` in the full series.
    pub points_from: usize,
    pub points: Vec<crate::usage::TurnPoint>,
}

fn ser_u64_as_string<S: serde::Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

/// `?epoch=` arrives as a query string; accept a decimal string (or absent).
fn de_opt_u64_from_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    use serde::Deserialize as _;
    let o: Option<String> = Option::deserialize(d)?;
    o.map(|s| s.parse::<u64>().map_err(serde::de::Error::custom))
        .transpose()
}

/// Price a run's fold into the endpoint's response. The tail from `since` is
/// served only when the client's `epoch` is the run's current one and `since`
/// is within the series (the series is append-only within an epoch);
/// otherwise the full series from 0 with the current epoch.
fn run_usage_response(
    u: &crate::usage_index::RunUsage,
    pricing: &rupu_config::PricingConfig,
    since: Option<usize>,
    epoch: Option<u64>,
) -> RunUsageResponse {
    let from = match (since, epoch) {
        (Some(n), Some(e)) if e == u.epoch && n <= u.points.len() => n,
        _ => 0,
    };
    RunUsageResponse {
        summary: crate::usage::summarize_run_usage(u, pricing),
        steps: u
            .by_step
            .iter()
            .map(|(k, rows)| (k.clone(), crate::usage::summarize(rows, pricing)))
            .collect(),
        turns: u.turns,
        partial: u.partial,
        epoch: u.epoch,
        points_from: from,
        points: u.points[from..].to_vec(),
    }
}

fn run_usage_json(
    u: &crate::usage_index::RunUsage,
    pricing: &rupu_config::PricingConfig,
    q: &RunUsageQuery,
) -> ApiResult<serde_json::Value> {
    serde_json::to_value(run_usage_response(u, pricing, q.since, q.epoch))
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// The live usage of run `id` in `store`: 404 unless the run exists there,
/// then the fold — both off the async executor. The fold never fails the
/// request (a failed fold reads as an empty, `partial` result).
async fn local_run_usage(
    store: Arc<RunStore>,
    s: &AppState,
    id: &str,
    q: &RunUsageQuery,
) -> ApiResult<serde_json::Value> {
    {
        let store = Arc::clone(&store);
        let id = id.to_string();
        blocking(move || {
            store
                .load(&id)
                .map(drop)
                .map_err(|e| run_not_found_or_internal(&id, e))
        })
        .await?;
    }
    let u = crate::usage::run_usage_blocking(store, id.to_string()).await;
    run_usage_json(&u, &s.pricing, q)
}

/// `GET /api/runs/:id/usage` on a resolved host. Shared by the explicit
/// `?host=` branch and the resolver's [`RunLocation::Host`] branch. Mirrored
/// transports build from the local mirror; HTTP hosts proxy, forwarding the
/// incremental cursor.
async fn run_usage_from_host(
    s: &AppState,
    host_id: &str,
    id: &str,
    q: &RunUsageQuery,
) -> ApiResult<serde_json::Value> {
    let conn = resolve_host(s, host_id)?;
    if conn.serves_runs_from_local_mirror() {
        return local_run_usage(Arc::clone(&s.run_store), s, id, q).await;
    }
    let mut path = format!("/api/runs/{id}/usage");
    let mut sep = '?';
    if let Some(n) = q.since {
        path.push_str(&format!("{sep}since={n}"));
        sep = '&';
    }
    if let Some(e) = q.epoch {
        path.push_str(&format!("{sep}epoch={e}"));
    }
    conn.proxy_get_json(&path).await.map_err(|e| match e {
        // An older remote CP has no such route: it answers 404, or — through
        // its SPA fallback — a 200 that is not JSON. Both mean "this host
        // cannot serve live usage", which the web degrades on; never a 500.
        // A reply that failed mid-body is a transport failure and stays 5xx.
        HostConnectorError::NotFound(_) | HostConnectorError::NotJson(_) => {
            ApiError::not_found(format!("host {host_id} does not serve usage for run {id}"))
        }
        HostConnectorError::Unreachable(m) => {
            ApiError::internal(format!("host {host_id} unreachable: {m}"))
        }
        other => ApiError::internal(other.to_string()),
    })
}

/// `GET /api/runs/:id/usage[?host=<id>][&since=N&epoch=E]` — the run's live
/// usage (spec 2026-09-29 §5.3): priced summary, per-step summaries, turns,
/// `partial`, and the per-call series. `points` is append-only within an
/// `epoch` (a decimal string on the wire), so a client holding `N` points of
/// epoch `E` gets only the tail (`points_from = N`); any other cursor gets
/// the full series from 0.
///
/// Routed exactly like [`get_run_usage_timeline`]: an explicit
/// `?host=<remote-id>` first; otherwise [`resolve_run_location`] —
/// `Global`/`ProjectLocal` fold the resolved store; `Host` proxies (or reads
/// the local mirror); `Unpersisted` has no artifacts anywhere, so it is a 200
/// with a zero summary and no points; `NotFound` → 404.
async fn get_run_usage(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunUsageQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(host_id) = q.host.as_deref().filter(|h| *h != "local") {
        return run_usage_from_host(&s, host_id, &id, &q).await.map(Json);
    }

    match resolve_run_location(&s, &id).await {
        RunLocation::Global => local_run_usage(Arc::clone(&s.run_store), &s, &id, &q)
            .await
            .map(Json),
        RunLocation::ProjectLocal { path } => {
            let store = Arc::new(RunStore::new(path.join(".rupu").join("runs")));
            local_run_usage(store, &s, &id, &q).await.map(Json)
        }
        RunLocation::Host { host_id } => run_usage_from_host(&s, &host_id, &id, &q).await.map(Json),
        RunLocation::Unpersisted { .. } => {
            run_usage_json(&crate::usage_index::RunUsage::default(), &s.pricing, &q).map(Json)
        }
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
/// - `Io` / `Json` / `TaskFailed` → 500
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
        RunStoreError::TaskFailed(msg) => ApiError::internal(msg),
    }
}

/// Shared guard+delete: refuse to delete a non-terminal ACTIVE run, then
/// delegate to `RunStore::delete` (which itself has no such guard — see its
/// doc comment; archived runs are always terminal, so `load` returning
/// `NotFound` for them just skips the guard).
///
/// Used by both the local branch of `delete_run` below and
/// [`crate::host::local::LocalHostConnector::delete_run`], so the two never
/// diverge on which runs may be removed.
pub(crate) fn delete_run_checked(store: &RunStore, id: &str) -> Result<(), RunStoreError> {
    if let Ok(rec) = store.load(id) {
        if !rec.status.is_terminal() {
            return Err(RunStoreError::NotTerminal(id.to_string()));
        }
    }
    store.delete(id)
}

/// Map a [`HostConnectorError`] from a proxied archive/restore/delete call
/// to an [`ApiError`], mirroring `pause_run`/`resume_run`'s table exactly:
/// `NotFound` → 404, `Invalid` → 409 (non-terminal / already-archived —
/// the same conflict semantics as the local branch's `RunStoreError`
/// mapping), `Unsupported` → 501 (a transport that genuinely can't do this,
/// surfaced as a real error rather than a silent no-op), everything else →
/// 500.
fn map_host_mutate_err(e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Invalid(m) => ApiError::conflict(m),
        // A remote CP-of-CP hop (HttpHostConnector) already mapped ITS OWN
        // local refusal to 409 before it reached us — preserve that status
        // rather than flattening it into a 500 below.
        HostConnectorError::Remote(409, m) => ApiError::conflict(m),
        HostConnectorError::Unsupported(m) => ApiError::not_available(m),
        other => ApiError::internal(other.to_string()),
    }
}

/// `POST /api/runs/:id/archive[?host=<id>]` — move a terminal run to the
/// archive scope.
///
/// Without `?host=` (or `?host=local`): unchanged — non-terminal runs yield
/// 409; the run's directory (including transcripts) is renamed into
/// `<global>/runs-archive/<id>`.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::archive_run`] and
/// returns `{ "ok": true, "id", "archived": true, "host_id": "<id>" }`.
async fn archive_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.archive_run(&id).await.map_err(map_host_mutate_err)?;
        return Ok(Json(
            serde_json::json!({ "ok": true, "id": id, "archived": true, "host_id": host }),
        ));
    }
    s.run_store
        .archive(&id)
        .map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "id": id, "archived": true }),
    ))
}

/// `POST /api/runs/:id/restore[?host=<id>]` — move an archived run back to
/// the active scope.
///
/// Without `?host=` (or `?host=local`): unchanged — 404 if the run is not
/// in the archive.
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::restore_run`] and
/// returns `{ "ok": true, "id", "archived": false, "host_id": "<id>" }`.
async fn restore_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.restore_run(&id).await.map_err(map_host_mutate_err)?;
        return Ok(Json(
            serde_json::json!({ "ok": true, "id": id, "archived": false, "host_id": host }),
        ));
    }
    s.run_store
        .restore(&id)
        .map_err(|e| map_run_store_err(&id, e))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "id": id, "archived": false }),
    ))
}

/// `DELETE /api/runs/:id[?host=<id>]` — permanently remove a run from
/// either scope.
///
/// Without `?host=` (or `?host=local`): unchanged — non-terminal runs in
/// the active scope yield 409 (see [`delete_run_checked`]).
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::delete_run`] and
/// returns `{ "ok": true, "id", "deleted": true, "host_id": "<id>" }`.
async fn delete_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RunControlQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = resolve_host(&s, host)?;
        conn.delete_run(&id).await.map_err(map_host_mutate_err)?;
        return Ok(Json(
            serde_json::json!({ "ok": true, "id": id, "deleted": true, "host_id": host }),
        ));
    }
    delete_run_checked(&s.run_store, &id).map_err(|e| map_run_store_err(&id, e))?;
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
    // An archived run's directory (ledger, events, step results) lives under
    // the archive root, so fold it there — the active store has nothing left.
    let store = RunStore::new(s.run_store.archive_root());
    let pricing = s.pricing.clone();
    let rows = blocking(move || {
        let mut rows = Vec::with_capacity(records.len());
        for r in &records {
            let row = RunListRow::with_usage(r, &store, &pricing);
            let mut v = serde_json::to_value(row).map_err(|e| ApiError::internal(e.to_string()))?;
            v["host_id"] = serde_json::json!("local");
            rows.push(v);
        }
        Ok(rows)
    })
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;
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
            awaiting: Vec::new(),
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
            resume_gate_id: None,
            resume_approver: None,
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            codename: None,
        }
    }

    #[tokio::test]
    async fn approve_awaiting_run_records_the_approval_and_requests_a_runner() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_app", "gate"), "name: x\n")
            .unwrap();

        let resp = approve_run(
            State(s.clone()),
            Path("run_app".into()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
            None,
        )
        .await
        .expect("approve should succeed");
        // The approval is recorded on the run (spec §7) and the marker asks
        // the background worker for the runner that applies it; its only
        // gate decided, the run is no longer parked.
        let body = resp.0;
        assert_eq!(body["run"]["status"], serde_json::json!("running"));
        assert_eq!(body["host_id"], "local");

        let loaded = s.run_store.load("run_app").unwrap();
        assert_eq!(loaded.status, RunStatus::Running);
        assert!(loaded.resume_requested_at.is_some());
        assert_eq!(loaded.awaiting_step_id, None);
        let decided: Vec<(&str, Option<&str>)> = loaded
            .gate_decisions
            .iter()
            .map(|d| (d.step_id.as_str(), d.approver.as_deref()))
            .collect();
        assert_eq!(decided, [("gate", Some("web"))]);
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
            Some(Json(body)),
        )
        .await
        .expect("approve should succeed");

        let loaded = s.run_store.load("run_mode").unwrap();
        assert_eq!(loaded.status, RunStatus::Running);
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
            None,
        )
        .await
        .expect("bodyless approve should succeed");

        let loaded = s.run_store.load("run_nobody").unwrap();
        assert_eq!(loaded.status, RunStatus::Running);
        assert_eq!(loaded.resume_mode, None);
        assert!(loaded.resume_requested_at.is_some());
    }

    // ── Task 5b-2b: per-gate approve/reject (spec §7) ──────────────────────

    /// A 2-gate `AwaitingApproval` run — both `gate_a` and `gate_b`
    /// genuinely parked (not the legacy compat-only shape a 1-gate run
    /// serializes to).
    fn multi_gate_awaiting_record(id: &str) -> RunRecord {
        let mut rec = awaiting_record(id, "gate_a");
        let since = chrono::Utc::now();
        rec.awaiting = vec![
            rupu_orchestrator::runs::AwaitingGate {
                step_id: "gate_a".into(),
                prompt: Some("approve a?".into()),
                since,
                expires_at: None,
            },
            rupu_orchestrator::runs::AwaitingGate {
                step_id: "gate_b".into(),
                prompt: Some("approve b?".into()),
                since,
                expires_at: None,
            },
        ];
        rec.sync_awaiting_compat();
        rec
    }

    #[tokio::test]
    async fn approve_targets_a_specific_gate_on_a_multi_gate_run_leaves_sibling_parked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(
                multi_gate_awaiting_record("run_multi_approve_b"),
                "name: x\n",
            )
            .unwrap();

        let resp = approve_run(
            State(s.clone()),
            Path("run_multi_approve_b".into()),
            Query(RunControlQuery {
                host: None,
                gate: Some("gate_b".into()),
            }),
            None,
        )
        .await
        .expect("approving a named gate on a multi-gate run should succeed");
        // Path-scoped (spec §7): gate_a is still parked, so the run stays
        // AwaitingApproval; gate_b's approval is recorded for the runner the
        // marker asks the background worker for.
        assert_eq!(
            resp.0["run"]["status"],
            serde_json::json!("awaiting_approval")
        );

        let loaded = s.run_store.load("run_multi_approve_b").unwrap();
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
        assert!(loaded.resume_requested_at.is_some());
        // gate_a is unaffected — still parked and approvable.
        assert_eq!(loaded.awaiting.len(), 1);
        assert_eq!(loaded.awaiting[0].step_id, "gate_a");
        assert_eq!(loaded.awaiting_step_id.as_deref(), Some("gate_a"));
        // gate_b's approval is durable, per gate — a second web approval
        // (of gate_a) can't overwrite it the way a single-slot marker could.
        assert_eq!(loaded.gate_decisions.len(), 1);
        assert_eq!(loaded.gate_decisions[0].step_id, "gate_b");
        assert_eq!(loaded.resume_gate_id.as_deref(), Some("gate_b"));
    }

    #[tokio::test]
    async fn approve_with_no_gate_on_a_multi_gate_run_is_conflict_listing_candidates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(multi_gate_awaiting_record("run_multi_ambig"), "name: x\n")
            .unwrap();

        let err = approve_run(
            State(s.clone()),
            Path("run_multi_ambig".into()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
            None,
        )
        .await
        .expect_err("omitting ?gate= on a multi-gate run should be a conflict, not a guess");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
        // The candidate gate ids are listed in the error body so the UI can
        // prompt with them.
        assert!(err.1.contains("gate_a"));
        assert!(err.1.contains("gate_b"));

        // No marker was set — the failed attempt didn't mutate the run.
        let loaded = s.run_store.load("run_multi_ambig").unwrap();
        assert!(loaded.resume_requested_at.is_none());
        assert_eq!(loaded.awaiting.len(), 2);
    }

    #[tokio::test]
    async fn approve_with_unknown_gate_on_multi_gate_run_is_conflict_not_500() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(
                multi_gate_awaiting_record("run_multi_unknown_gate"),
                "name: x\n",
            )
            .unwrap();

        let err = approve_run(
            State(s.clone()),
            Path("run_multi_unknown_gate".into()),
            Query(RunControlQuery {
                host: None,
                gate: Some("does_not_exist".into()),
            }),
            None,
        )
        .await
        .expect_err("naming a gate that isn't parked should be a clean error, not a 500");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);

        let loaded = s.run_store.load("run_multi_unknown_gate").unwrap();
        assert!(loaded.resume_requested_at.is_none());
    }

    #[tokio::test]
    async fn reject_targets_a_specific_gate_on_a_multi_gate_run_leaves_sibling_parked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(
                multi_gate_awaiting_record("run_multi_reject_a"),
                "name: x\n",
            )
            .unwrap();

        let resp = reject_run(
            State(s.clone()),
            Path("run_multi_reject_a".into()),
            Query(RunControlQuery {
                host: None,
                gate: Some("gate_a".into()),
            }),
            Json(RejectBody {
                reason: Some("gate_a looks bad".into()),
            }),
        )
        .await
        .expect("rejecting a named gate on a multi-gate run should succeed");
        // gate_b is still parked, so the run itself is NOT terminal yet.
        assert_eq!(
            resp.0["run"]["status"],
            serde_json::json!("awaiting_approval")
        );

        let loaded = s.run_store.load("run_multi_reject_a").unwrap();
        assert_eq!(loaded.status, RunStatus::AwaitingApproval);
        assert_eq!(loaded.awaiting.len(), 1);
        assert_eq!(loaded.awaiting[0].step_id, "gate_b");
    }

    #[tokio::test]
    async fn reject_last_gate_of_a_multi_gate_run_flips_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(
                multi_gate_awaiting_record("run_multi_reject_both"),
                "name: x\n",
            )
            .unwrap();

        let _ = reject_run(
            State(s.clone()),
            Path("run_multi_reject_both".into()),
            Query(RunControlQuery {
                host: None,
                gate: Some("gate_a".into()),
            }),
            Json(RejectBody { reason: None }),
        )
        .await
        .expect("rejecting gate_a should succeed");
        // Still awaiting: gate_b remains parked.
        assert_eq!(
            s.run_store.load("run_multi_reject_both").unwrap().status,
            RunStatus::AwaitingApproval
        );

        let resp = reject_run(
            State(s.clone()),
            Path("run_multi_reject_both".into()),
            Query(RunControlQuery {
                host: None,
                gate: Some("gate_b".into()),
            }),
            Json(RejectBody {
                reason: Some("gate_b too".into()),
            }),
        )
        .await
        .expect("rejecting the last remaining gate should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("rejected"));

        let loaded = s.run_store.load("run_multi_reject_both").unwrap();
        assert_eq!(loaded.status, RunStatus::Rejected);
        assert!(loaded.awaiting.is_empty());
    }

    /// Single-gate parity: an explicit `?gate=` naming the sole parked gate
    /// on a 1-gate run behaves exactly like omitting it.
    #[tokio::test]
    async fn approve_with_gate_matching_the_sole_gate_is_unchanged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_sole_named", "gate"), "name: x\n")
            .unwrap();

        let resp = approve_run(
            State(s.clone()),
            Path("run_sole_named".into()),
            Query(RunControlQuery {
                host: None,
                gate: Some("gate".into()),
            }),
            None,
        )
        .await
        .expect("naming the sole parked gate should behave like omitting it");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("running"));
        let loaded = s.run_store.load("run_sole_named").unwrap();
        assert_eq!(loaded.awaiting_step_id, None);
        assert_eq!(loaded.gate_decisions.len(), 1);
        assert_eq!(loaded.gate_decisions[0].step_id, "gate");
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
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
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
        .await
        .expect_err("resume on missing run should 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// A remote host whose `get_run` reports `status` and which records
    /// every `resume_run` it is asked for. With `status: None` the host has
    /// no such run — or, with `mirror`, can't answer (a remote predating
    /// `rupu run show`), so its runs are read from the local mirror. Methods
    /// these tests never reach panic rather than no-op.
    #[derive(Default)]
    struct ResumeHostConnector {
        status: Option<&'static str>,
        mirror: bool,
        resumed: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::host::connector::HostConnector for ResumeHostConnector {
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
        async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            match self.status {
                Some(status) => Ok(serde_json::json!({
                    "run": { "id": run_id, "status": status },
                    "steps": [],
                    "usage": {},
                })),
                None if self.mirror => Err(HostConnectorError::Unsupported(
                    "remote host does not support `rupu run show`".into(),
                )),
                None => Err(HostConnectorError::NotFound(run_id.to_string())),
            }
        }
        fn serves_runs_from_local_mirror(&self) -> bool {
            self.mirror
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
        async fn resume_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
            self.resumed.lock().unwrap().push(run_id.to_string());
            Ok(())
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
            unimplemented!("not exercised by this test")
        }
    }

    /// A writable state whose host `host_remote` is `conn` — the same
    /// `HostTransport::Local` injection seam `get_run_host_proxies` uses.
    fn state_with_remote(tmp: &tempfile::TempDir, conn: Arc<ResumeHostConnector>) -> AppState {
        let host_store = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        host_store
            .save(&rupu_workspace::Host {
                id: "host_remote".into(),
                name: "remote".into(),
                transport: rupu_workspace::HostTransport::Local,
                token_hash: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_seen_at: None,
            })
            .unwrap();
        writable_state(tmp).with_hosts(Arc::new(crate::host::registry::HostRegistry::new(
            host_store, conn,
        )))
    }

    async fn resume_on_remote(s: AppState, id: &str) -> ApiResult<Json<serde_json::Value>> {
        resume_run(
            State(s),
            Path(id.into()),
            Query(RunControlQuery {
                host: Some("host_remote".into()),
                gate: None,
            }),
        )
        .await
    }

    /// The remote branch holds a remote run to the local path's rule: only a
    /// `paused` run is resumed. A run that finished (or was cancelled) since
    /// the page offered "Resume" gets a 409, and the host is never asked —
    /// a resume there would retry the finished run and bring it back.
    #[tokio::test]
    async fn resume_remote_run_that_is_not_paused_conflicts_and_is_never_sent() {
        for status in [
            "completed",
            "failed",
            "rejected",
            "cancelled",
            "running",
            "awaiting_approval",
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let conn = Arc::new(ResumeHostConnector {
                status: Some(status),
                ..Default::default()
            });
            let s = state_with_remote(&tmp, Arc::clone(&conn));

            let err = resume_on_remote(s, "run_remote_done")
                .await
                .expect_err("only a paused remote run resumes");

            assert_eq!(err.0, axum::http::StatusCode::CONFLICT, "{status}");
            assert!(
                conn.resumed.lock().unwrap().is_empty(),
                "{status}: the host was asked to resume"
            );
        }
    }

    #[tokio::test]
    async fn resume_remote_paused_run_is_sent_to_its_host() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Arc::new(ResumeHostConnector {
            status: Some("paused"),
            ..Default::default()
        });
        let s = state_with_remote(&tmp, Arc::clone(&conn));

        let resp = resume_on_remote(s, "run_remote_paused")
            .await
            .expect("a paused remote run resumes");

        assert_eq!(resp.0["ok"], true);
        assert_eq!(resp.0["host_id"], "host_remote");
        assert_eq!(
            *conn.resumed.lock().unwrap(),
            vec!["run_remote_paused".to_string()]
        );
    }

    #[tokio::test]
    async fn resume_remote_unknown_run_is_not_found_and_never_sent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Arc::new(ResumeHostConnector::default());
        let s = state_with_remote(&tmp, Arc::clone(&conn));

        let err = resume_on_remote(s, "ghost")
            .await
            .expect_err("resume of a run the host does not have should 404");

        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
        assert!(conn.resumed.lock().unwrap().is_empty());
    }

    /// A host that can't report the run (one predating `rupu run show`)
    /// is checked against the local mirror — and those hosts are exactly
    /// the ones too old for `--if-unfinished`, so this check is all that
    /// stands between a finished run and its retry.
    #[tokio::test]
    async fn resume_remote_run_is_checked_against_the_mirror_when_the_host_cannot_report() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = Arc::new(ResumeHostConnector {
            mirror: true,
            ..Default::default()
        });
        let s = state_with_remote(&tmp, Arc::clone(&conn));
        s.run_store
            .create(terminal_record("run_mirrored_done"), "name: x\n")
            .unwrap();
        s.run_store
            .create(paused_record("run_mirrored_paused", "gate"), "name: x\n")
            .unwrap();

        let err = resume_on_remote(s.clone(), "run_mirrored_done")
            .await
            .expect_err("a mirrored run that finished does not resume");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
        assert!(conn.resumed.lock().unwrap().is_empty());

        let resp = resume_on_remote(s, "run_mirrored_paused")
            .await
            .expect("a mirrored paused run resumes");
        assert_eq!(resp.0["ok"], true);
        assert_eq!(
            *conn.resumed.lock().unwrap(),
            vec!["run_mirrored_paused".to_string()]
        );
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
        let _ = archive_run(
            State(s.clone()),
            Path(id.clone()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
        .await
        .expect("archive ok");
        assert_eq!(s.run_store.list().unwrap().len(), 0);
        assert_eq!(s.run_store.list_archived().unwrap().len(), 1);

        // delete (from archive)
        let _ = delete_run(
            State(s.clone()),
            Path(id.clone()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
        .await
        .expect("delete ok");
        let err = delete_run(
            State(s.clone()),
            Path(id.clone()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
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

    /// An archived run's usage lives under `runs-archive/<id>` (the run dir,
    /// ledger included, moves on archive): the archived list must fold it
    /// there, not against the active store where nothing is left.
    #[tokio::test]
    async fn list_archived_runs_reports_the_archived_runs_usage() {
        use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_01ARCHIVEDUSE"), "name: x\n")
            .unwrap();
        let row = LedgerRow {
            v: LEDGER_VERSION,
            id: "u1".into(),
            at: chrono::Utc::now(),
            kind: LedgerKind::Turn,
            step_id: Some("build".into()),
            unit_index: None,
            unit_key: None,
            agent_run_id: "run_ARCHAGENT".into(),
            parent_agent_run_id: None,
            transcript: PathBuf::from("/nowhere/run_ARCHAGENT.jsonl"),
            agent: "builder".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            input_tokens: 120,
            output_tokens: 30,
            cached_tokens: 0,
            cache_write_tokens: 0,
        };
        let mut line = serde_json::to_vec(&row).unwrap();
        line.push(b'\n');
        std::fs::write(s.run_store.usage_ledger_path("run_01ARCHIVEDUSE"), line).unwrap();
        s.run_store.archive("run_01ARCHIVEDUSE").unwrap();

        let rows = list_archived_runs(State(s), Query(ArchivedQuery { kind: None }))
            .await
            .expect("list_archived_runs should succeed")
            .0;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["usage"]["total_tokens"],
            serde_json::json!(150),
            "{}",
            rows[0]
        );
        assert_eq!(rows[0]["turns"], serde_json::json!(1), "{}", rows[0]);
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
        let err = archive_run(
            State(s.clone()),
            Path("../../etc".into()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
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
        let err = restore_run(
            State(s),
            Path("../../etc".into()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_run_traversal_id_is_bad_request() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = delete_run(
            State(s),
            Path("../../etc".into()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
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
        let err = archive_run(
            State(s.clone()),
            Path(id),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn archive_run_absent_host_matches_explicit_host_local() {
        // Back-compat proof: an absent `?host=` param and an explicit
        // `?host=local` must hit the exact same local branch and produce the
        // exact same response shape (no `host_id` key injected — unlike the
        // remote branch).
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_01ABSENT"), "name: x\n")
            .unwrap();
        s.run_store
            .create(terminal_record("run_01EXPLICITLOCAL"), "name: x\n")
            .unwrap();

        let absent = archive_run(
            State(s.clone()),
            Path("run_01ABSENT".into()),
            Query(RunControlQuery {
                host: None,
                gate: None,
            }),
        )
        .await
        .expect("absent host should archive locally")
        .0;
        let explicit_local = archive_run(
            State(s.clone()),
            Path("run_01EXPLICITLOCAL".into()),
            Query(RunControlQuery {
                host: Some("local".into()),
                gate: None,
            }),
        )
        .await
        .expect("host=local should archive locally")
        .0;

        assert_eq!(absent["archived"], serde_json::json!(true));
        assert_eq!(absent.get("host_id"), None);
        assert_eq!(explicit_local["archived"], serde_json::json!(true));
        assert_eq!(explicit_local.get("host_id"), None);
        assert_eq!(s.run_store.list_archived().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn archive_run_unknown_host_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = archive_run(
            State(s),
            Path("run_01UNKNOWNHOST".into()),
            Query(RunControlQuery {
                host: Some("host_nonexistent".into()),
                gate: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
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
            codename: "cobalt-harbor".into(),
            codename_derived: false,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("usage").is_some());
        assert_eq!(v["usage"]["priced"], serde_json::Value::Bool(false));
        assert!(v.get("turns").is_some());
        assert!(v.get("duration_ms").is_some());
    }

    #[test]
    fn run_list_row_and_detail_codename_stored_else_derived() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        // Legacy run: no stored codename -> derived + flagged.
        let mut legacy = terminal_record("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W");
        legacy.codename = None;
        let row = RunListRow::from(&legacy);
        assert_eq!(row.codename, "jade-reef");
        assert!(row.codename_derived);
        s.run_store.create(legacy, "name: x\n").unwrap();
        let d =
            query_run_detail(&s.run_store, "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", &s.pricing).unwrap();
        assert_eq!(d["run"]["codename"], "jade-reef");
        assert_eq!(d["run"]["codename_derived"], true);

        // Stored codename wins.
        let mut named = terminal_record("run_01NAMED");
        named.codename = Some("cobalt-harbor".into());
        let row = RunListRow::from(&named);
        assert_eq!(row.codename, "cobalt-harbor");
        assert!(!row.codename_derived);
        s.run_store.create(named, "name: x\n").unwrap();
        let d = query_run_detail(&s.run_store, "run_01NAMED", &s.pricing).unwrap();
        assert_eq!(d["run"]["codename"], "cobalt-harbor");
        assert_eq!(d["run"]["codename_derived"], false);
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
    /// without any real network. `get_run`/`proxy_get_json` answer
    /// `run_json` itself and the list methods answer rows from its keys
    /// (`runs`, `agent_runs`, `sessions`, `autoflow_runs`,
    /// `autoflow_events`); every other method panics loudly if accidentally
    /// called, rather than silently no-opping.
    pub(crate) struct FakeHostConnector {
        pub(crate) run_json: serde_json::Value,
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
        /// Rows come from `run_json["runs"]` when present; otherwise the
        /// host answers as unreachable (exercises the 502 mapping).
        async fn list_runs(
            &self,
            _params: RunListQuery,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            match self.run_json.get("runs").and_then(|v| v.as_array()) {
                Some(rows) => Ok(rows.clone()),
                None => Err(HostConnectorError::Unreachable(
                    "fake host: no runs scripted".into(),
                )),
            }
        }
        async fn get_run(&self, _run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
            Ok(self.run_json.clone())
        }
        /// Rows for agent-run listing come from `run_json["agent_runs"]`
        /// when present (else the trait default `Unsupported`).
        async fn list_agent_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            match self.run_json.get("agent_runs").and_then(|v| v.as_array()) {
                Some(rows) => Ok(rows.clone()),
                None => Err(HostConnectorError::Unsupported("agent-run listing".into())),
            }
        }
        /// Rows for session listing come from `run_json["sessions"]` when
        /// present (else `Unsupported`, as the trait default answers).
        async fn list_sessions(
            &self,
            _scope: Option<&str>,
        ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            match self.run_json.get("sessions").and_then(|v| v.as_array()) {
                Some(rows) => Ok(rows.clone()),
                None => Err(HostConnectorError::Unsupported("session listing".into())),
            }
        }
        /// Rows for autoflow-cycle listing come from
        /// `run_json["autoflow_runs"]` when present (else `Unsupported`).
        async fn list_autoflow_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            match self
                .run_json
                .get("autoflow_runs")
                .and_then(|v| v.as_array())
            {
                Some(rows) => Ok(rows.clone()),
                None => Err(HostConnectorError::Unsupported(
                    "autoflow-run listing".into(),
                )),
            }
        }
        /// Rows for autoflow-event listing come from
        /// `run_json["autoflow_events"]` when present (else `Unsupported`).
        async fn list_autoflow_events(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
            match self
                .run_json
                .get("autoflow_events")
                .and_then(|v| v.as_array())
            {
                Some(rows) => Ok(rows.clone()),
                None => Err(HostConnectorError::Unsupported(
                    "autoflow-event listing".into(),
                )),
            }
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
        async fn unit_coverage(&self, _run_id: &str) -> Result<Vec<u8>, HostConnectorError> {
            Ok(Vec::new())
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            Ok(self.run_json.clone())
        }
    }

    #[test]
    fn host_list_error_maps_connector_failures_to_honest_statuses() {
        use axum::http::StatusCode;
        let cases = [
            (
                HostConnectorError::Unsupported("x".into()),
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                HostConnectorError::Invalid("x".into()),
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                HostConnectorError::NotFound("x".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                HostConnectorError::Unreachable("x".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (HostConnectorError::Unauthorized, StatusCode::BAD_GATEWAY),
            (
                HostConnectorError::Remote(500, "x".into()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                HostConnectorError::NotJson("x".into()),
                StatusCode::BAD_GATEWAY,
            ),
        ];
        for (err, want) in cases {
            let label = err.to_string();
            assert_eq!(host_list_error(err).0, want, "{label}");
        }
    }

    /// An `AppState` whose registry resolves `host_fake` to a
    /// [`FakeHostConnector`] scripted with `run_json` (same seam as
    /// `get_run_proxies_to_host_when_resolver_says_host`: a `Local`-transport
    /// entry under a distinct id resolves to the injected connector).
    pub(crate) fn state_with_fake_host(
        tmp: &tempfile::TempDir,
        run_json: serde_json::Value,
    ) -> AppState {
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
        let fake: Arc<dyn crate::host::connector::HostConnector> =
            Arc::new(FakeHostConnector { run_json });
        test_state(tmp).with_hosts(Arc::new(crate::host::registry::HostRegistry::new(
            host_store, fake,
        )))
    }

    #[tokio::test]
    async fn single_remote_host_run_lists_report_an_unreachable_host_as_502() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No "runs" scripted → the fake's list_runs answers Unreachable.
        let s = state_with_fake_host(&tmp, serde_json::json!({}));
        let err = list_runs(
            State(s.clone()),
            Query(RunsListQuery {
                offset: None,
                limit: None,
                host: Some("host_fake".into()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
        let err = list_workflow_runs(
            State(s),
            Query(WorkflowRunsQuery {
                offset: None,
                limit: None,
                lifecycle: None,
                host: Some("host_fake".into()),
                since: None,
                until: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_GATEWAY);
    }

    /// The only 404 left on a single-host list path: a host id that is not in
    /// the registry (the host was removed). Connector `NotFound` is 502.
    #[tokio::test]
    async fn single_remote_host_run_list_for_an_unregistered_host_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = state_with_fake_host(&tmp, serde_json::json!({}));
        let err = list_runs(
            State(s),
            Query(RunsListQuery {
                offset: None,
                limit: None,
                host: Some("host_missing".into()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
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
        // The proxied payload is unchanged apart from the derived codename
        // this coordinator fills in for an older remote.
        let mut got = resp.0;
        assert_eq!(got["run"]["codename_derived"], true);
        assert!(got["run"]["codename"].is_string());
        got["run"].as_object_mut().unwrap().remove("codename");
        got["run"]
            .as_object_mut()
            .unwrap()
            .remove("codename_derived");
        assert_eq!(got, fake_run_json);
    }

    #[tokio::test]
    async fn get_run_from_host_injects_derived_codename_for_older_remote() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mk = |run: serde_json::Value| {
            let fake: Arc<dyn crate::host::connector::HostConnector> =
                Arc::new(FakeHostConnector {
                    run_json: serde_json::json!({"run": run, "steps": [], "usage": {}}),
                });
            test_state(&tmp).with_hosts(Arc::new(crate::host::registry::HostRegistry::new(
                rupu_workspace::HostStore {
                    root: tmp.path().join("hosts"),
                },
                fake,
            )))
        };
        // Older remote: no codename -> derived + flagged.
        let s = mk(serde_json::json!({"id": "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W"}));
        let d = get_run_from_host(&s, "local", "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W")
            .await
            .unwrap();
        assert_eq!(d["run"]["codename"], "jade-reef");
        assert_eq!(d["run"]["codename_derived"], true);
        // Newer remote: stored name kept, not derived.
        let s = mk(serde_json::json!({"id": "run_x", "codename": "cobalt-harbor"}));
        let d = get_run_from_host(&s, "local", "run_x").await.unwrap();
        assert_eq!(d["run"]["codename"], "cobalt-harbor");
        assert_eq!(d["run"]["codename_derived"], false);
    }

    /// Fake `HostConnector` that mimics an SSH host whose remote `rupu`
    /// cannot answer `get_run` yet (no `rupu run show` support, or the
    /// remote record isn't written yet) but whose runs are mirrored into
    /// the coordinator's own `RunStore` — the seam Fix A exercises.
    struct MirrorOnlyHostConnector;

    #[async_trait::async_trait]
    impl crate::host::connector::HostConnector for MirrorOnlyHostConnector {
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
            Err(HostConnectorError::Unsupported("no run show".into()))
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
        async fn unit_coverage(&self, _run_id: &str) -> Result<Vec<u8>, HostConnectorError> {
            Ok(Vec::new())
        }
        async fn proxy_get_json(
            &self,
            _path_and_query: &str,
        ) -> Result<serde_json::Value, HostConnectorError> {
            unimplemented!("not exercised by this test")
        }
        fn serves_runs_from_local_mirror(&self) -> bool {
            true
        }
    }

    /// Register `MirrorOnlyHostConnector` under host id `host_mirror` in a
    /// fresh `HostRegistry`, mirroring the injection seam
    /// `get_run_host_proxies` uses for `FakeHostConnector`.
    fn mirror_only_registry(tmp: &tempfile::TempDir) -> Arc<crate::host::registry::HostRegistry> {
        let conn: Arc<dyn crate::host::connector::HostConnector> =
            Arc::new(MirrorOnlyHostConnector);
        let host_store = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        host_store
            .save(&rupu_workspace::Host {
                id: "host_mirror".into(),
                name: "mirror".into(),
                transport: rupu_workspace::HostTransport::Local,
                token_hash: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_seen_at: None,
            })
            .unwrap();
        Arc::new(crate::host::registry::HostRegistry::new(host_store, conn))
    }

    /// A `RunRecord` shaped like `NodeMirror::create_run`'s mirror record
    /// (see `crates/rupu-cp/src/node/mirror.rs`) for a run mirrored from
    /// `host_mirror`.
    fn mirrored_record(id: &str) -> RunRecord {
        RunRecord {
            id: id.into(),
            workflow_name: "wf".into(),
            status: RunStatus::Running,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: String::new(),
            workspace_path: PathBuf::from("."),
            transcript_dir: PathBuf::from("/tmp/mirrored"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: Some("host_mirror".into()),
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
            resume_gate_id: None,
            resume_approver: None,
            resume_rerequested_at: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
            gate_decisions: Vec::new(),
            codename: None,
        }
    }

    /// Fix A: when the remote can't answer `get_run` (e.g. an SSH host that
    /// doesn't support `rupu run show`, or hasn't written the record yet)
    /// but the run is mirrored locally, `get_run_from_host` must serve the
    /// mirrored record instead of surfacing the connector error.
    #[tokio::test]
    async fn get_run_host_falls_back_to_local_mirror_on_connector_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_hosts(mirror_only_registry(&tmp));
        s.run_store
            .create(mirrored_record("run_01MIRRORED"), "")
            .unwrap();

        let resp = get_run(
            State(s),
            Path("run_01MIRRORED".into()),
            Query(RunDetailQuery {
                host: Some("host_mirror".into()),
            }),
        )
        .await
        .expect("a mirrored run must be served from the local mirror when the remote errors");
        assert_eq!(resp.0["run"]["id"], serde_json::json!("run_01MIRRORED"));
    }

    /// One serialized ledger row (`input`/`output` tokens) for `run_id`.
    fn write_ledger_row(store: &RunStore, run_id: &str, input: u64, output: u64) {
        write_ledger_row_with_cache_write(store, run_id, input, output, 0);
    }

    /// [`write_ledger_row`] with `cache_write` prompt tokens written to the
    /// provider's cache.
    fn write_ledger_row_with_cache_write(
        store: &RunStore,
        run_id: &str,
        input: u64,
        output: u64,
        cache_write: u64,
    ) {
        use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
        let row = LedgerRow {
            v: LEDGER_VERSION,
            id: format!("{run_id}_u1"),
            at: chrono::Utc::now(),
            kind: LedgerKind::Turn,
            step_id: Some("build".into()),
            unit_index: None,
            unit_key: None,
            agent_run_id: format!("{run_id}_agent"),
            parent_agent_run_id: None,
            transcript: PathBuf::from(format!("/nowhere/{run_id}_agent.jsonl")),
            agent: "builder".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: cache_write,
        };
        let mut line = serde_json::to_vec(&row).unwrap();
        line.push(b'\n');
        std::fs::write(store.usage_ledger_path(run_id), line).unwrap();
    }

    /// `?host=<mirrored transport>` builds the live usage from the local
    /// mirror (the mirror-only connector's `proxy_get_json` panics).
    #[tokio::test]
    async fn run_usage_host_mirror_builds_from_the_local_store() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_hosts(mirror_only_registry(&tmp));
        s.run_store
            .create(mirrored_record("run_01MIRRUSAGE"), "")
            .unwrap();
        write_ledger_row(&s.run_store, "run_01MIRRUSAGE", 100, 50);

        let v = get_run_usage(
            State(s),
            Path("run_01MIRRUSAGE".into()),
            Query(RunUsageQuery {
                host: Some("host_mirror".into()),
                ..RunUsageQuery::default()
            }),
        )
        .await
        .expect("a mirrored run's usage must be built from the local store")
        .0;
        assert_eq!(v["summary"]["total_tokens"], serde_json::json!(150), "{v}");
        assert_eq!(v["steps"]["build"]["input_tokens"], serde_json::json!(100));
        assert!(v["epoch"].is_string(), "{v}");
        assert_eq!(v["points"].as_array().map(Vec::len), Some(1), "{v}");
    }

    /// The live endpoint reports cache writes in the run summary, each step
    /// summary, and every series point.
    #[tokio::test]
    async fn run_usage_reports_cache_write_tokens() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_01CACHEWRITE"), "name: x\n")
            .unwrap();
        write_ledger_row_with_cache_write(&s.run_store, "run_01CACHEWRITE", 1000, 20, 30);

        let v = get_run_usage(
            State(s),
            Path("run_01CACHEWRITE".into()),
            Query(RunUsageQuery::default()),
        )
        .await
        .expect("a local run's usage")
        .0;
        assert_eq!(
            v["summary"]["cache_write_tokens"],
            serde_json::json!(30),
            "{v}"
        );
        assert_eq!(
            v["steps"]["build"]["cache_write_tokens"],
            serde_json::json!(30),
            "{v}"
        );
        assert_eq!(
            v["points"][0]["tokens_cache_write"],
            serde_json::json!(30),
            "{v}"
        );
        // Cache writes are a subset of input: the total is input + output
        // (1000 + 20), never input + output + cache writes.
        assert_eq!(v["summary"]["total_tokens"], serde_json::json!(1020), "{v}");
        assert_eq!(
            v["steps"]["build"]["total_tokens"],
            serde_json::json!(1020),
            "{v}"
        );
    }

    /// Same mirror rule for the usage-timeline (now built off the executor).
    #[tokio::test]
    async fn run_usage_timeline_host_mirror_builds_from_the_local_store() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_hosts(mirror_only_registry(&tmp));
        s.run_store
            .create(mirrored_record("run_01MIRRTL"), "")
            .unwrap();
        write_ledger_row(&s.run_store, "run_01MIRRTL", 7, 3);

        let v = get_run_usage_timeline(
            State(s),
            Path("run_01MIRRTL".into()),
            Query(RunDetailQuery {
                host: Some("host_mirror".into()),
            }),
        )
        .await
        .expect("a mirrored run's timeline must be built from the local store")
        .0;
        let points = v.as_array().expect("array");
        assert_eq!(points.len(), 1, "{v}");
        assert_eq!(points[0]["tokens_in"], serde_json::json!(7));
    }

    /// A mirrored host whose run is not in the mirror → 404, not a panic/500.
    #[tokio::test]
    async fn run_usage_host_mirror_unknown_run_is_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_hosts(mirror_only_registry(&tmp));
        let err = get_run_usage(
            State(s),
            Path("run_01NOTMIRRORED".into()),
            Query(RunUsageQuery {
                host: Some("host_mirror".into()),
                ..RunUsageQuery::default()
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    /// An unpersisted autoflow run has no artifacts anywhere: a 200 with a
    /// zero summary and an empty series (as the usage-timeline does).
    #[tokio::test]
    async fn run_usage_unpersisted_is_an_empty_200() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        write_cycle_with_run(
            &tmp,
            "2026-07-01",
            "afc_unpersisted_usage",
            "run_01UNPERSISTEDUSE",
            "blocked",
            "github:Section9Labs/rupu/issues/42",
            "issue-supervisor-dispatch",
            Some("401 invalid x-api-key"),
            None,
        );
        let v = get_run_usage(
            State(s),
            Path("run_01UNPERSISTEDUSE".into()),
            Query(RunUsageQuery::default()),
        )
        .await
        .expect("an unpersisted run has no usage, not a 404")
        .0;
        assert_eq!(v["summary"]["total_tokens"], serde_json::json!(0), "{v}");
        assert_eq!(v["points_from"], serde_json::json!(0), "{v}");
        assert_eq!(v["points"], serde_json::json!([]), "{v}");
        assert!(v["epoch"].is_string(), "{v}");
    }

    /// Sibling of the above: when the run is NOT in the local mirror either,
    /// the connector error must still surface (same status mapping as
    /// today) rather than being swallowed into a misleading success.
    #[tokio::test]
    async fn get_run_host_surfaces_connector_error_when_not_mirrored_locally() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp).with_hosts(mirror_only_registry(&tmp));

        let err = get_run(
            State(s),
            Path("run_never_mirrored".into()),
            Query(RunDetailQuery {
                host: Some("host_mirror".into()),
            }),
        )
        .await
        .expect_err("no local mirror record exists, so the connector error must surface");
        assert_eq!(err.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
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

    // Minor finding: a remote CP-of-CP hop's own 409 refusal (HttpHostConnector
    // already turned it into `Remote(409, body)`) must surface as 409 here
    // too, not fall through to the generic 500 `other` arm.
    #[test]
    fn map_host_mutate_err_preserves_remote_409_as_conflict() {
        let err = map_host_mutate_err(HostConnectorError::Remote(409, "not terminal".into()));
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[test]
    fn map_host_mutate_err_still_500s_other_remote_statuses() {
        let err = map_host_mutate_err(HostConnectorError::Remote(500, "boom".into()));
        assert_eq!(err.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ── Date-range filtering (perf & interaction arc, Plan 5 Task 5) ─────────

    /// A run record with an explicit `started_at`, otherwise identical to
    /// `terminal_record` — the "in range or not" fixture below only cares
    /// about the timestamp, not the lifecycle status.
    fn record_started_at(id: &str, started_at: chrono::DateTime<chrono::Utc>) -> RunRecord {
        let mut rec = terminal_record(id);
        rec.started_at = started_at;
        rec
    }

    #[test]
    fn query_run_rows_since_until_filters_before_pagination() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let d = |day: u32| {
            chrono::Utc
                .with_ymd_and_hms(2026, 8, day, 12, 0, 0)
                .unwrap()
        };
        s.run_store
            .create(record_started_at("run_aug_01", d(1)), "name: x\n")
            .unwrap();
        s.run_store
            .create(record_started_at("run_aug_10", d(10)), "name: x\n")
            .unwrap();
        s.run_store
            .create(record_started_at("run_aug_20", d(20)), "name: x\n")
            .unwrap();

        let range = crate::pagination::DateRangeQuery {
            since: Some("2026-08-05T00:00:00Z".into()),
            until: Some("2026-08-15T00:00:00Z".into()),
        };
        let rows = query_run_rows(&s.run_store, 0, 20, None, false, None, &s.pricing, &range)
            .expect("query_run_rows ok");
        assert_eq!(rows.len(), 1, "only run_aug_10 falls in [Aug 5, Aug 15]");
        assert_eq!(rows[0].id, "run_aug_10");

        // No bounds at all → every run passes, same as before this task.
        let unbounded = crate::pagination::DateRangeQuery::default();
        let rows = query_run_rows(
            &s.run_store,
            0,
            20,
            None,
            false,
            None,
            &s.pricing,
            &unbounded,
        )
        .expect("query_run_rows ok");
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn list_workflow_runs_local_host_honors_since_until_bounds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);

        let d = |day: u32| {
            chrono::Utc
                .with_ymd_and_hms(2026, 8, day, 12, 0, 0)
                .unwrap()
        };
        s.run_store
            .create(record_started_at("run_early", d(1)), "name: x\n")
            .unwrap();
        s.run_store
            .create(record_started_at("run_mid", d(10)), "name: x\n")
            .unwrap();

        let Json(rows) = list_workflow_runs(
            State(s.clone()),
            Query(WorkflowRunsQuery {
                offset: None,
                limit: None,
                lifecycle: None,
                host: Some("local".into()),
                since: Some("2026-08-05T00:00:00Z".into()),
                until: None,
            }),
        )
        .await
        .expect("list_workflow_runs ok");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], serde_json::json!("run_mid"));
        assert_eq!(rows[0]["host_id"], serde_json::json!("local"));
    }

    #[tokio::test]
    async fn list_workflow_runs_bad_since_degrades_to_unfiltered_not_500() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(terminal_record("run_x"), "name: x\n")
            .unwrap();

        let Json(rows) = list_workflow_runs(
            State(s),
            Query(WorkflowRunsQuery {
                offset: None,
                limit: None,
                lifecycle: None,
                host: Some("local".into()),
                since: Some("not-a-real-timestamp".into()),
                until: None,
            }),
        )
        .await
        .expect("a malformed since must degrade, never error");

        assert_eq!(rows.len(), 1, "unparseable since imposes no bound");
    }
}
