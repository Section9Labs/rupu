//! Project-scoped, read-only file access for the CP "Code" tab: a lazy
//! per-directory tree and whole-file source. Every caller-supplied path is
//! resolved through `crate::api::source::resolve_under_workspace`, the same
//! containment primitive the run-scoped source endpoint uses — no path here
//! ever escapes the workspace root.

use crate::api::source::{detect_language, SourceLine};
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

/// Whole-file cap. Larger files soft-fail (the viewer shows a placeholder).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<SourceLine>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl FileContent {
    fn unavailable(reason: impl Into<String>) -> Self {
        FileContent {
            available: false,
            reason: Some(reason.into()),
            ..Default::default()
        }
    }
}

/// Read a workspace-relative file whole (up to `MAX_FILE_BYTES`). Missing,
/// oversized, non-file, or non-UTF-8 targets return `available:false` with a
/// reason (HTTP 200) — only path-safety violations are hard errors.
fn read_whole_file(workspace: &FsPath, rel: &str) -> Result<FileContent, ApiError> {
    let file = crate::api::source::resolve_under_workspace(workspace, rel)?;
    if !file.is_file() {
        return Ok(FileContent::unavailable("file not found"));
    }
    let meta = match std::fs::metadata(&file) {
        Ok(m) => m,
        Err(_) => return Ok(FileContent::unavailable("file not found")),
    };
    if meta.len() > MAX_FILE_BYTES {
        return Ok(FileContent::unavailable("file too large to display"));
    }
    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(e) => return Ok(FileContent::unavailable(e.to_string())),
    };
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Ok(FileContent::unavailable("binary or non-UTF-8 file")),
    };
    let lines: Vec<SourceLine> = text
        .lines()
        .enumerate()
        .map(|(i, l)| SourceLine {
            n: i + 1,
            text: l.to_string(),
        })
        .collect();
    Ok(FileContent {
        available: true,
        path: Some(rel.to_string()),
        language: detect_language(&file),
        total_lines: Some(lines.len()),
        lines: Some(lines),
        reason: None,
    })
}

async fn get_source(
    Path(ws_id): Path<String>,
    Query(q): Query<TreeQuery>,
    State(s): State<AppState>,
) -> ApiResult<Json<FileContent>> {
    let w = load_workspace(&s, &ws_id)?;
    let fc = read_whole_file(FsPath::new(&w.path), &q.path)?;
    Ok(Json(fc))
}

/// Hard cap on the number of files [`list_all_files`] will collect before
/// giving up and reporting `truncated: true` — bounds the response size (and
/// the walk itself) for pathologically large workspaces.
const MAX_FILES: usize = 20_000;

#[derive(Debug, Serialize, PartialEq)]
pub struct FileListResult {
    /// Sorted, workspace-relative POSIX-style file paths.
    pub files: Vec<String>,
    pub truncated: bool,
}

/// Recursively collect every workspace-relative POSIX-style *file* path
/// under `workspace`, for the project-wide search box (which needs to match
/// across the whole tree, not just the directories the lazy [`list_tree`]
/// view has loaded so far). [`HIDDEN_DIRS`] are skipped entirely, same as
/// the tree; dotfiles are otherwise included.
///
/// Symlinks — whether to a file or a directory — are never followed. This
/// is the simplest possible containment guarantee: a symlink planted inside
/// the workspace can't steer the walk (or a leaked path) outside the
/// workspace root, because we never read through it in the first place.
/// `DirEntry::file_type()` reports the entry's own type without following
/// the link, so a symlink is neither `is_dir()` nor `is_file()` and is
/// simply skipped by the `if`/`else if` below.
///
/// An unreadable directory (permissions, races) is skipped rather than
/// failing the whole walk — this is a best-effort listing, not a strict
/// contract. Stops (with `truncated: true`) once `cap` files have been
/// collected. Result is sorted.
fn list_all_files_capped(workspace: &FsPath, cap: usize) -> FileListResult {
    let mut files: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut stack: Vec<String> = vec![String::new()];
    'walk: while let Some(rel) = stack.pop() {
        let dir = if rel.is_empty() {
            workspace.to_path_buf()
        } else {
            workspace.join(&rel)
        };
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in read {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", rel.trim_end_matches('/'), name)
            };
            if file_type.is_dir() {
                if HIDDEN_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(child);
            } else if file_type.is_file() {
                files.push(child);
                if files.len() >= cap {
                    truncated = true;
                    break 'walk;
                }
            }
        }
    }
    files.sort();
    FileListResult { files, truncated }
}

async fn get_files(
    Path(ws_id): Path<String>,
    State(s): State<AppState>,
) -> ApiResult<Json<FileListResult>> {
    let w = load_workspace(&s, &ws_id)?;
    let res = list_all_files_capped(FsPath::new(&w.path), MAX_FILES);
    Ok(Json(res))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/projects/:ws_id/tree", get(get_tree))
        .route("/api/projects/:ws_id/source", get(get_source))
        .route("/api/projects/:ws_id/files", get(get_files))
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

    #[test]
    fn reads_whole_file_with_language_and_line_numbers() {
        let d = tmp_ws();
        let fc = read_whole_file(d.path(), "src/main.rs").unwrap();
        assert!(fc.available);
        assert_eq!(fc.path.as_deref(), Some("src/main.rs"));
        assert_eq!(fc.language, Some("rust"));
        assert_eq!(fc.total_lines, Some(1));
        let lines = fc.lines.unwrap();
        assert_eq!(lines[0].n, 1);
        assert_eq!(lines[0].text, "fn main() {}");
    }

    #[test]
    fn source_refuses_escape() {
        let d = tmp_ws();
        let err = read_whole_file(d.path(), "../secret").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn source_soft_fails_on_missing_file() {
        let d = tmp_ws();
        let fc = read_whole_file(d.path(), "src/nope.rs").unwrap();
        assert!(!fc.available);
        assert!(fc.reason.is_some());
        assert!(fc.lines.is_none());
    }

    #[test]
    fn source_soft_fails_on_oversized_file() {
        let d = tmp_ws();
        let big = vec![b'a'; (MAX_FILE_BYTES + 1) as usize];
        std::fs::write(d.path().join("big.bin"), &big).unwrap();
        let fc = read_whole_file(d.path(), "big.bin").unwrap();
        assert!(!fc.available);
        assert!(fc.reason.as_deref().unwrap().contains("too large"));
    }

    #[test]
    fn source_soft_fails_on_non_utf8() {
        let d = tmp_ws();
        std::fs::write(d.path().join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        let fc = read_whole_file(d.path(), "bin.dat").unwrap();
        assert!(!fc.available);
    }

    #[test]
    fn lists_all_files_workspace_relative_and_hides_noise_dirs() {
        let d = tmp_ws();
        fs::create_dir_all(d.path().join("node_modules/leftpad")).unwrap();
        fs::write(d.path().join("node_modules/leftpad/index.js"), "x").unwrap();
        let res = list_all_files_capped(d.path(), MAX_FILES);
        assert_eq!(res.files, vec!["README.md", "src/main.rs"]);
        assert!(!res.truncated);
    }

    #[test]
    fn file_list_is_sorted() {
        let d = tmp_ws();
        fs::create_dir_all(d.path().join("a/b")).unwrap();
        fs::write(d.path().join("a/b/z.rs"), "x").unwrap();
        fs::write(d.path().join("a/a.rs"), "x").unwrap();
        let res = list_all_files_capped(d.path(), MAX_FILES);
        let mut expected = res.files.clone();
        expected.sort();
        assert_eq!(res.files, expected);
        assert!(res.files.contains(&"a/a.rs".to_string()));
        assert!(res.files.contains(&"a/b/z.rs".to_string()));
    }

    #[test]
    fn file_list_does_not_follow_symlinks_out_of_the_workspace() {
        #[cfg(unix)]
        {
            let d = tmp_ws();
            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("secret.txt"), "shh").unwrap();
            // A symlinked *file* escaping the workspace...
            std::os::unix::fs::symlink(
                outside.path().join("secret.txt"),
                d.path().join("escape.txt"),
            )
            .unwrap();
            // ...and a symlinked *directory* escaping the workspace, which
            // (if followed) would leak `outside`'s contents under a
            // workspace-relative path.
            std::os::unix::fs::symlink(outside.path(), d.path().join("escape_dir")).unwrap();

            let res = list_all_files_capped(d.path(), MAX_FILES);
            assert!(!res.files.iter().any(|p| p.contains("secret")));
            assert!(!res.files.contains(&"escape.txt".to_string()));
            assert_eq!(res.files, vec!["README.md", "src/main.rs"]);
        }
    }

    #[test]
    fn file_list_respects_the_cap_and_sets_truncated() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..10 {
            fs::write(d.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let res = list_all_files_capped(d.path(), 3);
        assert_eq!(res.files.len(), 3);
        assert!(res.truncated);

        let full = list_all_files_capped(d.path(), MAX_FILES);
        assert_eq!(full.files.len(), 10);
        assert!(!full.truncated);
    }
}
