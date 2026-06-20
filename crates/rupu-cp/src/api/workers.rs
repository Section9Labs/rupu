use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::State, routing::get, Json, Router};
use rupu_runtime::WorkerRecord;
use rupu_workspace::worker_store::WorkerStore;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/workers", get(list_workers))
}

async fn list_workers(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkerRecord>>> {
    let store = WorkerStore {
        root: s.global_dir.join("autoflows").join("workers"),
    };
    let workers = store.list().map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(workers))
}
