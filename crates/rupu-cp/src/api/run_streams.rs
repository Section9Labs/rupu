use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_runtime::{AutoflowCycleRecord, AutoflowHistoryStore, AutoflowHistoryStoreError};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/runs/autoflows", get(list_autoflow_runs))
}

/// Slim DTO returned for each autoflow cycle.
///
/// A *cycle* is a single batch tick of the autoflow worker — it may have
/// dispatched zero or more workflow runs. `run_ids` collects every `run_id`
/// found inside the cycle's embedded events (those that carry one).
#[derive(serde::Serialize)]
struct AutoflowCycleRow {
    cycle_id: String,
    mode: String,
    worker_name: Option<String>,
    started_at: String,
    finished_at: String,
    workflow_count: usize,
    ran_cycles: usize,
    skipped_cycles: usize,
    failed_cycles: usize,
    run_ids: Vec<String>,
}

impl From<AutoflowCycleRecord> for AutoflowCycleRow {
    fn from(r: AutoflowCycleRecord) -> Self {
        // Harvest every distinct run_id from the cycle's embedded event list.
        let mut run_ids: Vec<String> = r
            .events
            .iter()
            .filter_map(|e| e.run_id.clone())
            .collect();
        run_ids.dedup();

        // Stringify the mode enum via serde's snake_case tag.
        let mode = serde_json::to_value(r.mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{:?}", r.mode).to_lowercase());

        Self {
            cycle_id: r.cycle_id,
            mode,
            worker_name: r.worker_name,
            started_at: r.started_at,
            finished_at: r.finished_at,
            workflow_count: r.workflow_count,
            ran_cycles: r.ran_cycles,
            skipped_cycles: r.skipped_cycles,
            failed_cycles: r.failed_cycles,
            run_ids,
        }
    }
}

/// `GET /api/runs/autoflows` — returns the most-recent autoflow cycle records.
///
/// The store root matches the CLI canonical path: `<global_dir>/autoflows/history`.
/// A missing store directory is treated as "no cycles yet" and returns `[]`.
async fn list_autoflow_runs(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<AutoflowCycleRow>>> {
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let records = match store.list_recent(100) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };

    Ok(Json(records.into_iter().map(AutoflowCycleRow::from).collect()))
}
