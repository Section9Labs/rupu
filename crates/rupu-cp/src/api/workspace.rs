//! Workspace-sync HTTP endpoints (multi-host Slice 3c).
//!
//! Backs the [`HttpHostConnector`](crate::host::http::HttpHostConnector)'s
//! `stage_workspace` / `collect_workspace_delta` over the wire:
//!
//! - `POST /api/workspace/stage` — body is a wire-encoded
//!   [`rupu_workspace::Payload`]; stages it under `<global_dir>/workspace-sync/`
//!   and returns `{ "working_dir": "<path>" }`.
//! - `GET  /api/workspace/delta?dir=<working_dir>` — diffs the staged working
//!   dir against its baseline sidecar and returns the wire-encoded
//!   [`rupu_workspace::Delta`] as `application/octet-stream`.
//!
//! Both are backed by `rupu_workspace::{stage,collect_delta}` plus the shared
//! baseline sidecar codec in [`crate::host::connector`], so a CP host serves
//! exactly the same staging mechanism the in-process local connector uses.

#![deny(clippy::all)]

use axum::{
    body::Bytes,
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use ulid::Ulid;

use crate::{
    error::{ApiError, ApiResult},
    host::connector::{
        decode_payload, deserialize_baseline, encode_delta, serialize_baseline, MAX_WORKSPACE_BYTES,
    },
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workspace/stage", post(stage_workspace))
        .route("/api/workspace/delta", get(collect_workspace_delta))
}

/// Root under which all staged workspaces live. The `dir` query param of the
/// delta endpoint MUST resolve inside this directory.
fn sync_root(s: &AppState) -> std::path::PathBuf {
    s.global_dir.join("workspace-sync")
}

async fn stage_workspace(
    State(s): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    if body.len() > MAX_WORKSPACE_BYTES {
        return Err(ApiError::bad_request(format!(
            "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
            body.len()
        )));
    }
    let decoded = decode_payload(&body).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let base = sync_root(&s).join(Ulid::new().to_string());
    let work = base.join("work");
    let baseline =
        rupu_workspace::stage(&decoded, &work).map_err(|e| ApiError::internal(e.to_string()))?;
    let sidecar = serialize_baseline(&baseline).map_err(|e| ApiError::internal(e.to_string()))?;
    std::fs::write(base.join("baseline.json"), sidecar)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "working_dir": work.to_string_lossy(),
    })))
}

#[derive(Deserialize)]
struct DeltaQuery {
    dir: String,
}

async fn collect_workspace_delta(
    State(s): State<AppState>,
    Query(q): Query<DeltaQuery>,
) -> ApiResult<Vec<u8>> {
    let work = std::path::PathBuf::from(&q.dir);
    let base = work
        .parent()
        .ok_or_else(|| ApiError::bad_request("invalid working dir"))?;

    // Confinement: the requested dir must live inside the sync cache. Compare
    // canonicalized paths so `..` traversal cannot escape the cache root.
    let root = sync_root(&s);
    let root_canon =
        std::fs::canonicalize(&root).map_err(|e| ApiError::internal(format!("sync root: {e}")))?;
    let work_canon = std::fs::canonicalize(&work)
        .map_err(|_| ApiError::not_found("staged workspace not found"))?;
    if !work_canon.starts_with(&root_canon) {
        return Err(ApiError::bad_request(
            "working dir is outside the workspace-sync cache",
        ));
    }

    let baseline_bytes = std::fs::read(base.join("baseline.json"))
        .map_err(|_| ApiError::not_found("staged baseline not found"))?;
    let baseline =
        deserialize_baseline(&baseline_bytes).map_err(|e| ApiError::internal(e.to_string()))?;
    let delta = rupu_workspace::collect_delta(&work, &baseline)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let bytes = encode_delta(&delta);
    // Best-effort scratch cleanup once the delta has been read out.
    let _ = std::fs::remove_dir_all(base);
    Ok(bytes)
}
