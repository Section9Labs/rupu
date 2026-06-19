use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::{
    coverage_status, discover_targets, read_findings, CoveragePaths, CoverageStatusInput,
};
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/coverage", get(list_coverage))
        .route("/api/coverage/:target", get(get_coverage))
}

#[derive(Serialize)]
struct CoverageSummary {
    /// Owning workspace id — target_ids can collide across workspaces, so the
    /// frontend uses this to disambiguate (and to scope the detail fetch).
    ws_id: String,
    /// Workspace path basename — display attribution / grouping key.
    project: String,
    target_id: String,
    assertion_lines: usize,
    has_catalog: bool,
    findings: usize,
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

/// Workspace path basename, falling back to the full path.
fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Coverage lives per-PROJECT under each registered workspace's
/// `<path>/.rupu/coverage/`. The firehose page aggregates every target across
/// every registered workspace (NOT the CP launch dir). A missing registry
/// yields `[]`.
async fn list_coverage(State(s): State<AppState>) -> ApiResult<Json<Vec<CoverageSummary>>> {
    let workspaces = store(&s).list().unwrap_or_default();

    let mut summaries = Vec::new();
    for w in &workspaces {
        let wp = std::path::Path::new(&w.path);
        // Tolerate workspaces whose path is gone / unreadable → skip.
        let targets = match discover_targets(wp) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(ws_id = %w.id, path = %w.path, error = %e, "discover_targets failed; skipping workspace");
                continue;
            }
        };
        let project = project_name(&w.path);
        for t in targets {
            let paths = CoveragePaths::new(wp, &t.target_id);
            let findings = match read_findings(&paths) {
                Ok(f) => f.len(),
                Err(ref e) => {
                    tracing::warn!(ws_id = %w.id, target_id = %t.target_id, error = %e, "failed to read findings; using 0");
                    0
                }
            };
            summaries.push(CoverageSummary {
                ws_id: w.id.clone(),
                project: project.clone(),
                target_id: t.target_id,
                assertion_lines: t.assertion_lines,
                has_catalog: t.has_catalog,
                findings,
            });
        }
    }
    Ok(Json(summaries))
}

#[derive(Deserialize)]
struct GetCoverageQuery {
    /// Workspace id the target lives under. Required to disambiguate colliding
    /// target_ids; the frontend threads it from the list row.
    ws_id: Option<String>,
}

/// `GET /api/coverage/:target?ws_id=…` — per-target detail.
///
/// The target is resolved under the workspace named by `ws_id`. If `ws_id` is
/// absent we fall back to scanning every registered workspace for the first
/// matching target (best-effort, for hand-typed URLs).
async fn get_coverage(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let workspaces = store(&s).list().unwrap_or_default();

    // Build the candidate (workspace-path) list: either the single named
    // workspace, or every registered workspace.
    let candidates: Vec<&rupu_workspace::Workspace> = match &q.ws_id {
        Some(id) => workspaces.iter().filter(|w| &w.id == id).collect(),
        None => workspaces.iter().collect(),
    };

    for w in candidates {
        let wp = std::path::Path::new(&w.path);
        let targets = discover_targets(wp).unwrap_or_default();
        if let Some(discovered) = targets.into_iter().find(|t| t.target_id == target) {
            let paths = CoveragePaths::new(wp, &target);
            let assertions = coverage_status(&paths, CoverageStatusInput::default())
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let findings =
                read_findings(&paths).map_err(|e| ApiError::internal(e.to_string()))?;

            return Ok(Json(serde_json::json!({
                "ws_id": w.id,
                "project": project_name(&w.path),
                "target_id": discovered.target_id,
                "assertion_lines": discovered.assertion_lines,
                "has_catalog": discovered.has_catalog,
                "assertions": assertions,
                "findings": findings,
            })));
        }
    }

    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}
