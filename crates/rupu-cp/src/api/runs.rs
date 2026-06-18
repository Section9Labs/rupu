use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::{RunRecord, RunStoreError};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs", get(list_runs))
        .route("/api/runs/:id", get(get_run))
}

async fn list_runs(State(s): State<AppState>) -> ApiResult<Json<Vec<RunRecord>>> {
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(runs))
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
