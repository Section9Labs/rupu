use crate::{
    api::agents::AgentDto,
    api::autoflows::{scan_autoflow_defs, AutoflowDefRow},
    api::repo_scope::ScopeKind,
    api::runs::RunListRow,
    api::workflows::{scan_workflow_names, WorkflowDto},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::{discover_targets, read_declared_findings, run_audit, CoveragePaths};
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
    /// Repository landing-page URL derived from `repo_remote` (github/gitlab
    /// only; `None` for an unrecognized host or no remote at all). Powers the
    /// "View on repository" link in the identity header + Code tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_home_url: Option<String>,
    pub created_at: String,
    pub last_run_at: Option<String>,
    pub usage: crate::usage::UsageSummary,
    pub run_count: u64,
    pub last_active: Option<String>,
    /// The customer the project is currently assigned to. Always serialized
    /// (`null` = no customer), so a coordinator can tell "no customer" from
    /// a peer too old to say.
    pub customer: Option<crate::customers::CustomerRef>,
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
    usage: crate::usage::UsageSummary,
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

/// Map a [`rupu_workspace::Workspace`] to a [`ProjectRow`] with no customer;
/// callers set `customer` from the current assignment
/// ([`crate::customers::CustomerLookup::project_customer`]).
pub(crate) fn project_row(w: &rupu_workspace::Workspace) -> ProjectRow {
    ProjectRow {
        name: std::path::Path::new(&w.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| w.path.clone()),
        ws_id: w.id.clone(),
        path: w.path.clone(),
        repo_remote: w.repo_remote.clone(),
        branch: w.initial_branch.clone(),
        repo_home_url: w
            .repo_remote
            .as_deref()
            .and_then(rupu_scm::weburl::parse_repo_remote)
            .map(|r| r.home_url()),
        created_at: w.created_at.clone(),
        last_run_at: w.last_run_at.clone(),
        usage: crate::usage::UsageSummary::default(),
        run_count: 0,
        last_active: None,
        customer: None,
    }
}

/// Each workspace's current customer, as row references. Blocking IO.
fn project_customers(
    lookup: &mut crate::customers::CustomerLookup,
    ws_ids: &[String],
) -> Result<BTreeMap<String, crate::customers::CustomerRef>, ApiError> {
    let mut out = BTreeMap::new();
    for id in ws_ids {
        if let Some(c) = lookup.project_customer(id)? {
            out.insert(id.clone(), c);
        }
    }
    Ok(out)
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

/// The Projects page's per-project rollups: every workflow run grouped by
/// owning workspace id, plus each project's standalone agent runs and
/// session turns (`extras`, from
/// [`crate::usage_sources::unclaimed_extra_sources`] — each transcript
/// once), which add spend and activity, not to `run_count` (the project's
/// workflow-run count, as its runs tab lists). Each run and transcript is
/// priced with the pricing of the customer it is attributed to (recorded,
/// else — a legacy record — the workspace's current assignment). A project
/// rollup counts ALL of the project's work, whoever it is attributed to: a
/// run that recorded another customer (or none) before the project was
/// (re)assigned stays in the project's rollup but counts towards the
/// customer it recorded, so a customer's projects need not sum to that
/// customer's rollup. `keep` selects the projects. An assignment that
/// cannot be read fails the call. Blocking IO.
pub(crate) fn project_rollups(
    run_store: &rupu_orchestrator::runs::RunStore,
    runs: &[RunRecord],
    extras: &[crate::usage_sources::ExtraSource],
    prices: &mut crate::customers::PricingMemo<'_>,
    lookup: &mut crate::customers::CustomerLookup,
    keep: impl Fn(&str) -> bool,
) -> Result<BTreeMap<String, crate::usage::EntityRollup>, ApiError> {
    let mut out: BTreeMap<String, crate::usage::EntityRollup> = BTreeMap::new();
    for r in runs {
        if !keep(&r.workspace_id) {
            continue;
        }
        let who = lookup.attribute(crate::customers::Recorded::of(&r.customer), &r.workspace_id)?;
        let usage = crate::usage::summarize_run(run_store, &r.id, prices.get(who.slug.as_deref()));
        out.entry(r.workspace_id.clone())
            .or_default()
            .add(&usage, Some(r.started_at.to_rfc3339()));
    }
    for src in extras {
        if src.workspace_id.is_empty() || !keep(&src.workspace_id) {
            continue;
        }
        let who = src.attribute(lookup)?;
        let usage = crate::usage::summarize_run_usage(
            &crate::usage::transcripts_usage(&src.paths),
            prices.get(who.slug.as_deref()),
        );
        out.entry(src.workspace_id.clone())
            .or_default()
            .add_spend(&usage, src.started_at.map(|t| t.to_rfc3339()));
    }
    Ok(out)
}

/// Apply a project's rollup to its row.
pub(crate) fn apply_rollup(row: &mut ProjectRow, roll: &crate::usage::EntityRollup) {
    row.usage = roll.usage.clone();
    row.run_count = roll.run_count;
    row.last_active = roll.last_active.clone();
}

/// `?customer=<slug>|none` on `GET /api/projects`.
#[derive(serde::Deserialize, Default)]
struct ProjectsQuery {
    customer: Option<String>,
}

/// `GET /api/projects[?customer=<slug>|none]` — every registered project with
/// its rollup and current customer. `?customer=` keeps the projects CURRENTLY
/// assigned to that customer (`none` = to no customer; plan ruling 6). A bad
/// slug is a 400.
async fn list_projects(
    State(s): State<AppState>,
    Query(q): Query<ProjectsQuery>,
) -> ApiResult<Json<Vec<ProjectRow>>> {
    let filter = crate::customers::CustomerFilter::parse(q.customer.as_deref())?;
    let workspaces = store(&s).list().unwrap_or_default();
    let ws_ids: Vec<String> = workspaces.iter().map(|w| w.id.clone()).collect();
    // Every workflow run grouped by owning workspace id, plus each project's
    // standalone agent runs and session turns: they add spend and activity,
    // not to `run_count` (the project's workflow-run count, as its runs tab
    // lists). Blocking IO.
    let (rollups, mut customers) = {
        let run_store = std::sync::Arc::clone(&s.run_store);
        let global = s.global_dir.clone();
        let pricing = std::sync::Arc::clone(&s.customer_pricing);
        crate::api::runs::blocking(move || {
            let mut lookup =
                crate::customers::CustomerLookup::new(rupu_workspace::CustomerStore::new(&global));
            let mut prices = crate::customers::PricingMemo::new(&pricing);
            let customers = project_customers(&mut lookup, &ws_ids)?;
            let runs = run_store.list().unwrap_or_default();
            let extras = crate::usage_sources::unclaimed_extra_sources(&global, &run_store);
            let rollups =
                project_rollups(&run_store, &runs, &extras, &mut prices, &mut lookup, |_| {
                    true
                })?;
            Ok((rollups, customers))
        })
        .await?
    };
    let mut rows: Vec<ProjectRow> = workspaces.iter().map(project_row).collect();
    for row in &mut rows {
        if let Some(roll) = rollups.get(&row.ws_id) {
            apply_rollup(row, roll);
        }
        row.customer = customers.remove(&row.ws_id);
    }
    if let Some(f) = &filter {
        rows.retain(|r| f.matches(r.customer.as_ref().map(|c| c.slug.as_str())));
    }
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
        match r.trigger_str() {
            "manual" => workflow += 1,
            _ => autoflow += 1,
        }
    }
    let mut recent_runs: Vec<RunListRow> = runs.iter().take(10).map(RunListRow::from).collect();
    let runs_obj = json!({
        "total": total,
        "running": running,
        "by_status": by_status,
        "by_surface": { "workflow": workflow, "autoflow": autoflow },
    });

    // ── sessions + usage (blocking IO) ────────────────────────────────────
    // Sessions are only counted here, so they are listed without folding
    // their transcripts. Usage is the project's workflow runs plus its
    // standalone agent runs and session turns, each transcript once.
    let (scoped_sessions, usage, mut customers, recent_who) = {
        let run_store = std::sync::Arc::clone(&s.run_store);
        let global = s.global_dir.clone();
        let pricing = std::sync::Arc::clone(&s.customer_pricing);
        let ws = ws_id.clone();
        let scoped = runs.clone();
        crate::api::runs::blocking(move || {
            let mut lookup =
                crate::customers::CustomerLookup::new(rupu_workspace::CustomerStore::new(&global));
            let mut prices = crate::customers::PricingMemo::new(&pricing);
            let customers = project_customers(&mut lookup, std::slice::from_ref(&ws))?;
            // The recent runs' customers: recorded, else (legacy) derived.
            let recent_who = scoped
                .iter()
                .take(10)
                .map(|r| {
                    lookup.attribute(crate::customers::Recorded::of(&r.customer), &r.workspace_id)
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            let sessions = crate::api::sessions::collect_sessions_with(
                &global,
                crate::api::sessions::SessionScan {
                    pricing: None,
                    workspace: Some(&ws),
                    fail_closed: true,
                },
            )?;
            let extras = crate::usage_sources::unclaimed_extra_sources(&global, &run_store);
            let usage = project_rollups(
                &run_store,
                &scoped,
                &extras,
                &mut prices,
                &mut lookup,
                |w| w == ws,
            )?
            .remove(&ws)
            .unwrap_or_default()
            .usage;
            Ok((sessions, usage, customers, recent_who))
        })
        .await?
    };
    for (row, who) in recent_runs.iter_mut().zip(recent_who) {
        row.set_customer(who);
    }
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
            read_declared_findings(&paths).map(|f| f.len()).unwrap_or(0)
        })
        .sum();
    let coverage_obj = json!({
        "targets": targets.len(),
        "findings": findings_sum,
    });

    let mut project = project_row(&w);
    project.customer = customers.remove(&w.id);
    Ok(Json(ProjectDetail {
        project,
        runs: runs_obj,
        sessions: sessions_obj,
        coverage: coverage_obj,
        recent_runs,
        usage,
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
/// Each row carries its customer (recorded, else the project's current
/// assignment) and is priced with that customer's pricing (spec §1); work
/// with no customer at the global pricing.
async fn project_runs(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    // 404 when the project is unknown, mirroring the rollup endpoint.
    load_workspace(&s, &ws_id)?;
    let runs = scoped_runs(&s, &ws_id)?; // already sorted newest-first
    let page_runs = crate::pagination::paginate(runs, &page);
    // The usage fold and the assignment read run on the blocking pool.
    let store = std::sync::Arc::clone(&s.run_store);
    let customer_pricing = std::sync::Arc::clone(&s.customer_pricing);
    let global = s.global_dir.clone();
    let rows = crate::api::runs::blocking(move || {
        let mut lookup =
            crate::customers::CustomerLookup::new(rupu_workspace::CustomerStore::new(&global));
        let mut prices = crate::customers::PricingMemo::new(&customer_pricing);
        page_runs
            .iter()
            .map(|r| RunListRow::attributed(r, &store, &mut lookup, &mut prices))
            .collect()
    })
    .await?;
    Ok(Json(rows))
}

/// `GET /api/projects/:ws_id/sessions` — session DTOs scoped to the project.
async fn project_sessions(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
    Query(page): Query<crate::pagination::PageQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    load_workspace(&s, &ws_id)?;
    // Scoped BEFORE usage is folded; each session is attributed to its
    // customer and priced with that customer's pricing. Blocking IO.
    let scoped: Vec<Value> = {
        let global = s.global_dir.clone();
        let pricing = std::sync::Arc::clone(&s.customer_pricing);
        tokio::task::spawn_blocking(move || {
            crate::api::sessions::collect_sessions_with(
                &global,
                // An unfiltered list: a session whose assignment can't be
                // read is listed without customer keys.
                crate::api::sessions::SessionScan {
                    pricing: Some(&pricing),
                    workspace: Some(&ws_id),
                    fail_closed: false,
                },
            )
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))??
    };
    Ok(Json(crate::pagination::paginate(scoped, &page)))
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
        let findings = read_declared_findings(&paths).map(|f| f.len()).unwrap_or(0);
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
/// Each row is tagged `scope: "project" | "global"` (+ structured
/// `scope_kind`): a spec's frontmatter `name` is `"project"` iff SOME file
/// under the project layer's `agents/` dir parses to that `name` — checked
/// via `project_slugs` (keyed by parsed `name`, mapping to that file's STEM),
/// not by re-deriving a candidate path from `name` itself. The latter assumed
/// the file stem equals the frontmatter name, which mislabels a project
/// agent whose stem differs from its name (hand- or CLI-authored files) as
/// `scope: "global"` — the same name-vs-stem confusion `agent_slug_map`
/// exists to avoid for `slug`.
async fn project_agents(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<AgentDto>>> {
    let w = load_workspace(&s, &ws_id)?;
    // The loader joins `agents` onto the project arg, so we pass `<path>/.rupu`.
    let rupu_dir = std::path::Path::new(&w.path).join(".rupu");
    let specs = rupu_agent::loader::load_agents(&s.global_dir, Some(&rupu_dir))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let global_slugs = crate::api::agents::agent_slug_map(&s.global_dir);
    let project_slugs = crate::api::agents::agent_slug_map(&rupu_dir);
    let dtos = specs
        .into_iter()
        .map(|spec| {
            let is_project = project_slugs.contains_key(&spec.name);
            let (scope, scope_kind) = if is_project {
                ("project", ScopeKind::Project)
            } else {
                ("global", ScopeKind::Global)
            };
            let slugs = if is_project {
                &project_slugs
            } else {
                &global_slugs
            };
            let slug = slugs
                .get(&spec.name)
                .cloned()
                .unwrap_or_else(|| spec.name.clone());
            let scope_id = is_project.then(|| w.id.clone());
            AgentDto::from_spec(spec, scope, scope_kind, slug, scope_id)
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
    let global = scan_workflow_names(
        &s.global_dir.join("workflows"),
        "global",
        ScopeKind::Global,
        None,
    );
    let project_dir = std::path::Path::new(&w.path)
        .join(".rupu")
        .join("workflows");
    let project = scan_workflow_names(
        &project_dir,
        "project",
        ScopeKind::Project,
        Some(w.id.clone()),
    );
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

/// `GET /api/projects/:ws_id/autoflows` — workflows carrying an `autoflow:`
/// block (enabled AND disabled alike — see [`scan_autoflow_defs`]) from the
/// global layer merged with the project's `<path>/.rupu/workflows`; project
/// shadows global by name.
async fn project_autoflows(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<AutoflowDefRow>>> {
    let w = load_workspace(&s, &ws_id)?;
    let global = scan_autoflow_defs(
        &s.global_dir.join("workflows"),
        "global",
        ScopeKind::Global,
        None,
    );
    let project_dir = std::path::Path::new(&w.path)
        .join(".rupu")
        .join("workflows");
    let project = scan_autoflow_defs(
        &project_dir,
        "project",
        ScopeKind::Project,
        Some(w.id.clone()),
    );
    Ok(Json(merge_autoflow_defs(global, project)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(repo_remote: Option<&str>) -> rupu_workspace::Workspace {
        rupu_workspace::Workspace {
            id: "ws1".to_string(),
            path: "/p".to_string(),
            repo_remote: repo_remote.map(|s| s.to_string()),
            initial_branch: Some("main".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_run_at: None,
        }
    }

    #[test]
    fn project_row_computes_repo_home_url_for_known_host() {
        let row = project_row(&ws(Some("git@github.com:o/r.git")));
        assert_eq!(row.repo_home_url.as_deref(), Some("https://github.com/o/r"));
    }

    #[test]
    fn project_row_repo_home_url_is_none_without_remote() {
        let row = project_row(&ws(None));
        assert_eq!(row.repo_home_url, None);
    }

    #[test]
    fn project_row_repo_home_url_is_none_for_unknown_host() {
        let row = project_row(&ws(Some("git@bitbucket.org:o/r.git")));
        assert_eq!(row.repo_home_url, None);
    }

    #[test]
    fn project_detail_serializes_usage() {
        let detail = ProjectDetail {
            project: ProjectRow {
                ws_id: "w".into(),
                name: "n".into(),
                path: "/p".into(),
                repo_remote: None,
                branch: None,
                repo_home_url: None,
                created_at: String::new(),
                last_run_at: None,
                usage: crate::usage::UsageSummary::default(),
                run_count: 0,
                last_active: None,
                customer: None,
            },
            runs: json!({}),
            sessions: json!({}),
            coverage: json!({}),
            recent_runs: vec![],
            usage: crate::usage::UsageSummary::default(),
        };
        let v = serde_json::to_value(&detail).unwrap();
        assert!(v.get("usage").is_some());
    }

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
    }

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root`. Mirrors the identical helper in
    /// `api::agents`/`api::workflows`'s test modules.
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &std::path::Path) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    /// MINOR fix: `project_agents` must label an agent as `scope: "project"`
    /// by whether its frontmatter `name` matches SOME project-layer file (via
    /// `project_slugs`, keyed by parsed name), not by re-deriving
    /// `<project>/agents/<name>.md` and checking that exact path exists. The
    /// latter assumes the file stem equals the frontmatter name — false for a
    /// project agent hand- or CLI-authored under a different filename — and
    /// mislabels it `scope: "global"` even though it only exists in the
    /// project layer.
    #[tokio::test]
    async fn project_agents_labels_by_parsed_name_not_reconstructed_stem_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = test_state(&tmp); // no global agents

        let proj = tempfile::TempDir::new().unwrap();
        let proj_agents = proj.path().join(".rupu").join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        // File stem ("my-file-stem") deliberately differs from the parsed
        // frontmatter `name` ("code-reviewer") — `project_agents_dir.join(
        // format!("{}.md", spec.name))` would look for `code-reviewer.md`,
        // which does not exist, and wrongly conclude "global".
        std::fs::write(
            proj_agents.join("my-file-stem.md"),
            "---\nname: code-reviewer\nmodel: opus\n---\nReview code carefully.\n",
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", proj.path());
        let w = store(&s).list().unwrap().into_iter().next().unwrap();

        let Json(rows) = project_agents(State(s), Path(w.id)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "code-reviewer");
        assert_eq!(rows[0].slug, "my-file-stem");
        assert_eq!(
            rows[0].scope, "project",
            "must be labeled project-scoped even though its file stem differs from its name"
        );
        assert_eq!(rows[0].scope_kind, ScopeKind::Project);
    }
}
