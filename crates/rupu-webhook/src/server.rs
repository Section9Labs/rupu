//! Axum-based HTTP receiver. Exposes:
//!
//!   POST /webhook/github   — `x-hub-signature-256` HMAC-validated
//!   POST /webhook/gitlab   — `x-gitlab-token` shared-secret validated
//!   GET  /healthz          — liveness probe
//!
//! Both webhook routes return 401 on signature/token mismatch, 400
//! on malformed payload, and 200 with a JSON summary of dispatched
//! workflows on success (including the no-match case — `{ "fired": [] }`).
//!
//! `serve(config)` blocks the current task. Drop the future to stop.

use crate::dispatch::{dispatch_event, DispatchedWorkflow, WorkflowDispatcher};
use crate::event_vocab::{map_github_event, map_gitlab_event};
use crate::signature::{verify_github_signature, verify_gitlab_token};
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
    /// Closure that returns the candidate workflows to consider on
    /// each request. Called fresh per request so authors can edit
    /// workflow files without restarting the server.
    pub workflow_loader: Arc<dyn Fn() -> Vec<(String, Workflow)> + Send + Sync>,
    /// Dispatches a matched workflow by name. Production: thin
    /// wrapper around `cmd::workflow::run_by_name`.
    pub dispatcher: Arc<dyn WorkflowDispatcher>,
}

#[derive(Clone)]
struct AppState {
    github_secret: Option<Arc<Vec<u8>>>,
    gitlab_token: Option<Arc<Vec<u8>>>,
    workflow_loader: Arc<dyn Fn() -> Vec<(String, Workflow)> + Send + Sync>,
    dispatcher: Arc<dyn WorkflowDispatcher>,
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
        workflow_loader: config.workflow_loader,
        dispatcher: config.dispatcher,
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/webhook/github", post(github_handler))
        .route("/webhook/gitlab", post(gitlab_handler))
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

    let candidates = (state.workflow_loader)();
    let fired = dispatch_event(&event_id, &payload, &candidates, state.dispatcher.as_ref()).await;
    Json(WebhookResponse {
        event: Some(event_id),
        fired,
    })
    .into_response()
}
