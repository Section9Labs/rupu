//! `GET /api/transcript` — read a JSONL transcript file with a
//! security-critical path validator.
//!
//! The validator ([`validate_transcript_path`]) is the security boundary: it
//! prevents the `?path=` query parameter from being used to read arbitrary
//! files off the host. It canonicalizes the requested path (resolving `..`
//! traversal and symlinks) and then requires the resolved path to live inside
//! one of the canonicalized allowed roots.

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use std::path::{Path, PathBuf};

/// Canonicalize `raw`; require a `.jsonl` file whose canonical path is inside
/// one of `allowed_roots` (themselves canonicalized). Rejects traversal,
/// symlink-escape, out-of-root absolute paths, and non-`.jsonl`.
///
/// Canonicalize-then-prefix-check is what defeats `../` traversal and symlink
/// escapes: [`std::fs::canonicalize`] resolves them before the `starts_with`
/// check runs, so a path that *textually* sits under a root but *resolves*
/// outside it is rejected.
pub fn validate_transcript_path(raw: &str, allowed_roots: &[PathBuf]) -> Result<PathBuf, String> {
    let p = Path::new(raw);
    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("not a .jsonl file".into());
    }
    let canon = std::fs::canonicalize(p).map_err(|_| "cannot resolve path".to_string())?;
    for root in allowed_roots {
        if let Ok(rc) = std::fs::canonicalize(root) {
            if canon.starts_with(&rc) {
                return Ok(canon);
            }
        }
    }
    Err("path is outside the allowed roots".into())
}

#[derive(serde::Deserialize)]
struct PathQ {
    path: String,
}

/// The set of directories a transcript path is allowed to resolve into: the
/// CP global dir plus every registered workspace's path (project-local
/// transcripts/coverage live under `<project>/.rupu/`).
fn allowed_roots(s: &AppState) -> Vec<PathBuf> {
    let mut roots = vec![s.global_dir.clone()];
    if let Ok(list) = (rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    })
    .list()
    {
        roots.extend(list.into_iter().map(|w| PathBuf::from(w.path)));
    }
    roots
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/transcript", get(get_transcript))
}

async fn get_transcript(
    State(s): State<AppState>,
    Query(q): Query<PathQ>,
) -> ApiResult<Json<serde_json::Value>> {
    let path =
        validate_transcript_path(&q.path, &allowed_roots(&s)).map_err(ApiError::bad_request)?;
    let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(&path)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    let summary = rupu_transcript::JsonlReader::summary(&path).ok();
    Ok(Json(serde_json::json!({ "events": events, "summary": summary })))
}
