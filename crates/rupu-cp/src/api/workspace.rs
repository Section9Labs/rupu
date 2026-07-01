//! Workspace-sync HTTP endpoints (multi-host Slice 3c).
//!
//! Backs the [`HttpHostConnector`](crate::host::http::HttpHostConnector)'s
//! `stage_workspace` / `collect_workspace_delta` / `discard_workspace` over the
//! wire:
//!
//! - `POST /api/workspace/stage` — body is a wire-encoded
//!   [`rupu_workspace::Payload`]; stages it under `<global_dir>/workspace-sync/`
//!   and returns `{ "working_dir": "<path>" }`.
//! - `GET  /api/workspace/delta?dir=<working_dir>` — diffs the staged working
//!   dir against its baseline sidecar and returns the wire-encoded
//!   [`rupu_workspace::Delta`] as `application/octet-stream`.
//! - `DELETE /api/workspace/discard?dir=<working_dir>` — best-effort removal
//!   of a staged scratch dir when the coordinator failed between stage and
//!   collect (launch failure, poll timeout) and never called the delta
//!   endpoint. Returns `{ "ok": true }`.
//!
//! `stage`/`delta` are backed by `rupu_workspace::{stage,collect_delta}` plus
//! the shared baseline sidecar codec in [`crate::host::connector`], so a CP
//! host serves exactly the same staging mechanism the in-process local
//! connector uses. `discard` shares the confinement guard + scratch-removal
//! logic in [`crate::host::workspace_stage`] with the local connector.

#![deny(clippy::all)]

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::{
    error::{ApiError, ApiResult},
    host::connector::{HostConnectorError, MAX_WORKSPACE_BYTES},
    host::workspace_stage::{collect_from_dir, discard_from_dir, stage_to_dir},
    state::AppState,
};

/// Map a shared-core [`HostConnectorError`] to an [`ApiError`] status. The
/// stage/collect handlers delegate to the shared `workspace_stage` core (the
/// same staging the in-process local connector uses), so their failures arrive
/// as `HostConnectorError`; translate to HTTP status here.
fn to_api_err(e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::Invalid(m) => ApiError::bad_request(m),
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Unsupported(m) => ApiError::not_available(m),
        other => ApiError::internal(other.to_string()),
    }
}

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
        .route("/api/workspace/discard", delete(discard_workspace))
}

async fn stage_workspace(
    State(s): State<AppState>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    // Configurable cap at the HTTP boundary: `[cp].max_workspace_bytes` if the
    // operator set one, else the compiled default. The route's
    // `DefaultBodyLimit` still caps the body at the compiled
    // `MAX_WORKSPACE_BYTES` before we get here; this guard lets an operator
    // tighten (or, since it's read from the resolved config, apply) a
    // narrower limit without recompiling.
    let limit = crate::config_write::effective_max_workspace_bytes(
        &s.config.read().map(|c| c.cp.clone()).unwrap_or_default(),
    );
    if body.len() > limit {
        return Err(ApiError::bad_request(format!(
            "workspace payload {} bytes exceeds limit {limit}",
            body.len()
        )));
    }
    // Delegate to the shared staging core (identical to the in-process local
    // connector): size-guard + decode + stage under `<global_dir>/workspace-sync`
    // + write the baseline sidecar. `stage_to_dir` also enforces the compiled
    // `MAX_WORKSPACE_BYTES` const as a backstop.
    let work = stage_to_dir(&body, &s.global_dir).map_err(to_api_err)?;
    Ok(Json(serde_json::json!({ "working_dir": work })))
}

#[derive(Deserialize)]
struct DeltaQuery {
    dir: String,
}

async fn collect_workspace_delta(
    State(s): State<AppState>,
    Query(q): Query<DeltaQuery>,
) -> ApiResult<Vec<u8>> {
    // Delegate to the shared core: confine `dir` under the sync root, read the
    // baseline sidecar, diff, encode, and remove the scratch (even on error).
    let bytes = collect_from_dir(&q.dir, &s.global_dir).map_err(to_api_err)?;
    Ok(bytes)
}

/// `DELETE /api/workspace/discard?dir=<working_dir>` — best-effort removal of
/// a staged scratch dir the coordinator abandoned (launch failure or poll
/// timeout) without ever calling `collect_workspace_delta`. Shares the
/// confinement guard + removal logic in [`crate::host::workspace_stage`] with
/// [`crate::host::local::LocalHostConnector::discard_workspace`].
async fn discard_workspace(
    State(s): State<AppState>,
    Query(q): Query<DeltaQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    discard_from_dir(&q.dir, &s.global_dir).map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
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

    // ── discard endpoint ──────────────────────────────────────────────────────

    /// `DELETE /api/workspace/discard?dir=<staged>` removes the scratch dir
    /// without ever collecting a delta — simulating a coordinator that gave up
    /// after a launch failure.
    #[tokio::test]
    async fn discard_removes_staged_scratch_without_collecting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp);

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
        let base = std::path::Path::new(&working_dir)
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(base.exists(), "scratch base must exist after staging");

        discard_workspace(
            State(state),
            Query(DeltaQuery {
                dir: working_dir.clone(),
            }),
        )
        .await
        .expect("discard must succeed");

        assert!(!base.exists(), "scratch base must be removed by discard");
    }

    /// `DELETE /api/workspace/discard?dir=<outside>` must reject paths outside
    /// the sync root, same as the delta endpoint.
    #[tokio::test]
    async fn discard_rejects_working_dir_outside_sync_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state(&tmp);

        let sync_root = tmp.path().join("workspace-sync");
        std::fs::create_dir_all(&sync_root).unwrap();
        let outside = tempfile::TempDir::new().unwrap();

        let result = discard_workspace(
            State(state),
            Query(DeltaQuery {
                dir: outside.path().to_string_lossy().into_owned(),
            }),
        )
        .await;

        assert!(result.is_err(), "path outside sync root must be rejected");
        let crate::error::ApiError(status, _msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
