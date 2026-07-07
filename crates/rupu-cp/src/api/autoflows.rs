use crate::{api::repo_scope::distinct_repo_workspaces, error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_orchestrator::Workflow;
use rupu_workspace::{RepoRegistryStore, WorkspaceStore};
use serde::Serialize;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/autoflows", get(list_autoflow_defs))
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
}
