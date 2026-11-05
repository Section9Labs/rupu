//! Session-local orchestration configuration and effective lead-tool inventory.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_runtime::session_orchestration::{inspect, SessionOrchestration};
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct HostQuery {
    host: Option<String>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/sessions/:id/orchestration",
        get(get_config).put(put_config),
    )
}
async fn get_config(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HostQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    tokio::task::spawn_blocking(move || response(&s, &id, q.host.as_deref(), None))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}
async fn put_config(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HostQuery>,
    Json(config): Json<SessionOrchestration>,
) -> ApiResult<Json<serde_json::Value>> {
    tokio::task::spawn_blocking(move || response(&s, &id, q.host.as_deref(), Some(config)))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
}
fn response(
    s: &AppState,
    id: &str,
    host: Option<&str>,
    update: Option<SessionOrchestration>,
) -> ApiResult<Json<serde_json::Value>> {
    crate::api::runs::validate_id(id)?;
    if host.is_some_and(|h| h != "local") {
        return Err(ApiError::not_available(
            "session orchestration requires a direct connection to the owning host",
        ));
    }
    if update.is_some() {
        crate::api::config::require_writable_to(s, "changing session orchestration")?;
    }
    super::active_session(s, id, "orchestration")?;
    let dir = s.global_dir.join("sessions").join(id);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.join("session.json")).map_err(|e| ApiError::internal(e.to_string()))?,
    )
    .map_err(|e| ApiError::internal(e.to_string()))?;
    if record["session_id"].as_str() != Some(id) {
        return Err(ApiError::conflict(
            "session identity does not match its directory",
        ));
    }
    let settings = rupu_runtime::session_settings::SessionSettings::read(&dir)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    settings
        .validate()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let declared: Option<Vec<String>> = settings
        .tools
        .or(serde_json::from_value(record["agent_tools"].clone())
            .map_err(|e| ApiError::internal(e.to_string()))?);
    let mode = record["permission_mode"]
        .as_str()
        .and_then(|mode| rupu_tools::PermissionMode::parse(mode).ok())
        .unwrap_or(rupu_tools::PermissionMode::Readonly);
    let config = update
        .clone()
        .map(Ok)
        .unwrap_or_else(|| SessionOrchestration::read(&dir))
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let tools = inspect(
        config.enabled,
        declared.as_deref(),
        record["messaging"] != false,
        mode,
    )
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let config = if update.is_some() {
        config
            .save(&dir)
            .map_err(|e| ApiError::conflict(e.to_string()))?
    } else {
        config
    };
    Ok(Json(serde_json::json!({
        "session_id": id, "host_id": "local", "enabled": config.enabled, "revision": config.revision,
        "context_id": format!("session:{id}"), "applies": "next_turn", "execution": "turn_bound", "tools": tools,
        "permission_mode": mode.to_string(),
        "limits": [
            "Unfinished children checkpoint at turn end and resume on the next enabled turn; no background or time-based scheduler.",
            "No Agentiflow goal evaluator, envelope budget or aggregate flow coverage is attached.",
            "Availability describes configured services and grants. Provider credentials, child allowlists, depth and workflow inputs are checked on execution.",
            "Disabling suspends continuation and retains durable task and message records; active turns keep their snapshot."
        ]
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoLaunch;
    #[async_trait::async_trait]
    impl crate::launcher::RunLauncher for NoLaunch {
        async fn launch(
            &self,
            _: crate::launcher::LaunchRequest,
        ) -> Result<String, crate::launcher::LaunchError> {
            unreachable!("configuration never launches work")
        }
    }
    fn fixture() -> (tempfile::TempDir, AppState) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sessions/ses_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            r#"{"session_id":"ses_test","agent_tools":[],"permission_mode":"readonly"}"#,
        )
        .unwrap();
        let state = AppState::new(tmp.path().into(), rupu_config::PricingConfig::default());
        (tmp, state)
    }
    #[test]
    fn config_and_host_guards() {
        let (tmp, s) = fixture();
        let initial = response(&s, "ses_test", None, None).unwrap().0;
        assert_eq!(initial["revision"], 0);
        assert_eq!(initial["context_id"], "session:ses_test");
        assert!(!tmp
            .path()
            .join("sessions/ses_test/orchestration.json")
            .exists());
        assert_eq!(
            response(
                &s,
                "ses_test",
                None,
                Some(SessionOrchestration {
                    revision: 0,
                    enabled: true
                })
            )
            .unwrap_err()
            .0,
            axum::http::StatusCode::NOT_IMPLEMENTED
        );
        assert!(!tmp
            .path()
            .join("sessions/ses_test/orchestration.json")
            .exists());
        let s = s.with_launcher(Some(std::sync::Arc::new(NoLaunch)));
        let save = SessionOrchestration {
            revision: 0,
            enabled: true,
        };
        assert_eq!(
            response(&s, "ses_test", Some("remote"), Some(save.clone()))
                .unwrap_err()
                .0,
            axum::http::StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            response(&s, "ses_test", None, Some(save.clone()))
                .unwrap()
                .0["revision"],
            1
        );
        assert_eq!(
            response(&s, "ses_test", None, Some(save)).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        assert!(response(&s, "../ses_test", None, None).is_err());
        assert!(response(&s, "missing", None, None).is_err());
    }
    #[test]
    fn archived_and_mismatched_sessions_are_refused_without_writes() {
        let (tmp, s) = fixture();
        let s = s.with_launcher(Some(std::sync::Arc::new(NoLaunch)));
        let dir = tmp.path().join("sessions/ses_test");
        std::fs::write(dir.join("session.json"), r#"{"session_id":"ses_other"}"#).unwrap();
        assert_eq!(
            response(
                &s,
                "ses_test",
                None,
                Some(SessionOrchestration {
                    revision: 0,
                    enabled: true
                })
            )
            .unwrap_err()
            .0,
            axum::http::StatusCode::CONFLICT
        );
        std::fs::create_dir_all(tmp.path().join("sessions-archive")).unwrap();
        std::fs::rename(&dir, tmp.path().join("sessions-archive/ses_test")).unwrap();
        assert_eq!(
            response(&s, "ses_test", None, None).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        assert!(!tmp
            .path()
            .join("sessions-archive/ses_test/orchestration.json")
            .exists());
    }

    #[tokio::test]
    async fn wire_rejects_unknown_fields() {
        use tower::ServiceExt;
        let (_tmp, s) = fixture();
        let response = routes()
            .with_state(s)
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/api/sessions/ses_test/orchestration")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"revision":0,"enabled":true,"flow_id":"unrelated"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
