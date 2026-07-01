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
    extract::{DefaultBodyLimit, Query, State},
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
        // Raise the per-route body limit beyond axum's 2 MB default so that
        // real packed workspaces (up to MAX_WORKSPACE_BYTES = 256 MiB) are
        // accepted by the router before reaching the handler. Without this
        // layer, any payload > 2 MB returns 413 before the handler runs.
        .route(
            "/api/workspace/stage",
            post(stage_workspace).layer(DefaultBodyLimit::max(MAX_WORKSPACE_BYTES)),
        )
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::connector::encode_payload;
    use http::{Request, StatusCode};
    use tower::ServiceExt as _;

    fn test_state(tmp: &tempfile::TempDir) -> AppState {
        AppState::new(
            tmp.path().to_path_buf(),
            rupu_config::PricingConfig::default(),
        )
    }

    /// Build a minimal valid tar archive containing one file.
    fn tar_bytes(path: &str, content: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut hdr = tar::Header::new_gnu();
            hdr.set_size(content.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            b.append_data(&mut hdr, path, content).unwrap();
            b.finish().unwrap();
        }
        buf
    }

    /// Build a wire-encoded Payload (mode byte + raw tar) for tar mode.
    fn tar_payload(path: &str, content: &[u8]) -> Vec<u8> {
        let payload = rupu_workspace::Payload {
            mode: rupu_workspace::SyncMode::Tar,
            bytes: tar_bytes(path, content),
        };
        encode_payload(&payload)
    }

    // ── FIX 1 proof: large payload (> 2 MB default) must reach the handler ──

    /// POST a ~3 MB valid tar payload to `/api/workspace/stage` and assert the
    /// response is NOT 413. If DefaultBodyLimit::max(MAX_WORKSPACE_BYTES) is
    /// wired correctly, axum's 2 MB default is overridden and the handler runs.
    #[tokio::test]
    async fn stage_large_payload_above_default_limit_is_accepted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp);

        // Build a ~3 MB tar payload (well above the 2 MB axum default).
        let big_content = vec![0u8; 3_000_000];
        let body = tar_payload("big.bin", &big_content);
        assert!(body.len() > 2 * 1024 * 1024, "payload must exceed 2 MB");

        let app = routes().with_state(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/workspace/stage")
            .body(axum::body::Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        // 200 OK or any non-413 status: the body limit layer didn't reject it.
        assert_ne!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "large-but-under-max payload must not be 413"
        );
    }

    // ── FIX 2b: round-trip stage → delta returns valid bytes ─────────────────

    /// Stage a small workspace then immediately collect its delta via the HTTP
    /// handlers. The staged tree is unmodified, so the delta has no changed or
    /// deleted paths, and `encode_delta` produces a decodable wire blob.
    #[tokio::test]
    async fn stage_and_collect_delta_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp);

        // Stage a one-file workspace.
        let body = axum::body::Bytes::from(tar_payload("hello.txt", b"world"));
        let stage_resp = stage_workspace(State(state.clone()), body)
            .await
            .expect("stage must succeed");
        let working_dir = stage_resp
            .0
            .get("working_dir")
            .and_then(|v| v.as_str())
            .expect("response must contain working_dir")
            .to_string();

        // Collect the delta without modifying the staged tree (empty delta).
        let delta_bytes = collect_workspace_delta(
            State(state),
            Query(DeltaQuery {
                dir: working_dir.clone(),
            }),
        )
        .await
        .expect("collect_delta must succeed");

        // The bytes must decode back to a valid Delta.
        let delta =
            crate::host::connector::decode_delta(&delta_bytes).expect("delta bytes must decode");
        // Unmodified tree → no changes.
        assert!(
            delta.changed.is_empty(),
            "unmodified staged tree must have no changed paths"
        );
        assert!(
            delta.deleted.is_empty(),
            "unmodified staged tree must have no deleted paths"
        );
    }

    // ── FIX 2c: confinement guard rejects paths outside the sync root ─────────

    /// `GET /api/workspace/delta?dir=<outside>` must return 400 when the
    /// requested dir exists but is not inside the workspace-sync cache root.
    #[tokio::test]
    async fn delta_rejects_working_dir_outside_sync_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp);

        // The confinement guard canonicalizes both the sync root and the
        // requested path. Create the sync root so its canonicalize succeeds;
        // then pass a path that exists but lives *outside* it.
        let sync_root = tmp.path().join("workspace-sync");
        std::fs::create_dir_all(&sync_root).unwrap();

        // Use another tempdir that definitely lives outside the sync root.
        let outside = tempfile::TempDir::new().unwrap();

        let result = collect_workspace_delta(
            State(state),
            Query(DeltaQuery {
                dir: outside.path().to_string_lossy().into_owned(),
            }),
        )
        .await;

        // Must fail with 400 BAD_REQUEST (confinement) not 404 or 500.
        assert!(result.is_err(), "path outside sync root must be rejected");
        let crate::error::ApiError(status, msg) = result.unwrap_err();
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "confinement violation must be 400, got {status}: {msg}"
        );
        assert!(
            msg.contains("outside"),
            "error message must mention 'outside', got: {msg}"
        );
    }
}
