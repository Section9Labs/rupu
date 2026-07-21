//! Project-scoped, read-only file access for the CP "Code" tab: a lazy
//! per-directory tree and whole-file source. Every caller-supplied path is
//! resolved through `crate::api::source::resolve_under_workspace`, the same
//! containment primitive the run-scoped source endpoint uses — no path here
//! ever escapes the workspace root.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::Path as FsPath;

/// A directory noise-list hidden from the tree. Dotfiles are otherwise shown.
const HIDDEN_DIRS: &[&str] = &[".git", ".rupu", "node_modules", "target"];

#[derive(Debug, Serialize, PartialEq)]
pub struct TreeEntry {
    pub name: String,
    /// Workspace-relative POSIX-style path (matches `FindingRecord.file_path`).
    pub path: String,
    /// `"dir"` or `"file"`.
    pub kind: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TreeResult {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    #[serde(default)]
    pub path: String,
}

/// Resolve a `ws_id` to its `Workspace`, or a 404. Shared by both handlers.
pub(crate) fn load_workspace(
    s: &AppState,
    ws_id: &str,
) -> Result<rupu_workspace::Workspace, ApiError> {
    let store = rupu_workspace::store::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    };
    match store.load(ws_id) {
        Ok(Some(w)) => Ok(w),
        Ok(None) => Err(ApiError::not_found(format!("project {ws_id} not found"))),
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

/// List the immediate children of a workspace-relative directory.
/// `rel == ""` means the workspace root. Dirs first, then files, each group
/// sorted by name; entries in `HIDDEN_DIRS` are omitted.
fn list_tree(workspace: &FsPath, rel: &str) -> Result<TreeResult, ApiError> {
    let dir = crate::api::source::resolve_under_workspace(workspace, rel)?;
    if !dir.is_dir() {
        return Err(ApiError::bad_request("path is not a directory"));
    }
    let mut dirs: Vec<TreeEntry> = Vec::new();
    let mut files: Vec<TreeEntry> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| ApiError::internal(e.to_string()))? {
        let entry = entry.map_err(|e| ApiError::internal(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && HIDDEN_DIRS.contains(&name.as_str()) {
            continue;
        }
        // Build the workspace-relative child path with forward slashes.
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel.trim_end_matches('/'), name)
        };
        let te = TreeEntry {
            name,
            path: child,
            kind: if is_dir { "dir".into() } else { "file".into() },
        };
        if is_dir {
            dirs.push(te);
        } else {
            files.push(te);
        }
    }
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.extend(files);

    let parent = if rel.is_empty() {
        None
    } else {
        Some(
            rel.trim_end_matches('/')
                .rsplit_once('/')
                .map(|(p, _)| p.to_string())
                .unwrap_or_default(),
        )
    };
    Ok(TreeResult {
        path: rel.to_string(),
        parent,
        entries: dirs,
    })
}

async fn get_tree(
    Path(ws_id): Path<String>,
    Query(q): Query<TreeQuery>,
    State(s): State<AppState>,
) -> ApiResult<Json<TreeResult>> {
    let w = load_workspace(&s, &ws_id)?;
    let res = list_tree(FsPath::new(&w.path), &q.path)?;
    Ok(Json(res))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/projects/:ws_id/tree", get(get_tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_ws() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("src")).unwrap();
        fs::write(d.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(d.path().join("README.md"), "# hi\n").unwrap();
        fs::create_dir_all(d.path().join(".git")).unwrap();
        fs::write(d.path().join(".git/HEAD"), "ref: x\n").unwrap();
        d
    }

    #[test]
    fn lists_root_dirs_first_then_files_and_hides_git() {
        let d = tmp_ws();
        let res = list_tree(d.path(), "").unwrap();
        let names: Vec<_> = res.entries.iter().map(|e| e.name.as_str()).collect();
        // dirs before files, alphabetical within group, .git hidden
        assert_eq!(names, vec!["src", "README.md"]);
        assert_eq!(res.entries[0].kind, "dir");
        assert_eq!(res.entries[0].path, "src");
        assert_eq!(res.entries[1].kind, "file");
        assert_eq!(res.entries[1].path, "README.md");
        assert_eq!(res.parent, None);
    }

    #[test]
    fn lists_subdir_and_sets_parent() {
        let d = tmp_ws();
        let res = list_tree(d.path(), "src").unwrap();
        assert_eq!(res.path, "src");
        assert_eq!(res.parent.as_deref(), Some(""));
        let names: Vec<_> = res.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["main.rs"]);
        assert_eq!(res.entries[0].path, "src/main.rs");
    }

    #[test]
    fn refuses_parent_dir_escape() {
        let d = tmp_ws();
        let err = list_tree(d.path(), "../etc").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn refuses_absolute_path() {
        let d = tmp_ws();
        let err = list_tree(d.path(), "/etc").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn errors_when_path_is_a_file_not_dir() {
        let d = tmp_ws();
        let err = list_tree(d.path(), "README.md").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }
}
