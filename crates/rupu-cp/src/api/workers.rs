use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::State, routing::get, Json, Router};
use rupu_orchestrator::RunStatus;
use rupu_runtime::WorkerRecord;
use rupu_workspace::worker_store::WorkerStore;
use std::collections::HashMap;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/workers", get(list_workers))
}

/// A [`WorkerRecord`] enriched with run-activity attribution.
///
/// A worker is a *local execution identity* (not a per-run object), so the raw
/// record alone tells the user little about what it's currently doing. The
/// extra fields summarize the runs whose `worker_id` matches this worker:
/// - `active_run_count` — runs currently Running / Pending / AwaitingApproval
/// - `total_run_count`  — every run attributed to this worker
/// - `last_run_at`      — most recent run `started_at` (None when it has none)
#[derive(serde::Serialize)]
struct WorkerView {
    #[serde(flatten)]
    record: WorkerRecord,
    active_run_count: u64,
    total_run_count: u64,
    last_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Per-worker run tally accumulated while scanning the run store.
#[derive(Default)]
struct Activity {
    active: u64,
    total: u64,
    last_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn list_workers(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkerView>>> {
    let store = WorkerStore {
        root: s.global_dir.join("autoflows").join("workers"),
    };
    let workers = store.list().map_err(|e| ApiError::internal(e.to_string()))?;

    // Attribute every run to its `worker_id` (runs without one are ignored).
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut by_worker: HashMap<String, Activity> = HashMap::new();
    for r in &runs {
        let Some(wid) = r.worker_id.as_deref() else {
            continue;
        };
        let entry = by_worker.entry(wid.to_string()).or_default();
        entry.total += 1;
        if matches!(
            r.status,
            RunStatus::Running | RunStatus::Pending | RunStatus::AwaitingApproval
        ) {
            entry.active += 1;
        }
        entry.last_run_at = Some(match entry.last_run_at {
            Some(prev) => prev.max(r.started_at),
            None => r.started_at,
        });
    }

    let views: Vec<WorkerView> = workers
        .into_iter()
        .map(|record| {
            let act = by_worker.get(&record.worker_id);
            WorkerView {
                active_run_count: act.map(|a| a.active).unwrap_or(0),
                total_run_count: act.map(|a| a.total).unwrap_or(0),
                last_run_at: act.and_then(|a| a.last_run_at),
                record,
            }
        })
        .collect();
    Ok(Json(views))
}
