use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    response::{IntoResponse as _, Response},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::{RunRecord, RunStatus, RunStoreError};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/workflows", get(list_workflow_runs))
        .route("/api/runs/:id", get(get_run))
        .route("/api/runs/:id/log", get(get_run_log))
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
        }
    }
}

async fn list_runs(State(s): State<AppState>) -> ApiResult<Json<Vec<RunListRow>>> {
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(runs.iter().map(RunListRow::from).collect()))
}

/// `GET /api/runs/workflows` — manual/direct runs only (no event or cron wake).
async fn list_workflow_runs(State(s): State<AppState>) -> ApiResult<Json<Vec<RunListRow>>> {
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let rows: Vec<RunListRow> = runs
        .iter()
        .filter(|r| r.event.is_none() && r.source_wake_id.is_none())
        .map(RunListRow::from)
        .collect();
    Ok(Json(rows))
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
    Ok(Json(serde_json::json!({ "run": record, "steps": steps })))
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
