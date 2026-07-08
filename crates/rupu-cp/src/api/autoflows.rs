use crate::{
    api::{fs_safety, repo_scope::distinct_repo_workspaces},
    config_write::write_atomic_raw,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use rupu_orchestrator::Workflow;
use rupu_workspace::{RepoRegistryStore, WorkspaceStore};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/autoflows", get(list_autoflow_defs))
        .route("/api/autoflows/:name/enable", post(enable_autoflow))
        .route("/api/autoflows/:name/disable", post(disable_autoflow))
}

/// Slim DTO for a single autoflow-enabled workflow definition.
#[derive(Serialize)]
pub(crate) struct AutoflowDefRow {
    pub(crate) name: String,
    /// File stem (e.g. `my-workflow` for `my-workflow.yaml`). The workflow
    /// detail route is keyed by file stem, not parsed `name`, so the frontend
    /// links to `/workflows/{slug}`.
    pub(crate) slug: String,
    /// `TriggerKind` as a lowercase string: `"manual"`, `"cron"`, or `"event"`.
    pub(crate) trigger: String,
    /// `"global"` or the registered project's name, depending on the layer
    /// the file came from.
    pub(crate) scope: String,
}

fn store(s: &AppState) -> WorkspaceStore {
    WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
}

fn repo_store(s: &AppState) -> RepoRegistryStore {
    RepoRegistryStore {
        root: s.global_dir.join("repos"),
    }
}

/// Scan `<dir>/*.{yaml,yml}`, parse each, and keep only those whose
/// `autoflow.enabled == true` (matching the CLI's `autoflow list` predicate).
/// Each kept row is tagged with `scope`. A missing/unreadable dir → empty vec
/// (tolerated). Unparseable files are skipped with a `tracing::warn!`.
pub(crate) fn scan_autoflow_defs(
    dir: &std::path::Path,
    scope: impl Into<String>,
) -> Vec<AutoflowDefRow> {
    let scope = scope.into();
    if !dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("autoflows: could not read workflows dir: {err}");
            return vec![];
        }
    };

    let mut rows: Vec<AutoflowDefRow> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext == "yaml" || ext == "yml")
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let path = e.path();
            let slug = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())?;
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!("autoflows: could not read {}: {err}", path.display());
                    return None;
                }
            };
            let workflow = match Workflow::parse(&body) {
                Ok(w) => w,
                Err(err) => {
                    tracing::warn!("autoflows: skipping {}: {err}", path.display());
                    return None;
                }
            };
            // Mirror the CLI's `autoflow list` predicate exactly:
            // `workflow.autoflow.as_ref().map(|a| a.enabled).unwrap_or(false)`
            if !workflow
                .autoflow
                .as_ref()
                .map(|a| a.enabled)
                .unwrap_or(false)
            {
                return None;
            }
            let trigger = match workflow.trigger.on {
                rupu_orchestrator::TriggerKind::Manual => "manual",
                rupu_orchestrator::TriggerKind::Cron => "cron",
                rupu_orchestrator::TriggerKind::Event => "event",
            }
            .to_string();
            Some(AutoflowDefRow {
                name: workflow.name,
                slug,
                trigger,
                scope: scope.clone(),
            })
        })
        .collect();

    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// `GET /api/autoflows`
///
/// Scans `<global>/workflows/*.yaml` plus one representative workspace per
/// distinct repo among the registered projects' `<path>/.rupu/workflows/*.yaml`
/// (see [`distinct_repo_workspaces`]) — many registered workspaces are
/// autoflow run-worktrees of the same repo, so scanning every registered
/// workspace would emit one duplicate row per worktree. Each kept file is
/// parsed with [`Workflow::parse`]; only those where `autoflow.enabled ==
/// true` (matching the CLI's `autoflow list` predicate) are returned, sorted
/// by name then scope.
///
/// Each row is tagged `scope: "global"` or the representative workspace's
/// path basename. A project def shadows a same-named GLOBAL row; two
/// different repos defining the same name both appear (distinguished by
/// `scope`). With no registered projects this is byte-for-byte the prior
/// global-only behavior.
///
/// A missing workflows directory → `[]` (not an error).
/// An unparseable YAML file is skipped with a `tracing::warn!`.
async fn list_autoflow_defs(State(s): State<AppState>) -> ApiResult<Json<Vec<AutoflowDefRow>>> {
    let mut rows = scan_autoflow_defs(&s.global_dir.join("workflows"), "global");

    let workspaces = store(&s).list().unwrap_or_default();
    let repos = distinct_repo_workspaces(workspaces, &repo_store(&s));
    let mut project_rows: Vec<AutoflowDefRow> = Vec::new();
    for r in repos {
        let dir = std::path::Path::new(&r.workspace.path)
            .join(".rupu")
            .join("workflows");
        project_rows.extend(scan_autoflow_defs(&dir, r.scope));
    }

    let project_names: std::collections::BTreeSet<&str> =
        project_rows.iter().map(|r| r.name.as_str()).collect();
    rows.retain(|r| !project_names.contains(r.name.as_str()));
    rows.extend(project_rows);
    rows.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.scope.cmp(&b.scope)));

    Ok(Json(rows))
}

/// Response for `POST /api/autoflows/:name/enable` and `.../disable`.
#[derive(Debug, Serialize)]
pub(crate) struct SetEnabledResponse {
    pub(crate) name: String,
    pub(crate) enabled: bool,
}

/// `POST /api/autoflows/:name/enable` — flip `autoflow.enabled` to `true` in
/// the on-disk workflow YAML. See [`set_autoflow_enabled`] for the shared
/// implementation and its guarantees.
async fn enable_autoflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<SetEnabledResponse>> {
    set_autoflow_enabled(&s, &name, true).await
}

/// `POST /api/autoflows/:name/disable` — flip `autoflow.enabled` to `false`.
/// See [`set_autoflow_enabled`].
async fn disable_autoflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<SetEnabledResponse>> {
    set_autoflow_enabled(&s, &name, false).await
}

/// Shared enable/disable implementation.
///
/// - **Launcher-gated**: 501 on a read-only (`rupu cp`, no `cp serve`)
///   deploy — same "is this a `cp serve` deployment" marker every other
///   write-path gate in this crate uses (`api::hosts`, `api::config`).
/// - **Name-validated**: `:name` must pass [`fs_safety::validate_name`] (bare
///   file stem, no `/`, `.`, or `..`) before any path resolution or disk
///   access, mirroring the guard on the sibling write endpoints in
///   `api::workflows` (`write_workflow`, `create_workflow`,
///   `delete_workflow`) — a traversal name is rejected outright rather than
///   resolved against the workflows dir.
/// - **Project-aware**: resolves `:name` to a workflow YAML path via
///   [`super::workflows::resolve_workflow_path`] — the same global-then-
///   registered-projects resolution `GET /api/workflows/:name` uses — so an
///   autoflow defined only inside a registered project's `.rupu/workflows/`
///   is reachable, not just global ones. 404 if no file resolves.
/// - **Targeted edit, not a round-trip**: [`set_autoflow_enabled_in_yaml`]
///   rewrites only the `enabled:` scalar line inside the `autoflow:` block
///   (or inserts one, if the block omits it — `enabled` is `#[serde(default)]`
///   in `rupu_orchestrator::Autoflow`) and leaves every other line — comments,
///   key order, unrelated formatting — untouched. A `serde_yaml` round-trip
///   was considered and rejected: it would re-serialize the *entire* file,
///   silently dropping comments and normalizing key order/quoting on a
///   definition an operator may hand-edit.
/// - **Validated before write**: the candidate text must both
///   [`Workflow::parse`] successfully AND still carry a `workflow.autoflow`
///   block. Either failure rejects the request and leaves the on-disk file
///   byte-for-byte untouched — the edit only ever touches disk via
///   [`write_atomic_raw`], which is only called once validation passes.
/// - **Backup + atomic**: persisted via [`write_atomic_raw`] (backup to
///   `<path>.bak`, write-then-rename), not `config_write::write_atomic` —
///   that helper's `validate_toml` gate would reject YAML outright.
async fn set_autoflow_enabled(
    s: &AppState,
    name: &str,
    enabled: bool,
) -> ApiResult<Json<SetEnabledResponse>> {
    s.launcher.as_ref().ok_or_else(|| {
        ApiError::not_available("enabling/disabling an autoflow requires `rupu cp serve`")
    })?;

    // Reject a path-traversal `:name` (e.g. `../../evil`) before it ever
    // reaches `resolve_workflow_path` — mirrors the same guard the sibling
    // write endpoints in `api::workflows` apply (`write_workflow`,
    // `create_workflow`, `delete_workflow`).
    fs_safety::validate_name(name)?;

    let path = super::workflows::resolve_workflow_path(s, name)
        .ok_or_else(|| ApiError::not_found(format!("autoflow {name} not found")))?;
    let existing = std::fs::read_to_string(&path).map_err(|e| ApiError::internal(e.to_string()))?;

    // The file must already be an autoflow (not just any workflow) — mirrors
    // the 404-on-unknown-autoflow contract even when a same-named plain
    // workflow exists.
    let existing_workflow =
        Workflow::parse(&existing).map_err(|e| ApiError::internal(e.to_string()))?;
    if existing_workflow.autoflow.is_none() {
        return Err(ApiError::not_found(format!("autoflow {name} not found")));
    }

    let candidate = set_autoflow_enabled_in_yaml(&existing, enabled)
        .map_err(|e| ApiError::bad_request(format!("could not edit autoflow YAML: {e}")))?;

    // Validate the candidate before it ever touches disk: must parse AND
    // still be an autoflow. Reject (file untouched) on either failure.
    let parsed = Workflow::parse(&candidate).map_err(|e| {
        ApiError::bad_request(format!("edit would produce an invalid workflow: {e}"))
    })?;
    if parsed.autoflow.as_ref().map(|a| a.enabled) != Some(enabled) {
        return Err(ApiError::internal(
            "edit did not produce the expected autoflow.enabled value",
        ));
    }

    let path_for_write = path.clone();
    tokio::task::spawn_blocking(move || write_atomic_raw(&path_for_write, &candidate))
        .await
        .map_err(|e| ApiError::internal(format!("autoflow write task panicked: {e}")))?
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(SetEnabledResponse {
        name: name.to_string(),
        enabled,
    }))
}

/// Rewrite just the `enabled:` scalar inside a workflow YAML's top-level
/// `autoflow:` block, leaving every other line untouched. If the block has
/// no `enabled:` key (legal — `Autoflow::enabled` is `#[serde(default)]`),
/// inserts one as the block's first line, matching the indent unit of an
/// existing sibling key (falling back to 2 spaces for an empty block).
///
/// Line-based rather than a `serde_yaml` parse+re-serialize so comments and
/// unrelated formatting in the rest of the file survive untouched (see the
/// doc comment on [`set_autoflow_enabled`] for why the round-trip approach
/// was rejected).
fn set_autoflow_enabled_in_yaml(yaml: &str, enabled: bool) -> Result<String, String> {
    fn indent_of(line: &str) -> usize {
        line.len() - line.trim_start().len()
    }

    let had_trailing_newline = yaml.ends_with('\n');
    let mut lines: Vec<String> = yaml.lines().map(|l| l.to_string()).collect();

    let autoflow_idx = lines
        .iter()
        .position(|l| l.trim_start() == "autoflow:" && indent_of(l) == 0)
        .ok_or_else(|| "no top-level `autoflow:` key".to_string())?;

    // Block extent: every line after `autoflow:` more-indented than it (or
    // blank) belongs to the block; the first zero-indent non-blank line ends it.
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(autoflow_idx + 1) {
        if l.trim().is_empty() {
            continue;
        }
        if indent_of(l) == 0 {
            end = i;
            break;
        }
    }

    let enabled_line_idx = lines[autoflow_idx + 1..end]
        .iter()
        .position(|l| l.trim_start().starts_with("enabled:"))
        .map(|i| autoflow_idx + 1 + i);

    match enabled_line_idx {
        Some(idx) => {
            let indent = " ".repeat(indent_of(&lines[idx]));
            lines[idx] = format!("{indent}enabled: {enabled}");
        }
        None => {
            let indent = lines[autoflow_idx + 1..end]
                .iter()
                .find(|l| !l.trim().is_empty())
                .map(|l| indent_of(l))
                .unwrap_or(2);
            lines.insert(
                autoflow_idx + 1,
                format!("{}enabled: {enabled}", " ".repeat(indent)),
            );
        }
    }

    let mut out = lines.join("\n");
    if had_trailing_newline {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn slug_is_file_stem_distinct_from_parsed_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        // File stem (`my-file-stem`) deliberately differs from the workflow's
        // parsed `name` (`parsed-name`), because the workflow detail route is
        // keyed by file stem while the autoflow's display name is the parsed
        // name. The row must carry both.
        let path = dir.path().join("my-file-stem.yaml");
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(
            b"name: parsed-name\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n",
        )
        .expect("write");

        let rows = scan_autoflow_defs(dir.path(), "global");
        assert_eq!(rows.len(), 1, "the enabled autoflow should be returned");
        assert_eq!(rows[0].name, "parsed-name");
        assert_eq!(rows[0].slug, "my-file-stem");
    }

    const ENABLED_AUTOFLOW: &str =
        "name: nightly\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
        .with_workspace_dir(tmp.path().to_path_buf())
    }

    /// Register a workspace record `<global_dir>/workspaces/<id>.toml` whose
    /// `path` points at `project_root`.
    fn register_workspace(tmp: &tempfile::TempDir, id: &str, project_root: &std::path::Path) {
        register_workspace_with_remote(tmp, id, project_root, None);
    }

    /// Same as [`register_workspace`], optionally tagging the record with a
    /// `repo_remote` (simulating autoflow run-worktrees of the same repo).
    fn register_workspace_with_remote(
        tmp: &tempfile::TempDir,
        id: &str,
        project_root: &std::path::Path,
        repo_remote: Option<&str>,
    ) {
        std::fs::create_dir_all(tmp.path().join("workspaces")).unwrap();
        let remote_line = repo_remote
            .map(|u| format!("repo_remote = \"{u}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            tmp.path().join("workspaces").join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\npath = \"{}\"\n{remote_line}created_at = \"2026-01-01T00:00:00Z\"\n",
                project_root.display()
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn list_no_projects_is_global_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap();
        std::fs::write(
            tmp.path().join("workflows").join("nightly.yaml"),
            ENABLED_AUTOFLOW,
        )
        .unwrap();
        let s = test_state(&tmp);

        let Json(rows) = list_autoflow_defs(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "global");
    }

    #[tokio::test]
    async fn list_includes_project_defs_tagged_with_project_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap(); // empty global

        let proj = tempfile::TempDir::new().unwrap();
        let proj_workflows = proj.path().join(".rupu").join("workflows");
        std::fs::create_dir_all(&proj_workflows).unwrap();
        std::fs::write(proj_workflows.join("nightly.yaml"), ENABLED_AUTOFLOW).unwrap();
        register_workspace(&tmp, "ws_a", proj.path());

        let s = test_state(&tmp);
        let Json(rows) = list_autoflow_defs(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 1);
        let expected_scope = proj
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(rows[0].scope, expected_scope);
        assert_eq!(rows[0].name, "nightly");
    }

    /// Seed a `.rupu/workflows/issue-triage.yaml` under `root` (an
    /// autoflow-enabled def named `issue-triage`).
    fn seed_issue_triage(root: &std::path::Path) {
        let workflows = root.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(
            workflows.join("issue-triage.yaml"),
            "name: issue-triage\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn same_repo_worktrees_dedupe_to_one_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap(); // empty global

        // Three registered workspaces = run-worktrees of the SAME repo, each
        // carrying its own copy of `.rupu/workflows/issue-triage.yaml`.
        let remote = "git@github.com:acme/widgets.git";
        for (id, name) in [
            ("ws_a", "worktree-a"),
            ("ws_b", "worktree-b"),
            ("ws_c", "worktree-c"),
        ] {
            let root = tmp.path().join(name);
            std::fs::create_dir_all(&root).unwrap();
            seed_issue_triage(&root);
            register_workspace_with_remote(&tmp, id, &root, Some(remote));
        }

        let s = test_state(&tmp);
        let Json(rows) = list_autoflow_defs(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            1,
            "issue-triage must appear exactly once despite 3 worktrees of the same repo"
        );
        assert_eq!(rows[0].name, "issue-triage");
        // No tracked-repo record was seeded, so the tie-break is the
        // deterministic path sort: "worktree-a" sorts first.
        assert_eq!(rows[0].scope, "worktree-a");
    }

    #[tokio::test]
    async fn different_repos_same_def_name_both_appear() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap(); // empty global

        const FOO_WORKFLOW: &str =
            "name: foo\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";

        let proj_x = tmp.path().join("proj-x");
        let workflows_x = proj_x.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_x).unwrap();
        std::fs::write(workflows_x.join("foo.yaml"), FOO_WORKFLOW).unwrap();
        register_workspace_with_remote(&tmp, "ws_x", &proj_x, Some("git@github.com:acme/x.git"));

        let proj_y = tmp.path().join("proj-y");
        let workflows_y = proj_y.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_y).unwrap();
        std::fs::write(workflows_y.join("foo.yaml"), FOO_WORKFLOW).unwrap();
        register_workspace_with_remote(&tmp, "ws_y", &proj_y, Some("git@github.com:acme/y.git"));

        let s = test_state(&tmp);
        let Json(rows) = list_autoflow_defs(State(s)).await.expect("ok");
        assert_eq!(rows.len(), 2, "different repos are distinct groups");
        let scopes: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["proj-x", "proj-y"])
        );
        assert!(rows.iter().all(|r| r.name == "foo"));
    }

    #[tokio::test]
    async fn no_repo_remote_scans_every_standalone_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap(); // empty global

        let proj_a = tmp.path().join("standalone-a");
        let workflows_a = proj_a.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_a).unwrap();
        std::fs::write(
            workflows_a.join("alpha.yaml"),
            ENABLED_AUTOFLOW.replace("nightly", "alpha"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_a", &proj_a);

        let proj_b = tmp.path().join("standalone-b");
        let workflows_b = proj_b.join(".rupu").join("workflows");
        std::fs::create_dir_all(&workflows_b).unwrap();
        std::fs::write(
            workflows_b.join("beta.yaml"),
            ENABLED_AUTOFLOW.replace("nightly", "beta"),
        )
        .unwrap();
        register_workspace(&tmp, "ws_b", &proj_b);

        let s = test_state(&tmp);
        let Json(rows) = list_autoflow_defs(State(s)).await.expect("ok");
        assert_eq!(
            rows.len(),
            2,
            "both standalone (no repo_remote) dirs are scanned"
        );
        let names: std::collections::BTreeSet<&str> =
            rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, std::collections::BTreeSet::from(["alpha", "beta"]));
    }

    // ── enable/disable ───────────────────────────────────────────────────

    use crate::launcher::{LaunchError, LaunchRequest, RunLauncher};
    use std::sync::Arc;

    /// This feature only ever checks `AppState.launcher.is_some()`; it never
    /// calls `launch()`, so a launcher that panics if invoked doubles as an
    /// assertion that the write path stays launcher-free.
    struct DummyLauncher;

    #[async_trait::async_trait]
    impl RunLauncher for DummyLauncher {
        async fn launch(&self, _req: LaunchRequest) -> Result<String, LaunchError> {
            unreachable!("enable/disable must never invoke the launcher")
        }
    }

    fn with_dummy_launcher(s: AppState) -> AppState {
        s.with_launcher(Some(Arc::new(DummyLauncher) as Arc<dyn RunLauncher>))
    }

    const AUTOFLOW_ENABLED_TRUE: &str =
        "name: nightly\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";
    const AUTOFLOW_ENABLED_FALSE: &str =
        "name: nightly\nautoflow:\n  enabled: false\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";

    /// Seed `<tmp>/workflows/<filename>` (the global workflows dir) with
    /// `body`, returning its path.
    fn seed_global_autoflow(
        tmp: &tempfile::TempDir,
        filename: &str,
        body: &str,
    ) -> std::path::PathBuf {
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(filename);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn bak_path(path: &std::path::Path) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("{}.bak", path.display()))
    }

    #[tokio::test]
    async fn disable_sets_autoflow_enabled_false_in_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = seed_global_autoflow(&tmp, "nightly.yaml", AUTOFLOW_ENABLED_TRUE);
        let s = with_dummy_launcher(test_state(&tmp));

        let resp = disable_autoflow(State(s), Path("nightly".into()))
            .await
            .expect("disable should succeed");
        assert!(!resp.0.enabled);
        assert_eq!(resp.0.name, "nightly");

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("enabled: false"),
            "on-disk YAML should now be disabled: {on_disk}"
        );
        let parsed = Workflow::parse(&on_disk).expect("still Workflow::parse's");
        assert!(!parsed.autoflow.expect("still an autoflow").enabled);

        assert!(
            bak_path(&path).exists(),
            "a .bak of the prior content must exist"
        );
        assert_eq!(
            std::fs::read_to_string(bak_path(&path)).unwrap(),
            AUTOFLOW_ENABLED_TRUE
        );
    }

    #[tokio::test]
    async fn enable_sets_true() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = seed_global_autoflow(&tmp, "nightly.yaml", AUTOFLOW_ENABLED_FALSE);
        let s = with_dummy_launcher(test_state(&tmp));

        let resp = enable_autoflow(State(s), Path("nightly".into()))
            .await
            .expect("enable should succeed");
        assert!(resp.0.enabled);

        let on_disk = std::fs::read_to_string(&path).unwrap();
        let parsed = Workflow::parse(&on_disk).expect("still Workflow::parse's");
        assert!(parsed.autoflow.expect("still an autoflow").enabled);
    }

    #[tokio::test]
    async fn enable_requires_launcher() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_global_autoflow(&tmp, "nightly.yaml", AUTOFLOW_ENABLED_FALSE);
        let s = test_state(&tmp); // no launcher installed — read-only deploy

        let err = enable_autoflow(State(s), Path("nightly".into()))
            .await
            .expect_err("no launcher should be rejected");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn enable_unknown_autoflow_404() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap();
        let s = with_dummy_launcher(test_state(&tmp));

        let err = enable_autoflow(State(s), Path("does-not-exist".into()))
            .await
            .expect_err("unknown name should 404");
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn enable_rejects_path_traversal_name_before_any_disk_access() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A file that a successful traversal *would* reach: sibling of the
        // global dir, i.e. `<global_dir>/evil.yaml` via `../evil` from inside
        // `<global_dir>/workflows/`.
        let outside_target = tmp.path().join("evil.yaml");
        std::fs::write(&outside_target, AUTOFLOW_ENABLED_FALSE).unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap();
        let s = with_dummy_launcher(test_state(&tmp));

        let err = enable_autoflow(State(s), Path("../evil".into()))
            .await
            .expect_err("traversal name must be rejected");
        assert_eq!(
            err.0,
            axum::http::StatusCode::BAD_REQUEST,
            "validate_name should reject the traversal name outright"
        );

        let on_disk = std::fs::read_to_string(&outside_target).unwrap();
        assert_eq!(
            on_disk, AUTOFLOW_ENABLED_FALSE,
            "the out-of-tree file must be byte-for-byte untouched"
        );
        assert!(
            !bak_path(&outside_target).exists(),
            "no backup should be created — the guard must fire before any disk access"
        );
    }

    #[tokio::test]
    async fn disable_rejects_path_traversal_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside_target = tmp.path().join("evil.yaml");
        std::fs::write(&outside_target, AUTOFLOW_ENABLED_TRUE).unwrap();
        std::fs::create_dir_all(tmp.path().join("workflows")).unwrap();
        let s = with_dummy_launcher(test_state(&tmp));

        let err = disable_autoflow(State(s), Path("../evil".into()))
            .await
            .expect_err("traversal name must be rejected");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);

        let on_disk = std::fs::read_to_string(&outside_target).unwrap();
        assert_eq!(on_disk, AUTOFLOW_ENABLED_TRUE, "file must be untouched");
        assert!(!bak_path(&outside_target).exists(), "no backup written");
    }

    #[tokio::test]
    async fn enable_invalid_result_rejected_file_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Flow-style `autoflow: {...}` is valid YAML — `Workflow::parse`
        // accepts it — but the targeted line-based editor only recognizes
        // block-style `autoflow:\n  enabled: ...`, so it must reject this
        // rather than corrupt the file with a malformed insertion.
        let body = "name: nightly\nautoflow: {enabled: false}\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";
        let path = seed_global_autoflow(&tmp, "nightly.yaml", body);
        let s = with_dummy_launcher(test_state(&tmp));

        let err = enable_autoflow(State(s), Path("nightly".into()))
            .await
            .expect_err("unsupported edit should be rejected");
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, body, "rejected edit must leave the file untouched");
        assert!(
            !bak_path(&path).exists(),
            "no backup should be created when the write never happens"
        );
    }
}
