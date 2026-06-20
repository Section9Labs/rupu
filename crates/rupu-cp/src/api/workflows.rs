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
pub(crate) struct WorkflowDto {
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) usage: crate::usage::UsageSummary,
    pub(crate) run_count: u64,
    pub(crate) last_run: Option<String>,
}

/// Scan `<dir>/*.yaml` and return one [`WorkflowDto`] per file stem, tagged
/// with `scope`, sorted by name. A missing/unreadable directory yields an
/// empty vec (tolerated, not an error) so the caller can merge layers freely.
pub(crate) fn scan_workflow_names(dir: &std::path::Path, scope: &'static str) -> Vec<WorkflowDto> {
    if !dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("workflows: could not read {}: {err}", dir.display());
            return vec![];
        }
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("yaml"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| WorkflowDto {
            name,
            scope: scope.to_string(),
            usage: crate::usage::UsageSummary::default(),
            run_count: 0,
            last_run: None,
        })
        .collect()
}

async fn list_workflows(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkflowDto>>> {
    let mut rows = scan_workflow_names(&s.global_dir.join("workflows"), "global");
    let runs = s.run_store.list().unwrap_or_default();
    let rollups =
        crate::usage::rollup_by(&s.run_store, &runs, &s.pricing, |r| Some(r.workflow_name.clone()));
    for row in &mut rows {
        if let Some(roll) = rollups.get(&row.name) {
            row.usage = roll.usage.clone();
            row.run_count = roll.run_count;
            row.last_run = roll.last_active.clone();
        }
    }
    Ok(Json(rows))
}

async fn get_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = s.global_dir.join("workflows").join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ApiError::not_found(format!("workflow {name} not found")));
    }
    let yaml = std::fs::read_to_string(&path).map_err(|e| ApiError::internal(e.to_string()))?;
    let workflow = Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;

    let runs = s.run_store.list().unwrap_or_default();
    let usage = crate::usage::rollup(
        runs.iter()
            .filter(|r| r.workflow_name == name)
            .map(|r| crate::usage::summarize_run(&s.run_store, &r.id, &s.pricing)),
    );

    Ok(Json(
        serde_json::json!({ "workflow": workflow, "yaml": yaml, "usage": usage }),
    ))
}
