//! Shared "one representative workspace per distinct repo" selection.
//!
//! The Build list endpoints (`/api/agents`, `/api/workflows`,
//! `/api/autoflows`) aggregate global defs with every registered project's
//! `.rupu/` defs. Many registered workspaces are autoflow **run-worktrees**
//! of the very same repo (same `repo_remote`), each carrying an identical
//! copy of `.rupu/workflows/` (or `.rupu/agents/`). Scanning every
//! registered workspace therefore emits one duplicate row per worktree for
//! the same def. [`distinct_repo_workspaces`] collapses that list down to a
//! single representative workspace per distinct repo (or per standalone
//! local checkout with no tracked remote) so callers scan each repo exactly
//! once.

use rupu_workspace::{RepoRegistryStore, Workspace};
use std::collections::BTreeMap;

/// A single workspace chosen to represent a repo, tagged with the display
/// `scope` name (the representative path's basename).
pub(crate) struct RepoScope {
    pub(crate) workspace: Workspace,
    pub(crate) scope: String,
}

/// Collapse `workspaces` to one representative per distinct repo.
///
/// Grouping:
/// - `repo_remote: Some(url)` (non-empty) → grouped by `url`. The
///   representative is the group member whose `path` matches the tracked
///   repo's `preferred_path` (found via [`RepoRegistryStore::list`] by
///   matching `url` against `TrackedRepo::origin_urls`); if no tracked repo
///   record matches, or none of the group's members is at the preferred
///   path (e.g. the preferred checkout itself isn't registered as a CP
///   workspace), the tie-break is deterministic: sort the group's paths and
///   take the first.
/// - `repo_remote: None` (or empty) → every such workspace is its own
///   distinct group; all are scanned (these are standalone local
///   directories, not worktrees of a tracked repo).
///
/// `workspace.path` and `TrackedRepo::preferred_path` are both stored
/// canonicalized by their respective stores (see
/// `rupu_workspace::store::WorkspaceStore` and
/// `rupu_workspace::repo_store::RepoRegistryStore::upsert`), so a plain
/// string compare is sufficient here.
///
/// Output is sorted by `scope` for a deterministic response order.
pub(crate) fn distinct_repo_workspaces(
    workspaces: Vec<Workspace>,
    repo_store: &RepoRegistryStore,
) -> Vec<RepoScope> {
    let tracked = repo_store.list().unwrap_or_default();

    let mut by_remote: BTreeMap<String, Vec<Workspace>> = BTreeMap::new();
    let mut standalone: Vec<Workspace> = Vec::new();

    for w in workspaces {
        match w.repo_remote.clone().filter(|u| !u.is_empty()) {
            Some(url) => by_remote.entry(url).or_default().push(w),
            None => standalone.push(w),
        }
    }

    let mut out = Vec::with_capacity(by_remote.len() + standalone.len());

    for (url, mut group) in by_remote {
        // Deterministic tie-break candidate order, independent of whatever
        // order the workspace store's directory listing produced.
        group.sort_by(|a, b| a.path.cmp(&b.path));

        let preferred_path = tracked
            .iter()
            .find(|t| t.origin_urls.iter().any(|u| u == &url))
            .map(|t| t.preferred_path.clone());

        let chosen = preferred_path
            .as_deref()
            .and_then(|preferred| group.iter().find(|w| w.path == preferred))
            .cloned()
            .unwrap_or_else(|| group[0].clone());

        let scope = scope_name(&chosen);
        out.push(RepoScope {
            workspace: chosen,
            scope,
        });
    }

    for w in standalone {
        let scope = scope_name(&w);
        out.push(RepoScope {
            workspace: w,
            scope,
        });
    }

    out.sort_by(|a, b| a.scope.cmp(&b.scope));
    out
}

/// Scope tag for a workspace: the path's basename, falling back to the
/// workspace id if the path has no basename (e.g. `/`).
fn scope_name(w: &Workspace) -> String {
    std::path::Path::new(&w.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| w.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: &str, path: &str, repo_remote: Option<&str>) -> Workspace {
        Workspace {
            id: id.to_string(),
            path: path.to_string(),
            repo_remote: repo_remote.map(ToOwned::to_owned),
            initial_branch: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_run_at: None,
        }
    }

    fn empty_repo_store(tmp: &tempfile::TempDir) -> RepoRegistryStore {
        RepoRegistryStore {
            root: tmp.path().join("repos"),
        }
    }

    #[test]
    fn same_repo_remote_collapses_to_one_representative_with_deterministic_tiebreak() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_store = empty_repo_store(&tmp);
        // No tracked repo record at all -> deterministic path-sort tie-break.
        let workspaces = vec![
            ws(
                "ws_c",
                "/repo/worktree-c",
                Some("git@github.com:acme/x.git"),
            ),
            ws(
                "ws_a",
                "/repo/worktree-a",
                Some("git@github.com:acme/x.git"),
            ),
            ws(
                "ws_b",
                "/repo/worktree-b",
                Some("git@github.com:acme/x.git"),
            ),
        ];
        let out = distinct_repo_workspaces(workspaces, &repo_store);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].workspace.path, "/repo/worktree-a");
        assert_eq!(out[0].scope, "worktree-a");
    }

    #[test]
    fn same_repo_remote_prefers_tracked_preferred_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_store = empty_repo_store(&tmp);
        let preferred_root = tmp.path().join("preferred-checkout");
        std::fs::create_dir_all(&preferred_root).unwrap();
        repo_store
            .upsert(
                "github:acme/x",
                &preferred_root,
                Some("git@github.com:acme/x.git"),
                Some("main"),
            )
            .unwrap();
        let preferred_canonical = preferred_root.canonicalize().unwrap().display().to_string();

        let workspaces = vec![
            ws(
                "ws_z",
                "/repo/worktree-z",
                Some("git@github.com:acme/x.git"),
            ),
            ws(
                "ws_pref",
                &preferred_canonical,
                Some("git@github.com:acme/x.git"),
            ),
            ws(
                "ws_a",
                "/repo/worktree-a",
                Some("git@github.com:acme/x.git"),
            ),
        ];
        let out = distinct_repo_workspaces(workspaces, &repo_store);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].workspace.id, "ws_pref");
        assert_eq!(out[0].scope, "preferred-checkout");
    }

    #[test]
    fn different_repo_remotes_each_get_a_representative() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_store = empty_repo_store(&tmp);
        let workspaces = vec![
            ws("ws_x", "/repo-x", Some("git@github.com:acme/x.git")),
            ws("ws_y", "/repo-y", Some("git@github.com:acme/y.git")),
        ];
        let out = distinct_repo_workspaces(workspaces, &repo_store);
        assert_eq!(out.len(), 2);
        let scopes: Vec<&str> = out.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(scopes, vec!["repo-x", "repo-y"]);
    }

    #[test]
    fn no_repo_remote_scans_every_standalone_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo_store = empty_repo_store(&tmp);
        let workspaces = vec![
            ws("ws_1", "/standalone-1", None),
            ws("ws_2", "/standalone-2", None),
        ];
        let out = distinct_repo_workspaces(workspaces, &repo_store);
        assert_eq!(out.len(), 2);
        let scopes: Vec<&str> = out.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(scopes, vec!["standalone-1", "standalone-2"]);
    }
}
