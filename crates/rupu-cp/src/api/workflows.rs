use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::Workflow;
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/:name", get(get_workflow))
}

#[derive(Serialize)]
struct WorkflowDto {
    name: String,
    scope: String,
}

async fn list_workflows(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkflowDto>>> {
    let dir = s.global_dir.join("workflows");
    if !dir.is_dir() {
        return Ok(Json(vec![]));
    }
    let entries = std::fs::read_dir(&dir)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().and_then(|s| s.to_str()) == Some("yaml")
        })
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    names.sort();
    let dtos = names
        .into_iter()
        .map(|name| WorkflowDto { name, scope: "global".to_string() })
        .collect();
    Ok(Json(dtos))
}

async fn get_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = s.global_dir.join("workflows").join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ApiError::not_found(format!("workflow {name} not found")));
    }
    let yaml = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let workflow = Workflow::parse(&yaml)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "workflow": workflow, "yaml": yaml })))
}
