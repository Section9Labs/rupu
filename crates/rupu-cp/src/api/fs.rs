use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/fs/browse", get(browse))
}

#[derive(Serialize)]
pub(crate) struct FsEntry {
    pub(crate) name: String,
    pub(crate) path: String,
}

#[derive(Serialize)]
pub(crate) struct BrowseResult {
    pub(crate) path: String,
    pub(crate) parent: Option<String>,
    pub(crate) dirs: Vec<FsEntry>,
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

/// Why a browse was refused.
#[derive(Debug)]
pub(crate) enum BrowseError {
    /// Missing, unreadable, or not a directory → 400.
    Bad(String),
    /// Outside every browsable root → 403.
    OutsideRoots(String),
}

/// List immediate subdirectories of `path` (sorted, hidden excluded), which
/// must lie under one of `roots` (canonical paths). `parent` is `None` at a
/// root, so the picker cannot walk above it. Pure + testable.
pub(crate) fn browse_dir(path: &str, roots: &[PathBuf]) -> Result<BrowseResult, BrowseError> {
    let p = FsPath::new(path)
        .canonicalize()
        .map_err(|e| BrowseError::Bad(format!("{path}: {e}")))?;
    let within = |q: &FsPath| roots.iter().any(|r| q.starts_with(r));
    if !within(&p) {
        return Err(BrowseError::OutsideRoots(format!(
            "{} is outside the browsable directories (your home directory and \
             registered projects)",
            p.display()
        )));
    }
    if !p.is_dir() {
        return Err(BrowseError::Bad(format!(
            "{} is not a directory",
            p.display()
        )));
    }
    let mut dirs: Vec<FsEntry> = std::fs::read_dir(&p)
        .map_err(|e| BrowseError::Bad(e.to_string()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            Some(FsEntry {
                path: e.path().to_string_lossy().into_owned(),
                name,
            })
        })
        .collect();
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(BrowseResult {
        path: p.to_string_lossy().into_owned(),
        parent: p
            .parent()
            .filter(|x| within(x))
            .map(|x| x.to_string_lossy().into_owned()),
        dirs,
    })
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

/// The directories the picker may show: `$HOME` and every registered
/// project, canonicalized (a root that no longer exists is skipped).
fn browse_roots(global_dir: &FsPath) -> Vec<PathBuf> {
    let store = rupu_workspace::WorkspaceStore {
        root: global_dir.join("workspaces"),
    };
    std::env::var("HOME")
        .ok()
        .into_iter()
        .chain(store.list().unwrap_or_default().into_iter().map(|w| w.path))
        .filter_map(|p| FsPath::new(&p).canonicalize().ok())
        .collect()
}

/// `GET /api/fs/browse?path=` — the launch/project directory picker. Scoped
/// to `$HOME` and registered projects (403 elsewhere), so a reachable control
/// plane is not a listing of the whole server's filesystem.
async fn browse(
    State(s): State<AppState>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Json<BrowseResult>> {
    let path = q.path.filter(|s| !s.is_empty()).unwrap_or_else(home_dir);
    let global = s.global_dir.clone();
    let res = tokio::task::spawn_blocking(move || browse_dir(&path, &browse_roots(&global)))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    res.map(Json).map_err(|e| match e {
        BrowseError::Bad(m) => ApiError::bad_request(m),
        BrowseError::OutsideRoots(m) => ApiError(axum::http::StatusCode::FORBIDDEN, m),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_subdirs_sorted_excludes_hidden_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("beta")).unwrap();
        std::fs::create_dir(root.join("alpha")).unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();

        // Use canonicalized root so macOS /private symlinks resolve consistently.
        let canonical_root = root.canonicalize().unwrap();
        let roots = vec![canonical_root.parent().unwrap().to_path_buf()];
        let out = browse_dir(root.to_str().unwrap(), &roots).expect("ok");
        assert_eq!(
            out.dirs.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            out.parent.as_deref(),
            canonical_root.parent().and_then(|p| p.to_str())
        );
    }

    #[test]
    fn missing_dir_errors() {
        let roots = vec![PathBuf::from("/")];
        assert!(matches!(
            browse_dir("/no/such/dir/xyz", &roots),
            Err(BrowseError::Bad(_))
        ));
    }

    #[test]
    fn refuses_outside_roots_and_stops_parent_at_a_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("proj")).unwrap();
        let roots = vec![root.join("proj")];

        // At the root: listed, but no parent to climb to.
        let out = browse_dir(root.join("proj").to_str().unwrap(), &roots).unwrap();
        assert_eq!(out.parent, None);

        // Its parent, and a `..` escape from inside it, are refused.
        for p in [root.clone(), root.join("proj/..")] {
            assert!(matches!(
                browse_dir(p.to_str().unwrap(), &roots),
                Err(BrowseError::OutsideRoots(_))
            ));
        }
    }
}
