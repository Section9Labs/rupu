use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse as _, Response},
    routing::{get, post},
    Json, Router,
};
use rupu_orchestrator::{
    runs::{CancelError, CancelOutcome},
    ApprovalError, RunRecord, RunStatus, RunStoreError,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/workflows", get(list_workflow_runs))
        .route("/api/runs/:id", get(get_run))
        .route("/api/runs/:id/log", get(get_run_log))
        .route("/api/runs/:id/usage-timeline", get(get_run_usage_timeline))
        .route("/api/runs/:id/approve", post(approve_run))
        .route("/api/runs/:id/reject", post(reject_run))
        .route("/api/runs/:id/cancel", post(cancel_run))
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

/// Optional body for `POST /api/runs/:id/approve`. `mode` selects the
/// permission mode (`ask` / `bypass` / `readonly`) the resumed run runs
/// under; an absent/empty body leaves it `None` (worker default).
#[derive(serde::Deserialize, Default)]
struct ApproveBody {
    #[serde(default)]
    mode: Option<String>,
}

/// `POST /api/runs/:id/approve` — record a web approval decision for a paused
/// (awaiting-approval) run. Sets the `resume_requested_at` marker (and the
/// optional `resume_mode`) that a background worker picks up to re-enter the
/// workflow; this endpoint does NOT execute the run itself. The run stays
/// `AwaitingApproval` until the worker approves+resumes it.
///
/// The JSON body is optional — a bodyless POST is accepted and treated as
/// `mode = None`.
async fn approve_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<ApproveBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = chrono::Utc::now();
    let mode = body.and_then(|b| b.0.mode);
    s.run_store
        .request_resume_approval(&id, "web", mode.as_deref(), now)
        .map_err(|e| map_approval_err(&id, e))?;
    run_response(&s, &id)
}

#[derive(serde::Deserialize)]
struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

/// `POST /api/runs/:id/reject` — record a web rejection decision for a paused
/// run, transitioning it to the terminal `Rejected` state.
async fn reject_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RejectBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = chrono::Utc::now();
    let reason = body.reason.unwrap_or_default();
    s.run_store
        .reject(&id, "web", &reason, now)
        .map_err(|e| map_approval_err(&id, e))?;
    run_response(&s, &id)
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

/// `POST /api/runs/:id/cancel` — cancel an in-flight run. A `Pending`/`Running`
/// run is marked `Cancelled` (and its live runner TERM'd); a run paused at an
/// approval gate is rejected. Terminal runs yield 409. The JSON body is
/// optional — a bodyless POST uses the default reason.
async fn cancel_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CancelBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let now = chrono::Utc::now();
    let reason = body
        .and_then(|b| b.0.reason)
        .unwrap_or_else(|| "Cancelled from control plane".to_string());
    let _outcome: CancelOutcome = s
        .run_store
        .cancel(&id, "web", &reason, now)
        .map_err(|e| map_cancel_err(&id, e))?;
    run_response(&s, &id)
}

/// Derive a trigger label from a [`RunRecord`].
///
/// - `"event"` — an SCM/webhook event fired the run (`event` field is set).
/// - `"cron"` — a polled-event / cron wake fired it (`source_wake_id` is set).
/// - `"manual"` — direct / manual dispatch (neither field is set).
pub(crate) fn trigger_of(r: &RunRecord) -> &'static str {
    if r.event.is_some() {
        "event"
    } else if r.source_wake_id.is_some() {
        "cron"
    } else {
        "manual"
    }
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

async fn list_runs(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    let mut runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let page_runs = crate::pagination::paginate(runs, &page);
    Ok(Json(
        page_runs
            .iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
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
            RunStatus::Running | RunStatus::Pending | RunStatus::AwaitingApproval
        ),
        Some("completed") => matches!(status, RunStatus::Completed),
        Some("failed") => matches!(
            status,
            RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled
        ),
        _ => true,
    }
}

/// `GET /api/runs/workflows` — manual/direct runs only (no event or cron wake).
async fn list_workflow_runs(
    State(s): State<AppState>,
    Query(q): Query<WorkflowRunsQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    let lifecycle = q.lifecycle.as_deref();
    let mut runs: Vec<RunRecord> = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .filter(|r| r.event.is_none() && r.source_wake_id.is_none())
        .filter(|r| in_lifecycle(r.status, lifecycle))
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let page_runs = crate::pagination::paginate(runs, &q.page());
    Ok(Json(
        page_runs
            .iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
}

async fn get_run(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let record = s.run_store.load(&id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let steps = s.run_store.read_step_results(&id).unwrap_or_default();
    let usage = crate::usage::summarize_run(&s.run_store, &id, &s.pricing);
    Ok(Json(
        serde_json::json!({ "run": record, "steps": steps, "usage": usage }),
    ))
}

/// `GET /api/runs/:id/log` — tail the run's `events.jsonl` as a live SSE stream.
///
/// Returns 404 if the run does not exist. The stream stays open while the run
/// is in progress and emits each [`rupu_orchestrator::executor::Event`] as a
/// JSON `data:` line.
async fn get_run_log(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    // Verify the run exists before opening the tail.
    s.run_store.load(&id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;

    let events_path = s.run_store.events_path(&id);
    let sse = crate::sse::tail_events_sse(events_path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(sse.into_response())
}

/// `GET /api/runs/:id/usage-timeline` — ordered per-turn token series across
/// every transcript the run produced (step results + fan-out items), labeled
/// by step id.
async fn get_run_usage_timeline(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<crate::usage::TurnPoint>>> {
    s.run_store.load(&id).map_err(|e| match e {
        RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let steps = s.run_store.read_step_results(&id).unwrap_or_default();
    let mut labeled: Vec<(String, std::path::PathBuf)> = Vec::new();
    for st in &steps {
        labeled.push((st.step_id.clone(), st.transcript_path.clone()));
        for item in &st.items {
            labeled.push((st.step_id.clone(), item.transcript_path.clone()));
        }
    }
    Ok(Json(crate::usage::turn_series(&labeled)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::RunStore;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Build an `AppState` backed by a fresh tempdir run store.
    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        let store = RunStore::new(tmp.path().join("runs"));
        AppState {
            global_dir: tmp.path().to_path_buf(),
            workspace_dir: tmp.path().to_path_buf(),
            run_store: Arc::new(store),
            pricing: rupu_config::PricingConfig::default(),
            launcher: None,
            session_sender: None,
        }
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
        }
    }

    #[tokio::test]
    async fn approve_awaiting_run_sets_resume_marker_and_stays_awaiting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        s.run_store
            .create(awaiting_record("run_app", "gate"), "name: x\n")
            .unwrap();

        let resp = approve_run(State(s.clone()), Path("run_app".into()), None)
            .await
            .expect("approve should succeed");
        // Marker-only: the endpoint records the approval but leaves the
        // run AwaitingApproval for the background worker to approve+resume.
        let body = resp.0;
        assert_eq!(body["run"]["status"], serde_json::json!("awaiting_approval"));

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
        let resp = reject_run(State(s.clone()), Path("run_rej".into()), Json(body))
            .await
            .expect("reject should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("rejected"));

        let loaded = s.run_store.load("run_rej").unwrap();
        assert_eq!(loaded.status, RunStatus::Rejected);
        assert_eq!(
            loaded.error_message.as_deref(),
            Some("rejected: not safe")
        );
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

        let err = approve_run(State(s), Path("run_done".into()), None)
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
        let _ = approve_run(State(s.clone()), Path("run_mode".into()), Some(Json(body)))
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

        let _ = approve_run(State(s.clone()), Path("run_nobody".into()), None)
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

        let resp = cancel_run(State(s.clone()), Path("run_cancel".into()), None)
            .await
            .expect("cancel should succeed");
        assert_eq!(resp.0["run"]["status"], serde_json::json!("cancelled"));

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
        let err = cancel_run(State(s), Path("run_term".into()), Some(Json(body)))
            .await
            .expect_err("cancel on completed run should fail");
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn cancel_unknown_run_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp);
        let err = cancel_run(State(s), Path("ghost".into()), None)
            .await
            .expect_err("cancel on missing run should 404");
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
}
