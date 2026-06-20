use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::{
    coverage_status, discover_targets, read_findings, CoveragePaths, CoverageStatusInput,
};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/coverage", get(list_coverage))
        .route("/api/coverage/:target", get(get_coverage))
}

#[derive(Serialize)]
struct CoverageSummary {
    target_id: String,
    assertion_lines: usize,
    has_catalog: bool,
    findings: usize,
}

/// Coverage data lives under the WORKSPACE (the directory the CP was
/// launched in), not the global `~/.rupu` dir. Phase-1 scope: single
/// project pointed to by `AppState::workspace_dir`.
async fn list_coverage(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<CoverageSummary>>> {
    let targets = discover_targets(&s.workspace_dir)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut summaries = Vec::with_capacity(targets.len());
    for t in targets {
        let paths = CoveragePaths::new(&s.workspace_dir, &t.target_id);
        let findings = match read_findings(&paths) {
            Ok(f) => f.len(),
            Err(ref e) => {
                tracing::warn!(target_id = %t.target_id, error = %e, "failed to read findings; using 0");
                0
            }
        };
        summaries.push(CoverageSummary {
            target_id: t.target_id,
            assertion_lines: t.assertion_lines,
            has_catalog: t.has_catalog,
            findings,
        });
    }
    Ok(Json(summaries))
}

async fn get_coverage(
    State(s): State<AppState>,
    Path(target): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Verify the target exists.
    let targets = discover_targets(&s.workspace_dir)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let discovered = targets
        .into_iter()
        .find(|t| t.target_id == target)
        .ok_or_else(|| ApiError::not_found(format!("coverage target {target} not found")))?;

    let paths = CoveragePaths::new(&s.workspace_dir, &target);
    let assertions = coverage_status(&paths, CoverageStatusInput::default())
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let findings = read_findings(&paths)
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "target_id": discovered.target_id,
        "assertion_lines": discovered.assertion_lines,
        "has_catalog": discovered.has_catalog,
        "assertions": assertions,
        "findings": findings,
    })))
}
