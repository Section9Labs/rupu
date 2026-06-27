use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::Query, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

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

/// List immediate subdirectories of `path` (sorted, hidden excluded). Pure +
/// testable. Errors when the path is missing/unreadable/not a directory.
pub(crate) fn browse_dir(path: &str) -> Result<BrowseResult, String> {
    let p = std::path::Path::new(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    if !p.is_dir() {
        return Err(format!("{} is not a directory", p.display()));
    }
    let mut dirs: Vec<FsEntry> = std::fs::read_dir(&p)
        .map_err(|e| e.to_string())?
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
        parent: p.parent().map(|x| x.to_string_lossy().into_owned()),
        dirs,
    })
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

async fn browse(Query(q): Query<BrowseQuery>) -> ApiResult<Json<BrowseResult>> {
    let path = q.path.filter(|s| !s.is_empty()).unwrap_or_else(home_dir);
    let res = tokio::task::spawn_blocking(move || browse_dir(&path))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    res.map(Json).map_err(ApiError::bad_request)
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

        let out = browse_dir(root.to_str().unwrap()).expect("ok");
        assert_eq!(
            out.dirs.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        // Use canonicalized root so macOS /private symlinks resolve consistently.
        let canonical_root = root.canonicalize().unwrap();
        assert_eq!(
            out.parent.as_deref(),
            canonical_root.parent().and_then(|p| p.to_str())
        );
    }

    #[test]
    fn missing_dir_errors() {
        assert!(browse_dir("/no/such/dir/xyz").is_err());
    }
}
