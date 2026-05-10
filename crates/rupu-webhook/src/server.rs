//! Axum-based HTTP receiver. Exposes:
//!
//!   POST /webhook/github   — `x-hub-signature-256` HMAC-validated
//!   POST /webhook/gitlab   — `x-gitlab-token` shared-secret validated
//!   POST /webhook/linear   — `linear-signature` HMAC-validated
//!   GET  /healthz          — liveness probe
//!
//! Webhook routes return 401 on signature/token mismatch, 400
//! on malformed payload, and 200 with a JSON summary of dispatched
//! workflows on success (including the no-match case — `{ "fired": [] }`).
//!
//! `serve(config)` blocks the current task. Drop the future to stop.

use crate::dispatch::{dispatch_event, DispatchedWorkflow, WorkflowDispatcher};
use crate::event_vocab::{
    map_github_event, map_gitlab_event, map_linear_event, normalize_linear_event_payload,
};
use crate::signature::{verify_github_signature, verify_gitlab_token, verify_linear_signature};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use rupu_orchestrator::Workflow;
use serde::Serialize;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum WebhookError {
    #[error("bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("serve: {0}")]
    Serve(#[source] std::io::Error),
}

/// All the wiring the server needs at startup. The receiver is
/// stateless beyond this — every request loads workflows fresh from
/// the workspace via `workflow_loader`.
pub struct WebhookConfig {
    pub addr: SocketAddr,
    pub github_secret: Option<Vec<u8>>,
    pub gitlab_token: Option<Vec<u8>>,
    pub linear_secret: Option<Vec<u8>>,
    /// Closure that returns the candidate workflows to consider on
    /// each request. Called fresh per request so authors can edit
    /// workflow files without restarting the server.
    pub workflow_loader: Arc<dyn Fn() -> Vec<(String, Workflow)> + Send + Sync>,
    /// Dispatches a matched workflow by name. Production: thin
    /// wrapper around `cmd::workflow::run_by_name`.
    pub dispatcher: Arc<dyn WorkflowDispatcher>,
    /// Best-effort observer for every mapped webhook delivery.
    /// Failures are logged and do not change the HTTP response.
    pub observer: Option<Arc<dyn WebhookObserver>>,
}

#[derive(Clone)]
struct AppState {
    github_secret: Option<Arc<Vec<u8>>>,
    gitlab_token: Option<Arc<Vec<u8>>>,
    linear_secret: Option<Arc<Vec<u8>>>,
    workflow_loader: Arc<dyn Fn() -> Vec<(String, Workflow)> + Send + Sync>,
    dispatcher: Arc<dyn WorkflowDispatcher>,
    observer: Option<Arc<dyn WebhookObserver>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookSource {
    Github,
    Gitlab,
    Linear,
}

#[derive(Debug, Clone)]
pub struct WebhookEvent {
    pub source: WebhookSource,
    pub event_id: String,
    pub delivery_id: Option<String>,
    pub payload: Value,
}

#[async_trait]
pub trait WebhookObserver: Send + Sync {
    async fn observe(&self, event: &WebhookEvent) -> anyhow::Result<()>;
}

#[derive(Serialize)]
struct WebhookResponse {
    event: Option<String>,
    fired: Vec<DispatchedWorkflow>,
}

impl Serialize for DispatchedWorkflow {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("DispatchedWorkflow", 5)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("fired", &self.fired)?;
        st.serialize_field("error", &self.error)?;
        if !self.run_id.is_empty() {
            st.serialize_field("run_id", &self.run_id)?;
        } else {
            st.skip_field("run_id")?;
        }
        if self.awaiting_step_id.is_some() {
            st.serialize_field("awaiting_step_id", &self.awaiting_step_id)?;
        } else {
            st.skip_field("awaiting_step_id")?;
        }
        st.end()
    }
}

/// Start the receiver. Blocks the current task. Drop the future to stop.
pub async fn serve(config: WebhookConfig) -> Result<(), WebhookError> {
    let state = AppState {
        github_secret: config.github_secret.map(Arc::new),
        gitlab_token: config.gitlab_token.map(Arc::new),
        linear_secret: config.linear_secret.map(Arc::new),
        workflow_loader: config.workflow_loader,
        dispatcher: config.dispatcher,
        observer: config.observer,
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/webhook/github", post(github_handler))
        .route("/webhook/gitlab", post(gitlab_handler))
        .route("/webhook/linear", post(linear_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .map_err(|e| WebhookError::Bind {
            addr: config.addr,
            source: e,
        })?;
    info!(addr = %config.addr, "webhook receiver listening");
    axum::serve(listener, app)
        .await
        .map_err(WebhookError::Serve)?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn github_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let secret = match &state.github_secret {
        Some(s) => s.clone(),
        None => {
            warn!("github webhook received but no secret configured; rejecting");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "github secret not configured",
            )
                .into_response();
        }
    };
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if let Err(e) = verify_github_signature(&secret, &body, sig) {
        warn!(error = %e, "github signature verification failed");
        return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
    }

    let event_header = match headers.get("x-github-event").and_then(|v| v.to_str().ok()) {
        Some(h) => h.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, "missing X-GitHub-Event header").into_response();
        }
    };

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")).into_response();
        }
    };

    let Some(event_id) = map_github_event(&event_header, &payload) else {
        info!(event = %event_header, "unrecognized github event; ignoring");
        return Json(WebhookResponse {
            event: None,
            fired: vec![],
        })
        .into_response();
    };

    observe_event(
        state.observer.as_ref(),
        WebhookEvent {
            source: WebhookSource::Github,
            event_id: event_id.clone(),
            delivery_id: headers
                .get("x-github-delivery")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            payload: payload.clone(),
        },
    )
    .await;

    let candidates = (state.workflow_loader)();
    let fired = dispatch_event(&event_id, &payload, &candidates, state.dispatcher.as_ref()).await;
    Json(WebhookResponse {
        event: Some(event_id),
        fired,
    })
    .into_response()
}

async fn gitlab_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let token = match &state.gitlab_token {
        Some(t) => t.clone(),
        None => {
            warn!("gitlab webhook received but no token configured; rejecting");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "gitlab token not configured",
            )
                .into_response();
        }
    };
    let provided = headers.get("x-gitlab-token").and_then(|v| v.to_str().ok());
    if let Err(e) = verify_gitlab_token(&token, provided) {
        warn!(error = %e, "gitlab token verification failed");
        return (StatusCode::UNAUTHORIZED, "token mismatch").into_response();
    }

    let event_header = match headers.get("x-gitlab-event").and_then(|v| v.to_str().ok()) {
        Some(h) => h.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, "missing X-Gitlab-Event header").into_response();
        }
    };

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")).into_response();
        }
    };

    let Some(event_id) = map_gitlab_event(&event_header, &payload) else {
        info!(event = %event_header, "unrecognized gitlab event; ignoring");
        return Json(WebhookResponse {
            event: None,
            fired: vec![],
        })
        .into_response();
    };

    observe_event(
        state.observer.as_ref(),
        WebhookEvent {
            source: WebhookSource::Gitlab,
            event_id: event_id.clone(),
            delivery_id: headers
                .get("x-gitlab-event-uuid")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            payload: payload.clone(),
        },
    )
    .await;

    let candidates = (state.workflow_loader)();
    let fired = dispatch_event(&event_id, &payload, &candidates, state.dispatcher.as_ref()).await;
    Json(WebhookResponse {
        event: Some(event_id),
        fired,
    })
    .into_response()
}

async fn linear_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let secret = match &state.linear_secret {
        Some(s) => s.clone(),
        None => {
            warn!("linear webhook received but no secret configured; rejecting");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "linear secret not configured",
            )
                .into_response();
        }
    };

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")).into_response();
        }
    };

    if let Err(e) = verify_linear_signature(
        &secret,
        &body,
        headers
            .get("linear-signature")
            .and_then(|v| v.to_str().ok()),
        payload.get("webhookTimestamp").and_then(|v| v.as_i64()),
    ) {
        warn!(error = %e, "linear signature verification failed");
        return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
    }

    let event_header = headers
        .get("linear-event")
        .and_then(|value| value.to_str().ok())
        .or_else(|| payload.get("type").and_then(|value| value.as_str()))
        .unwrap_or_default()
        .to_string();

    let Some(event_id) = map_linear_event(&event_header, &payload) else {
        info!(event = %event_header, "unrecognized linear event; ignoring");
        return Json(WebhookResponse {
            event: None,
            fired: vec![],
        })
        .into_response();
    };

    let normalized_payload = normalize_linear_event_payload(&payload);
    observe_event(
        state.observer.as_ref(),
        WebhookEvent {
            source: WebhookSource::Linear,
            event_id: event_id.clone(),
            delivery_id: headers
                .get("linear-delivery")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
                .or_else(|| {
                    payload
                        .get("webhookId")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                }),
            payload: normalized_payload.clone(),
        },
    )
    .await;

    let candidates = (state.workflow_loader)();
    let fired = dispatch_event(
        &event_id,
        &normalized_payload,
        &candidates,
        state.dispatcher.as_ref(),
    )
    .await;
    Json(WebhookResponse {
        event: Some(event_id),
        fired,
    })
    .into_response()
}

async fn observe_event(observer: Option<&Arc<dyn WebhookObserver>>, event: WebhookEvent) {
    let Some(observer) = observer else {
        return;
    };
    if let Err(error) = observer.observe(&event).await {
        warn!(
            event = %event.event_id,
            source = ?event.source,
            delivery = event.delivery_id.as_deref().unwrap_or("-"),
            %error,
            "webhook observer failed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::Response;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::sync::Mutex;

    type HmacSha256 = Hmac<Sha256>;

    struct NoopDispatcher;

    #[async_trait]
    impl WorkflowDispatcher for NoopDispatcher {
        async fn dispatch(
            &self,
            _workflow_name: &str,
            _event: &Value,
        ) -> anyhow::Result<crate::dispatch::DispatchOutcome> {
            Ok(crate::dispatch::DispatchOutcome::default())
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<WebhookEvent>>,
        fail: bool,
    }

    #[async_trait]
    impl WebhookObserver for RecordingObserver {
        async fn observe(&self, event: &WebhookEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event.clone());
            if self.fail {
                anyhow::bail!("boom");
            }
            Ok(())
        }
    }

    fn github_signature(secret: &[u8], body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).expect("hmac");
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn state_with_observer(observer: Arc<dyn WebhookObserver>) -> AppState {
        AppState {
            github_secret: Some(Arc::new(b"secret".to_vec())),
            gitlab_token: None,
            linear_secret: Some(Arc::new(b"linear-secret".to_vec())),
            workflow_loader: Arc::new(Vec::new),
            dispatcher: Arc::new(NoopDispatcher),
            observer: Some(observer),
        }
    }

    fn response_status(response: Response) -> StatusCode {
        response.status()
    }

    #[tokio::test]
    async fn github_handler_notifies_observer() {
        let observer = Arc::new(RecordingObserver::default());
        let body = br#"{
          "action":"labeled",
          "issue":{"number":123},
          "repository":{"name":"rupu","owner":{"login":"Section9Labs"}}
        }"#;
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "issues".parse().unwrap());
        headers.insert(
            "x-hub-signature-256",
            github_signature(b"secret", body).parse().unwrap(),
        );
        headers.insert("x-github-delivery", "delivery-123".parse().unwrap());

        let response = github_handler(
            State(state_with_observer(observer.clone())),
            headers,
            Bytes::from_static(body),
        )
        .await
        .into_response();

        assert_eq!(response_status(response), StatusCode::OK);
        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, WebhookSource::Github);
        assert_eq!(events[0].event_id, "github.issue.labeled");
        assert_eq!(events[0].delivery_id.as_deref(), Some("delivery-123"));
    }

    #[tokio::test]
    async fn observer_failure_does_not_fail_request() {
        let observer = Arc::new(RecordingObserver {
            events: Mutex::new(Vec::new()),
            fail: true,
        });
        let body = br#"{
          "action":"labeled",
          "issue":{"number":123},
          "repository":{"name":"rupu","owner":{"login":"Section9Labs"}}
        }"#;
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "issues".parse().unwrap());
        headers.insert(
            "x-hub-signature-256",
            github_signature(b"secret", body).parse().unwrap(),
        );

        let response = github_handler(
            State(state_with_observer(observer)),
            headers,
            Bytes::from_static(body),
        )
        .await
        .into_response();

        assert_eq!(response_status(response), StatusCode::OK);
    }

    fn linear_signature(secret: &[u8], body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).expect("hmac");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[tokio::test]
    async fn linear_handler_notifies_observer_with_normalized_payload() {
        let observer = Arc::new(RecordingObserver::default());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let body = format!(
            r#"{{
              "action":"update",
              "type":"Issue",
              "url":"https://linear.app/acme/issue/ENG-123",
              "organizationId":"org-1",
              "data":{{
                "id":"issue-1",
                "identifier":"ENG-123",
                "stateId":"state-in-progress",
                "projectId":"project-core",
                "cycleId":"cycle-42",
                "teamId":"team-1"
              }},
              "updatedFrom":{{
                "stateId":"state-todo",
                "projectId":"project-backlog",
                "cycleId":"cycle-41"
              }},
              "webhookTimestamp":{ts},
              "webhookId":"delivery-xyz"
            }}"#
        );
        let mut headers = HeaderMap::new();
        headers.insert("linear-event", "Issue".parse().unwrap());
        headers.insert(
            "linear-signature",
            linear_signature(b"linear-secret", body.as_bytes())
                .parse()
                .unwrap(),
        );
        headers.insert("linear-delivery", "delivery-xyz".parse().unwrap());

        let response = linear_handler(
            State(state_with_observer(observer.clone())),
            headers,
            Bytes::from(body.into_bytes()),
        )
        .await
        .into_response();

        assert_eq!(response_status(response), StatusCode::OK);
        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, WebhookSource::Linear);
        assert_eq!(events[0].event_id, "linear.issue.updated");
        assert_eq!(events[0].delivery_id.as_deref(), Some("delivery-xyz"));
        assert_eq!(events[0].payload["subject"]["ref"], "ENG-123");
        assert_eq!(events[0].payload["state"]["category"], "workflow_state");
        assert_eq!(events[0].payload["project"]["after"]["id"], "project-core");
        assert_eq!(events[0].payload["cycle"]["before"]["id"], "cycle-41");
    }
}
