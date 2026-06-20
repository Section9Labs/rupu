use crate::{
    api::agents::AgentDto,
    api::autoflows::{scan_autoflow_defs, AutoflowDefRow},
    api::runs::{trigger_of, RunListRow},
    api::workflows::{scan_workflow_names, WorkflowDto},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::{discover_targets, read_findings, run_audit, CoveragePaths};
use rupu_orchestrator::RunRecord;
use rupu_workspace::WorkspaceStore;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(serde::Serialize)]
pub struct ProjectRow {
    pub ws_id: String,
    pub name: String,
    pub path: String,
    pub repo_remote: Option<String>,
    pub branch: Option<String>,
    pub created_at: String,
    pub last_run_at: Option<String>,
}

/// Project rollup returned by `GET /api/projects/:ws_id`. The nested
/// `runs` / `sessions` / `coverage` objects are built ad-hoc with
/// `serde_json::json!`; the typed `project` + `recent_runs` fields keep the
/// stable shape callers depend on.
#[derive(Serialize)]
struct ProjectDetail {
    project: ProjectRow,
    runs: Value,
    sessions: Value,
    coverage: Value,
    recent_runs: Vec<RunListRow>,
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

/// Map a [`rupu_workspace::Workspace`] to a [`ProjectRow`].
fn project_row(w: &rupu_workspace::Workspace) -> ProjectRow {
    ProjectRow {
        name: std::path::Path::new(&w.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| w.path.clone()),
        ws_id: w.id.clone(),
        path: w.path.clone(),
        repo_remote: w.repo_remote.clone(),
        branch: w.initial_branch.clone(),
        created_at: w.created_at.clone(),
        last_run_at: w.last_run_at.clone(),
    }
}

pub fn routes() -> Router<AppState> {
    // Static `/api/projects` is registered before the `:ws_id` matchers so
    // axum's static-over-dynamic preference is reinforced by registration
    // order.
    Router::new()
        .route("/api/projects", get(list_projects))
        .route("/api/projects/:ws_id", get(get_project))
        .route("/api/projects/:ws_id/runs", get(project_runs))
        .route("/api/projects/:ws_id/sessions", get(project_sessions))
        .route("/api/projects/:ws_id/coverage", get(project_coverage))
        .route(
            "/api/projects/:ws_id/coverage/assessed",
            get(project_coverage_assessed),
        )
        .route("/api/projects/:ws_id/agents", get(project_agents))
        .route("/api/projects/:ws_id/workflows", get(project_workflows))
        .route("/api/projects/:ws_id/autoflows", get(project_autoflows))
}

async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Vec<ProjectRow>>> {
    let mut rows: Vec<ProjectRow> = store(&s)
        .list()
        .unwrap_or_default()
        .iter()
        .map(project_row)
        .collect();
    // Newest activity first; `None` sorts last (None < Some(_) in Rust's
    // default Ord, so reversing puts Some(_) before None).
    rows.sort_by(|a, b| b.last_run_at.cmp(&a.last_run_at));
    Ok(Json(rows))
}

/// Load a workspace by id; `Ok(None)` → 404, store error → 500.
fn load_workspace(s: &AppState, ws_id: &str) -> Result<rupu_workspace::Workspace, ApiError> {
    match store(s).load(ws_id) {
        Ok(Some(w)) => Ok(w),
        Ok(None) => Err(ApiError::not_found(format!("project {ws_id} not found"))),
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

/// All runs for `ws_id`, newest-first (by `started_at`).
fn scoped_runs(s: &AppState, ws_id: &str) -> Result<Vec<RunRecord>, ApiError> {
    let mut runs: Vec<RunRecord> = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .filter(|r| r.workspace_id == ws_id)
        .collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    Ok(runs)
}

/// `GET /api/projects/:ws_id` — project rollup (runs / sessions / coverage).
async fn get_project(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<ProjectDetail>> {
    let w = load_workspace(&s, &ws_id)?;

    // ── runs ──────────────────────────────────────────────────────────────
    let runs = scoped_runs(&s, &ws_id)?;
    let total = runs.len();
    let running = runs
        .iter()
        .filter(|r| matches!(r.status, rupu_orchestrator::RunStatus::Running))
        .count();
    let mut by_status: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut workflow = 0usize;
    let mut autoflow = 0usize;
    for r in &runs {
        *by_status.entry(r.status.as_str()).or_insert(0) += 1;
        // "manual" → workflow surface; "cron"/"event" → autoflow surface.
        match trigger_of(r) {
            "manual" => workflow += 1,
            _ => autoflow += 1,
        }
    }
    let recent_runs: Vec<RunListRow> = runs.iter().take(10).map(RunListRow::from).collect();
    let runs_obj = json!({
        "total": total,
        "running": running,
        "by_status": by_status,
        "by_surface": { "workflow": workflow, "autoflow": autoflow },
    });

    // ── sessions ──────────────────────────────────────────────────────────
    let sessions = crate::api::sessions::collect_sessions(&s.global_dir);
    let scoped_sessions: Vec<&Value> = sessions
        .iter()
        .filter(|v| v["workspace_id"].as_str() == Some(ws_id.as_str()))
        .collect();
    let sessions_active = scoped_sessions
        .iter()
        .filter(|v| session_is_active(v))
        .count();
    let sessions_obj = json!({
        "total": scoped_sessions.len(),
        "active": sessions_active,
    });

    // ── coverage ──────────────────────────────────────────────────────────
    // Coverage lives under the PROJECT's path (`<project>/.rupu/coverage/`),
    // not the CP's launch dir.
    // Only CHEAP signals are computed here: target count + findings count.
    // The expensive `run_audit` (assessed_pct) is deferred to
    // `GET /api/projects/:ws_id/coverage/assessed` which the frontend fetches
    // in parallel without blocking the overview render.
    let wp = std::path::Path::new(&w.path);
    let targets = discover_targets(wp).unwrap_or_default();
    let findings_sum: usize = targets
        .iter()
        .map(|t| {
            let paths = CoveragePaths::new(wp, &t.target_id);
            read_findings(&paths).map(|f| f.len()).unwrap_or(0)
        })
        .sum();
    let coverage_obj = json!({
        "targets": targets.len(),
        "findings": findings_sum,
    });

    Ok(Json(ProjectDetail {
        project: project_row(&w),
        runs: runs_obj,
        sessions: sessions_obj,
        coverage: coverage_obj,
        recent_runs,
    }))
}

/// Best-effort "is this session active?" from the serialised DTO `status`.
/// The status value is whatever the serialiser produced (string or tagged
/// object); we accept the common `running` / `active` spellings.
fn session_is_active(v: &Value) -> bool {
    let status = &v["status"];
    if let Some(s) = status.as_str() {
        let s = s.to_ascii_lowercase();
        return s == "running" || s == "active";
    }
    // Tagged-enum shapes like {"type":"running"} or {"running": ...}.
    if let Some(obj) = status.as_object() {
        return obj.keys().any(|k| {
            let k = k.to_ascii_lowercase();
            k == "running" || k == "active"
        });
    }
    false
}

/// `GET /api/projects/:ws_id/runs` — scoped slim run list, newest-first.
async fn project_runs(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    // 404 when the project is unknown, mirroring the rollup endpoint.
    load_workspace(&s, &ws_id)?;
    let runs = scoped_runs(&s, &ws_id)?;
    Ok(Json(runs.iter().map(RunListRow::from).collect()))
}

/// `GET /api/projects/:ws_id/sessions` — session DTOs scoped to the project.
async fn project_sessions(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    load_workspace(&s, &ws_id)?;
    let scoped: Vec<Value> = crate::api::sessions::collect_sessions(&s.global_dir)
        .into_iter()
        .filter(|v| v["workspace_id"].as_str() == Some(ws_id.as_str()))
        .collect();
    Ok(Json(scoped))
}

/// `GET /api/projects/:ws_id/coverage` — per-target coverage summary rows,
/// rooted at the project's path (not the CP launch dir).
async fn project_coverage(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let w = load_workspace(&s, &ws_id)?;
    let wp = std::path::Path::new(&w.path);
    let targets = discover_targets(wp).unwrap_or_default();
    let mut rows = Vec::with_capacity(targets.len());
    for t in targets {
        let paths = CoveragePaths::new(wp, &t.target_id);
        let findings = read_findings(&paths).map(|f| f.len()).unwrap_or(0);
        rows.push(json!({
            "target_id": t.target_id,
            "assertion_lines": t.assertion_lines,
            "has_catalog": t.has_catalog,
            "findings": findings,
        }));
    }
    Ok(Json(rows))
}

/// Response shape for `GET /api/projects/:ws_id/coverage/assessed`.
#[derive(Serialize)]
struct AssessedPctResponse {
    assessed_pct: Option<f64>,
}

/// `GET /api/projects/:ws_id/coverage/assessed` — heavy per-target audit
/// aggregated into a single `assessed_pct` value.  This is the expensive
/// computation that was previously blocking the synchronous project rollup.
/// The frontend fetches it in parallel after the overview has already rendered.
async fn project_coverage_assessed(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<AssessedPctResponse>> {
    let w = load_workspace(&s, &ws_id)?;
    let wp = std::path::Path::new(&w.path);
    let targets = discover_targets(wp).unwrap_or_default();
    let mut total_concerns = 0usize;
    let mut complete_concerns = 0usize;
    for t in &targets {
        let paths = CoveragePaths::new(wp, &t.target_id);
        // Targets without a catalog (or with a malformed one) are skipped —
        // they simply don't contribute to the assessed ratio.
        if let Ok(a) = run_audit(&paths) {
            total_concerns += a.total_concerns;
            complete_concerns += a.complete_concerns;
        }
    }
    let assessed_pct = if total_concerns > 0 {
        Some((complete_concerns as f64 / total_concerns as f64) * 100.0)
    } else {
        None
    };
    Ok(Json(AssessedPctResponse { assessed_pct }))
}

/// `GET /api/projects/:ws_id/agents` — global agents merged with the project's
/// local `<path>/.rupu/agents/*.md`. Project entries shadow globals by name.
/// Each row is tagged `scope: "project" | "global"`: a name is `"project"` iff
/// `<path>/.rupu/agents/<name>.md` exists on disk.
async fn project_agents(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<AgentDto>>> {
    let w = load_workspace(&s, &ws_id)?;
    // The loader joins `agents` onto the project arg, so we pass `<path>/.rupu`.
    let rupu_dir = std::path::Path::new(&w.path).join(".rupu");
    let specs = rupu_agent::loader::load_agents(&s.global_dir, Some(&rupu_dir))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let project_agents_dir = rupu_dir.join("agents");
    let dtos = specs
        .into_iter()
        .map(|spec| {
            // Project iff the same-named file exists under the project layer.
            let local = project_agents_dir.join(format!("{}.md", spec.name));
            let scope = if local.is_file() { "project" } else { "global" };
            AgentDto::from_spec(spec, scope)
        })
        .collect();
    Ok(Json(dtos))
}

/// Merge a project-layer scan over a global-layer scan, where the project
/// entries shadow globals by `name`. Returns the merged list sorted by name.
fn merge_workflow_dtos(
    mut global: Vec<WorkflowDto>,
    project: Vec<WorkflowDto>,
) -> Vec<WorkflowDto> {
    let project_names: std::collections::BTreeSet<String> =
        project.iter().map(|d| d.name.clone()).collect();
    global.retain(|d| !project_names.contains(&d.name));
    global.extend(project);
    global.sort_by(|a, b| a.name.cmp(&b.name));
    global
}

/// `GET /api/projects/:ws_id/workflows` — global workflows merged with the
/// project's `<path>/.rupu/workflows/*.yaml`; project shadows global by name.
async fn project_workflows(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<WorkflowDto>>> {
    let w = load_workspace(&s, &ws_id)?;
    let global = scan_workflow_names(&s.global_dir.join("workflows"), "global");
    let project_dir = std::path::Path::new(&w.path)
        .join(".rupu")
        .join("workflows");
    let project = scan_workflow_names(&project_dir, "project");
    Ok(Json(merge_workflow_dtos(global, project)))
}

/// Merge project autoflow defs over globals (project shadows global by name),
/// sorted by name.
fn merge_autoflow_defs(
    mut global: Vec<AutoflowDefRow>,
    project: Vec<AutoflowDefRow>,
) -> Vec<AutoflowDefRow> {
    let project_names: std::collections::BTreeSet<String> =
        project.iter().map(|d| d.name.clone()).collect();
    global.retain(|d| !project_names.contains(&d.name));
    global.extend(project);
    global.sort_by(|a, b| a.name.cmp(&b.name));
    global
}

/// `GET /api/projects/:ws_id/autoflows` — autoflow-enabled workflows from the
/// global layer merged with the project's `<path>/.rupu/workflows`; project
/// shadows global by name.
async fn project_autoflows(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<AutoflowDefRow>>> {
    let w = load_workspace(&s, &ws_id)?;
    let global = scan_autoflow_defs(&s.global_dir.join("workflows"), "global");
    let project_dir = std::path::Path::new(&w.path)
        .join(".rupu")
        .join("workflows");
    let project = scan_autoflow_defs(&project_dir, "project");
    Ok(Json(merge_autoflow_defs(global, project)))
}
