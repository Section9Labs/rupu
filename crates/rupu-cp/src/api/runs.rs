use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
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

/// `GET /api/runs/workflows` — manual/direct runs only (no event or cron wake).
async fn list_workflow_runs(
    State(s): State<AppState>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    let mut runs: Vec<RunRecord> = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .filter(|r| r.event.is_none() && r.source_wake_id.is_none())
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let page_runs = crate::pagination::paginate(runs, &page);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
