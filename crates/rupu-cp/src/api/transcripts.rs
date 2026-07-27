//! Transcript archive / delete — `POST /api/transcripts/:id/archive` and
//! `DELETE /api/transcripts/:id`.
//!
//! Acts on STANDALONE agent-run transcripts only (a flat
//! `<global>/transcripts/<run_id>.{meta.json,jsonl}` pair). No `restore`
//! route: `rupu transcript restore` does not exist.
//!
//! Mirrors `api/sessions.rs`'s archive/restore/delete triplet: `validate_id`
//! guard first, dispatch through the local [`crate::transcript_mutator::TranscriptMutator`]
//! port (501 when absent — i.e. not running under `rupu cp serve`), or proxy
//! to a resolved remote host with `?host=<id>`.

use crate::{
    error::{ApiError, ApiResult},
    host::connector::HostConnectorError,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/transcripts/:id/archive", post(archive_transcript))
        .route("/api/transcripts/:id", axum::routing::delete(delete_transcript))
}

/// Optional `?host=<id>` query param, same shape as `sessions.rs`'s
/// `SessionHostQuery`.
#[derive(Deserialize, Default)]
struct TranscriptHostQuery {
    #[serde(default)]
    host: Option<String>,
}

async fn mutate_transcript(
    s: &AppState,
    id: &str,
    action: crate::transcript_mutator::TranscriptAction,
) -> ApiResult<Json<serde_json::Value>> {
    use crate::transcript_mutator::TranscriptMutateError as E;
    let m = s.transcript_mutator.clone().ok_or_else(|| {
        ApiError::not_available("transcript archive/delete requires `rupu cp serve`")
    })?;
    m.mutate(id, action).await.map_err(|e| match e {
        E::NotFound(_) => ApiError::not_found(format!("transcript {id} not found")),
        E::Invalid(msg) => ApiError::conflict(msg),
        E::Failed { message, .. } => ApiError::internal(message),
    })?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id })))
}

/// Map a [`HostConnectorError`] from a proxied transcript archive/delete call
/// to an [`ApiError`], mirroring `sessions.rs`'s
/// `map_host_session_mutate_err` (`NotFound` → 404, `Invalid`-shaped → 409,
/// `Unsupported` → 501 for a transport that genuinely can't do this, never a
/// silent no-op).
fn map_host_transcript_mutate_err(e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Invalid(m) => ApiError::conflict(m),
        // A remote CP-of-CP hop (HttpHostConnector) already mapped ITS OWN
        // local refusal to 409 before it reached us — preserve that status
        // rather than flattening it into a 500 below.
        HostConnectorError::Remote(409, m) => ApiError::conflict(m),
        HostConnectorError::Unsupported(m) => ApiError::not_available(m),
        other => ApiError::internal(other.to_string()),
    }
}

/// `POST /api/transcripts/:id/archive[?host=<id>]`.
///
/// Without `?host=` (or `?host=local`): dispatches through the local
/// [`crate::transcript_mutator::TranscriptMutator`] port (`mutate_transcript`).
///
/// With `?host=<remote-id>`: proxies via [`HostConnector::archive_transcript`]
/// and returns `{ "ok": true, "id", "host_id": "<id>" }`.
async fn archive_transcript(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TranscriptHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::api::runs::validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        conn.archive_transcript(&id)
            .await
            .map_err(map_host_transcript_mutate_err)?;
        return Ok(Json(serde_json::json!({ "ok": true, "id": id, "host_id": host })));
    }
    mutate_transcript(&s, &id, crate::transcript_mutator::TranscriptAction::Archive).await
}

/// `DELETE /api/transcripts/:id[?host=<id>]`. See [`archive_transcript`]'s doc.
async fn delete_transcript(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TranscriptHostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::api::runs::validate_id(&id)?;
    let host = q.host.as_deref().unwrap_or("local");
    if host != "local" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        conn.delete_transcript(&id)
            .await
            .map_err(map_host_transcript_mutate_err)?;
        return Ok(Json(serde_json::json!({ "ok": true, "id": id, "host_id": host })));
    }
    mutate_transcript(&s, &id, crate::transcript_mutator::TranscriptAction::Delete).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript_mutator::{TranscriptAction, TranscriptMutateError, TranscriptMutator};

    struct StubMutator;
    #[async_trait::async_trait]
    impl TranscriptMutator for StubMutator {
        async fn mutate(
            &self,
            id: &str,
            action: TranscriptAction,
        ) -> Result<(), TranscriptMutateError> {
            if id == "missing" {
                return Err(TranscriptMutateError::NotFound(id.into()));
            }
            if action == TranscriptAction::Archive && id == "session-owned" {
                return Err(TranscriptMutateError::Invalid(
                    "transcript session-owned is managed by session ses_1".into(),
                ));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn archive_transcript_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_transcript_mutator(Some(std::sync::Arc::new(StubMutator)));
        let resp = archive_transcript(
            State(state),
            Path("run_1".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .expect("ok");
        assert_eq!(
            resp.0,
            serde_json::json!({ "ok": true, "id": "run_1" })
        );
    }

    #[tokio::test]
    async fn delete_transcript_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_transcript_mutator(Some(std::sync::Arc::new(StubMutator)));
        let resp = delete_transcript(
            State(state),
            Path("run_1".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .expect("ok");
        assert_eq!(
            resp.0,
            serde_json::json!({ "ok": true, "id": "run_1" })
        );
    }

    /// SAFETY-CRITICAL: a session-owned transcript must be REFUSED (409),
    /// never silently mutated. Proves the `ensure_standalone_transcript`
    /// guard's error (surfaced through the subprocess adapter as
    /// `TranscriptMutateError::Invalid`) reaches the HTTP layer as a
    /// conflict, and that the stub mutator (standing in for the real
    /// subprocess call) is the only thing that could have acted — so
    /// "refused" here is equivalent to "nothing was moved or removed".
    #[tokio::test]
    async fn archive_transcript_session_owned_is_refused_with_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_transcript_mutator(Some(std::sync::Arc::new(StubMutator)));
        let err = archive_transcript(
            State(state),
            Path("session-owned".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
        assert!(err.1.contains("is managed by session"));
    }

    #[tokio::test]
    async fn archive_transcript_missing_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default())
            .with_transcript_mutator(Some(std::sync::Arc::new(StubMutator)));
        let err = archive_transcript(
            State(state),
            Path("missing".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn archive_transcript_without_adapter_is_501() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default()); // no mutator
        let err = archive_transcript(
            State(state),
            Path("run_1".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn delete_transcript_without_adapter_is_501() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default()); // no mutator
        let err = delete_transcript(
            State(state),
            Path("run_1".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    /// Traversal ids must be rejected with 400 BEFORE the mutator is reached.
    /// The no-mutator state (→ 501) is never hit when the id is invalid,
    /// which proves the guard fires first.
    #[tokio::test]
    async fn archive_transcript_traversal_id_is_bad_request() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = archive_transcript(
            State(state),
            Path("../../etc".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_transcript_traversal_id_is_bad_request() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = delete_transcript(
            State(state),
            Path("../../etc".to_string()),
            Query(TranscriptHostQuery::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn archive_transcript_unknown_host_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::new(tmp.path().to_path_buf(), Default::default());
        let err = archive_transcript(
            State(state),
            Path("run_1".to_string()),
            Query(TranscriptHostQuery {
                host: Some("host_nonexistent".into()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    }

    // Minor finding: a remote CP-of-CP hop's own 409 refusal must surface as
    // 409 here too, not fall through to the generic 500 `other` arm.
    #[test]
    fn map_host_transcript_mutate_err_preserves_remote_409_as_conflict() {
        let err = map_host_transcript_mutate_err(HostConnectorError::Remote(
            409,
            "still running".into(),
        ));
        assert_eq!(err.0, axum::http::StatusCode::CONFLICT);
    }
}
