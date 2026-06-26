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
    builtin_names, coverage_status, discover_targets, file_views, list_runs, read_file_events,
    read_findings, read_snapshot, resolve_builtin, run_audit, run_diff, CoveragePaths,
    CoverageStatusInput, DiffError, RunSelector,
};
use rupu_workspace::WorkspaceStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/coverage", get(list_coverage))
        .route("/api/coverage/templates", get(list_templates))
        .route("/api/coverage/templates/:name", get(get_template))
        .route("/api/coverage/:target", get(get_coverage))
        .route("/api/coverage/:target/catalog", get(get_catalog))
        .route("/api/coverage/:target/audit", get(get_audit))
        .route("/api/coverage/:target/runs", get(get_runs))
        .route("/api/coverage/:target/diff", get(get_diff))
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

/// The workspaces to search for a target: the single named one, or all.
fn scoped_workspaces(s: &AppState, ws_id: &Option<String>) -> Vec<rupu_workspace::Workspace> {
    let workspaces = store(s).list().unwrap_or_default();
    match ws_id {
        Some(id) => workspaces.into_iter().filter(|w| &w.id == id).collect(),
        None => workspaces,
    }
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
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        let targets = discover_targets(wp).unwrap_or_default();
        if let Some(discovered) = targets.into_iter().find(|t| t.target_id == target) {
            let paths = CoveragePaths::new(wp, &target);
            let assertions = coverage_status(&paths, CoverageStatusInput::default())
                .map_err(|e| ApiError::internal(e.to_string()))?;
            let findings = read_findings(&paths).map_err(|e| ApiError::internal(e.to_string()))?;
            // Per-file heatmap. Tolerate a missing files.jsonl → empty vec so the
            // detail still renders for targets that predate the file ledger.
            let files = file_views(&read_file_events(&paths).unwrap_or_default());

            return Ok(Json(serde_json::json!({
                "ws_id": w.id,
                "project": project_name(&w.path),
                "target_id": discovered.target_id,
                "assertion_lines": discovered.assertion_lines,
                "has_catalog": discovered.has_catalog,
                "assertions": assertions,
                "findings": findings,
                "files": files,
            })));
        }
    }

    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}

// ── Templates (global, target-independent) ────────────────────────────────

#[derive(Serialize)]
struct TemplateSummary {
    name: String,
    version: u32,
    description: String,
    concern_count: usize,
    /// Lowercase severity → count, e.g. {"high": 3, "medium": 5}.
    severity_breakdown: BTreeMap<String, usize>,
}

/// Resolve every bundled concern template into a list summary. Unparseable
/// builtins are skipped with a warning (should never happen for bundled YAML).
fn builtin_template_summaries() -> Vec<TemplateSummary> {
    let mut out = Vec::new();
    for name in builtin_names() {
        let tpl = match resolve_builtin(name) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                tracing::warn!(template = name, error = %e, "skipping unparseable builtin template");
                continue;
            }
            None => continue,
        };
        let mut severity_breakdown: BTreeMap<String, usize> = BTreeMap::new();
        for c in &tpl.concerns {
            // Severity serializes lowercase; reuse that vocabulary for the key.
            let key = serde_json::to_value(c.severity)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "medium".to_string());
            *severity_breakdown.entry(key).or_default() += 1;
        }
        out.push(TemplateSummary {
            name: tpl.name,
            version: tpl.version,
            description: tpl.description,
            concern_count: tpl.concerns.len(),
            severity_breakdown,
        });
    }
    out
}

async fn list_templates() -> Json<Vec<TemplateSummary>> {
    Json(builtin_template_summaries())
}

/// Resolve one bundled template by name. `None` for unknown names; an
/// unparseable builtin is also treated as absent (logged).
fn resolve_template_by_name(name: &str) -> Option<rupu_coverage::Template> {
    match resolve_builtin(name) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            tracing::warn!(template = name, error = %e, "unparseable builtin template");
            None
        }
        None => None,
    }
}

async fn get_template(Path(name): Path<String>) -> ApiResult<Json<rupu_coverage::Template>> {
    resolve_template_by_name(&name)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("template {name} not found")))
}

// ── Catalog (per-target) ──────────────────────────────────────────────────

/// Read the effective catalog snapshot for a target under one workspace path.
/// `Ok(None)` when the catalog file is absent; `Err` only on a corrupt file.
fn read_target_catalog(
    wp: &std::path::Path,
    target: &str,
) -> Result<Option<rupu_coverage::FlatCatalog>, String> {
    let paths = CoveragePaths::new(wp, target);
    if !paths.catalog.exists() {
        return Ok(None);
    }
    read_snapshot(&paths.catalog)
        .map(Some)
        .map_err(|e| e.to_string())
}

async fn get_catalog(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<rupu_coverage::FlatCatalog>> {
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match read_target_catalog(wp, &target) {
            Ok(Some(cat)) => return Ok(Json(cat)),
            Ok(None) => continue,
            Err(e) => return Err(ApiError::internal(e)),
        }
    }
    Err(ApiError::not_found(format!(
        "coverage catalog for target {target} not found"
    )))
}

// ── Audit (per-target; the Gap tab also consumes this) ────────────────────

/// Run the audit for a target under one workspace path. `Ok(None)` when the
/// target isn't present under this workspace.
fn run_target_audit(
    wp: &std::path::Path,
    target: &str,
) -> Result<Option<rupu_coverage::AuditReport>, String> {
    let exists = discover_targets(wp)
        .unwrap_or_default()
        .into_iter()
        .any(|t| t.target_id == target);
    if !exists {
        return Ok(None);
    }
    let paths = CoveragePaths::new(wp, target);
    run_audit(&paths).map(Some).map_err(|e| e.to_string())
}

async fn get_audit(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<rupu_coverage::AuditReport>> {
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match run_target_audit(wp, &target) {
            Ok(Some(report)) => return Ok(Json(report)),
            Ok(None) => continue,
            Err(e) => return Err(ApiError::internal(e)),
        }
    }
    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}

// ── Runs + Diff (per-target) ──────────────────────────────────────────────

/// List runs for a target under one workspace path. `Ok(None)` when the target
/// isn't present under this workspace.
fn list_target_runs(
    wp: &std::path::Path,
    target: &str,
) -> Result<Option<Vec<rupu_coverage::RunListEntry>>, String> {
    let exists = discover_targets(wp)
        .unwrap_or_default()
        .into_iter()
        .any(|t| t.target_id == target);
    if !exists {
        return Ok(None);
    }
    let paths = CoveragePaths::new(wp, target);
    list_runs(&paths).map(Some).map_err(|e| e.to_string())
}

async fn get_runs(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<GetCoverageQuery>,
) -> ApiResult<Json<Vec<rupu_coverage::RunListEntry>>> {
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match list_target_runs(wp, &target) {
            Ok(Some(runs)) => return Ok(Json(runs)),
            Ok(None) => continue,
            Err(e) => return Err(ApiError::internal(e)),
        }
    }
    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}

#[derive(Deserialize)]
struct DiffQuery {
    ws_id: Option<String>,
    base: Option<String>,
    compare: Option<String>,
}

/// Parse an optional selector string, falling back to `default` when absent.
/// `RunSelector::from_str` is infallible (`latest`/`previous`/explicit id).
fn parse_selector(raw: &Option<String>, default: RunSelector) -> RunSelector {
    match raw {
        Some(s) => s.parse().unwrap_or(default),
        None => default,
    }
}

/// Run a diff for a target under one workspace path. `Ok(None)` when the target
/// isn't present; `Err(DiffError)` when selectors can't resolve.
fn run_target_diff(
    wp: &std::path::Path,
    target: &str,
    base: &RunSelector,
    compare: &RunSelector,
) -> Result<Option<rupu_coverage::RunDiff>, DiffError> {
    let exists = discover_targets(wp)
        .unwrap_or_default()
        .into_iter()
        .any(|t| t.target_id == target);
    if !exists {
        return Ok(None);
    }
    let paths = CoveragePaths::new(wp, target);
    run_diff(&paths, base, compare).map(Some)
}

async fn get_diff(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<rupu_coverage::RunDiff>> {
    let base = parse_selector(&q.base, RunSelector::Previous);
    let compare = parse_selector(&q.compare, RunSelector::Latest);
    for w in scoped_workspaces(&s, &q.ws_id) {
        let wp = std::path::Path::new(&w.path);
        match run_target_diff(wp, &target, &base, &compare) {
            Ok(Some(diff)) => return Ok(Json(diff)),
            Ok(None) => continue,
            // A selector that can't resolve (too few runs / bad id) is a client
            // condition, not a server fault.
            Err(DiffError::Io(e)) => return Err(ApiError::internal(e.to_string())),
            Err(e @ (DiffError::UnknownRun(_) | DiffError::NoRunMatches(_))) => {
                return Err(ApiError::bad_request(e.to_string()))
            }
        }
    }
    Err(ApiError::not_found(format!(
        "coverage target {target} not found"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_summaries_include_known_builtins_with_counts() {
        let summaries = builtin_template_summaries();
        let stride = summaries
            .iter()
            .find(|t| t.name == "stride")
            .expect("stride template present");
        assert!(stride.concern_count > 0, "stride should have concerns");
        let sum: usize = stride.severity_breakdown.values().copied().sum();
        assert_eq!(sum, stride.concern_count);
    }

    #[test]
    fn resolve_template_returns_known_and_rejects_unknown() {
        assert!(resolve_template_by_name("stride").is_some());
        assert!(resolve_template_by_name("does-not-exist").is_none());
    }

    #[test]
    fn catalog_reads_back_written_snapshot() {
        use rupu_coverage::{write_snapshot, FlatCatalog};
        let dir = tempfile::tempdir().expect("tempdir");
        let wp = dir.path();
        let paths = CoveragePaths::new(wp, "tgt-1");
        std::fs::create_dir_all(paths.catalog.parent().unwrap()).unwrap();
        let cat = FlatCatalog {
            concerns: vec![],
            sources: Default::default(),
            render_modes: Default::default(),
        };
        write_snapshot(&cat, &paths.catalog).expect("write snapshot");

        let got = read_target_catalog(wp, "tgt-1").expect("ok").expect("some");
        assert_eq!(got.concerns.len(), 0);
        assert!(read_target_catalog(wp, "missing").unwrap().is_none());
    }

    #[test]
    fn audit_runs_for_existing_target_only() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let wp = dir.path();
        let paths = CoveragePaths::new(wp, "tgt-9");
        std::fs::create_dir_all(&paths.root).unwrap();
        let mut f = std::fs::File::create(&paths.concerns).unwrap();
        f.write_all(b"").unwrap();

        assert!(run_target_audit(wp, "tgt-9").expect("ok").is_some());
        assert!(run_target_audit(wp, "nope").expect("ok").is_none());
    }

    #[test]
    fn list_runs_for_existing_target_only() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let wp = dir.path();
        let paths = CoveragePaths::new(wp, "tgt-runs");
        std::fs::create_dir_all(&paths.root).unwrap();
        // Empty concerns ledger → target discovered, zero runs.
        std::fs::File::create(&paths.concerns)
            .unwrap()
            .write_all(b"")
            .unwrap();

        let got = list_target_runs(wp, "tgt-runs").expect("ok");
        assert!(got.is_some(), "existing target resolves");
        assert_eq!(got.unwrap().len(), 0, "no runs recorded");
        assert!(list_target_runs(wp, "missing").unwrap().is_none());
    }

    #[test]
    fn diff_query_selectors_default_and_parse() {
        assert_eq!(
            parse_selector(&None, RunSelector::Latest),
            RunSelector::Latest
        );
        assert_eq!(
            parse_selector(&Some("previous".to_string()), RunSelector::Latest),
            RunSelector::Previous
        );
        assert_eq!(
            parse_selector(&Some("run_123".to_string()), RunSelector::Latest),
            RunSelector::RunId("run_123".to_string())
        );
    }
}
