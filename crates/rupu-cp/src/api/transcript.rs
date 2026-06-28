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
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Query, State},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Json, Router,
};
use futures_util::StreamExt as _;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Validate a requested transcript `?path=`: require a `.jsonl` file, reject
/// `..` traversal, and require the path to resolve inside one of `allowed_roots`
/// (themselves canonicalized). Symlink-escape is defeated by canonicalizing the
/// deepest existing ancestor before the `starts_with` check.
///
/// Unlike a plain `canonicalize`, this does NOT require the target file to
/// exist: a freshly-sent turn's transcript is validated before its worker has
/// written the `.jsonl`, so the UI can open it and stream it in (the old
/// canonicalize-the-whole-path approach returned a misleading "cannot resolve
/// path" for that race).
pub fn validate_transcript_path(raw: &str, allowed_roots: &[PathBuf]) -> Result<PathBuf, String> {
    let p = Path::new(raw);
    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("not a .jsonl file".into());
    }
    // Reject `..` outright — the web only ever passes already-resolved absolute
    // transcript paths, and forbidding ParentDir removes traversal as a vector
    // before we touch the filesystem.
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err("path must not contain ..".into());
    }
    // The transcript file may not exist yet (a freshly-sent turn whose worker
    // hasn't written the `.jsonl` — that race previously produced a misleading
    // "cannot resolve path"). Resolve symlinks on the deepest EXISTING ancestor
    // for the security check, then re-append the not-yet-created remainder. An
    // existing file is still fully canonicalized (no loss of symlink-escape
    // protection for real files).
    let resolved = if p.exists() {
        std::fs::canonicalize(p).map_err(|_| "cannot resolve path".to_string())?
    } else {
        canonicalize_existing_prefix(p).ok_or_else(|| "cannot resolve path".to_string())?
    };
    for root in allowed_roots {
        if let Ok(rc) = std::fs::canonicalize(root) {
            if resolved.starts_with(&rc) {
                return Ok(resolved);
            }
        }
    }
    Err("path is outside the allowed roots".into())
}

/// Canonicalize the deepest existing ancestor of `p` (resolving symlinks in the
/// real prefix) and re-append the remaining, not-yet-created components. Returns
/// `None` only if no ancestor exists. `p` must be `..`-free (the caller rejects
/// ParentDir first).
fn canonicalize_existing_prefix(p: &Path) -> Option<PathBuf> {
    let mut ancestor = p;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if ancestor.exists() {
            let mut out = std::fs::canonicalize(ancestor).ok()?;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                ancestor = parent;
            }
            _ => return None,
        }
    }
}

#[derive(serde::Deserialize)]
struct PathQ {
    path: String,
    /// When present and not `"local"`, proxy the request to the named host.
    /// The `path` argument is forwarded verbatim — it is meaningful only on
    /// the remote host's filesystem.
    #[serde(default)]
    host: Option<String>,
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
    Router::new()
        .route("/api/transcript", get(get_transcript))
        .route("/api/transcript/stream", get(stream_transcript))
}

/// `GET /api/transcript?path=<path>[&host=<id>]`
///
/// Without `?host=` (or `?host=local`): validate the path against local
/// allowed roots and read from disk (unchanged behaviour).
///
/// With `?host=<remote-id>`: proxy to the remote host's `/api/transcript`
/// endpoint. The path is forwarded verbatim — it is meaningful only on that
/// host's filesystem. Unknown host id → 404.
async fn get_transcript(
    State(s): State<AppState>,
    Query(q): Query<PathQ>,
) -> ApiResult<Json<serde_json::Value>> {
    let host_id = q.host.as_deref().unwrap_or("local");

    // Remote host: proxy via connector.
    if host_id != "local" {
        let conn = s.hosts.resolve(host_id).map_err(|e| match e {
            HostConnectorError::NotFound(_) => {
                ApiError::not_found(format!("host {host_id} not found"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
        let result = conn.get_transcript(&q.path).await.map_err(|e| match e {
            HostConnectorError::NotFound(_) => ApiError::not_found(e.to_string()),
            HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(Json(result));
    }

    // Local path: validate against allowed roots then read from disk.
    let path =
        validate_transcript_path(&q.path, &allowed_roots(&s)).map_err(ApiError::bad_request)?;
    // A validated-but-not-yet-written transcript (freshly-sent turn) is an empty
    // transcript, not an error — the UI opens it and the stream fills it in.
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "events": [], "summary": null })));
    }
    let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(&path)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    let summary = rupu_transcript::JsonlReader::summary(&path).ok();
    Ok(Json(
        serde_json::json!({ "events": events, "summary": summary }),
    ))
}

/// `GET /api/transcript/stream?path=` — SSE live-tail of a transcript JSONL.
///
/// Validation runs first and is the SAME security boundary as the static
/// [`get_transcript`] endpoint (400 on an invalid / out-of-root path). On
/// success, opens a [`TranscriptTail`] and maps each parsed
/// [`rupu_transcript::Event`] to an SSE `data:` line of JSON; the connection
/// stays open, emitting events as the transcript grows.
///
/// [`TranscriptTail`]: crate::transcript_tail::TranscriptTail
async fn stream_transcript(State(s): State<AppState>, Query(q): Query<PathQ>) -> Response {
    let path = match validate_transcript_path(&q.path, &allowed_roots(&s)) {
        Ok(p) => p,
        Err(e) => return ApiError::bad_request(e).into_response(),
    };
    let tail = match crate::transcript_tail::TranscriptTail::open(&path).await {
        Ok(t) => t,
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };
    let stream = tail.map(|ev| {
        let sse = SseEvent::default()
            .json_data(&ev)
            .unwrap_or_else(|_| SseEvent::default().comment("event serialize error"));
        Ok::<_, Infallible>(sse)
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
