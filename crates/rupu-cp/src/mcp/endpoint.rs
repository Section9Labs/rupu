//! `POST /mcp`: MCP over HTTP (JSON replies, no server-initiated stream).

use crate::state::AppState;
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rupu_mcp::McpServer;
use serde_json::{json, Value};
use std::sync::Arc;

const SESSION_HEADER: &str = "mcp-session-id";

#[derive(Clone)]
struct McpState(Arc<McpServer>);

/// The `/mcp` route. GET and DELETE are 405 (no stream, no session delete).
pub fn routes(server: Arc<McpServer>) -> Router<AppState> {
    Router::new()
        .route("/mcp", post(handle).fallback(not_allowed))
        .with_state(McpState(server))
}

async fn not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        Json(json!({"error": "method not allowed: /mcp takes POST"})),
    )
        .into_response()
}

/// True when `Origin` is absent, or names the authority the request was
/// sent to.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let authority = origin
        .to_str()
        .ok()
        .and_then(|o| {
            o.strip_prefix("http://")
                .or_else(|| o.strip_prefix("https://"))
        })
        .map(|a| a.trim_end_matches('/'));
    matches!((host, authority), (Some(h), Some(a)) if a.eq_ignore_ascii_case(h))
}

fn is_initialize(msg: &Value) -> bool {
    match msg {
        Value::Array(batch) => batch.iter().any(is_initialize),
        m => m.get("method").and_then(Value::as_str) == Some("initialize"),
    }
}

async fn handle(
    State(McpState(server)): State<McpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !origin_ok(&headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(
                json!({"error": "cross-origin request refused: /mcp answers its own origin only"}),
            ),
        )
            .into_response();
    }
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}})),
            )
                .into_response();
        }
    };
    let supplied = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let minted = is_initialize(&msg).then(|| format!("mcp_{}", ulid::Ulid::new()));
    let session = minted.clone().or(supplied);
    let reply = server.handle(msg, session.as_deref()).await;
    let mut resp = match reply {
        Some(v) => Json(v).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    };
    if let Some(id) = minted {
        if let Ok(v) = id.parse() {
            resp.headers_mut()
                .insert(HeaderName::from_static(SESSION_HEADER), v);
        }
    }
    resp
}
