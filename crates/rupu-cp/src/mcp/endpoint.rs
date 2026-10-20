//! `POST /mcp`: MCP over HTTP. Replies are JSON, or an SSE stream for a
//! `tools/call` that asked for progress.

use crate::state::AppState;
use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderName, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    routing::post,
    Json, Router,
};
use futures_util::StreamExt;
use rupu_mcp::{HandleCx, McpServer};
use serde_json::{json, Value};
use std::sync::Arc;

const SESSION_HEADER: &str = "mcp-session-id";

#[derive(Clone)]
struct McpState {
    server: Arc<McpServer>,
    loopback_bind: bool,
}

/// The `/mcp` route. GET and DELETE are 405 (no stream, no session delete).
/// `loopback_bind`: the CP is bound to a loopback address, so `Host` must
/// be a loopback name too.
pub fn routes(server: Arc<McpServer>, loopback_bind: bool) -> Router<AppState> {
    Router::new()
        .route("/mcp", post(handle).fallback(not_allowed))
        .with_state(McpState {
            server,
            loopback_bind,
        })
}

async fn not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST")],
        Json(json!({"error": "method not allowed: /mcp takes POST"})),
    )
        .into_response()
}

/// A `Host` header naming this machine's loopback: `localhost`,
/// `127.0.0.1` or `[::1]`, any port.
fn loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().map(|h| format!("[{h}]")),
        None => host.split(':').next().map(str::to_string),
    };
    name.is_some_and(|n| n.eq_ignore_ascii_case("localhost") || n == "127.0.0.1" || n == "[::1]")
}

/// The request may be answered: `Origin`, if present, names the `Host` it
/// was sent to, and on a loopback bind the `Host` is itself a loopback name
/// (a DNS-rebinding page sends its own name in both).
fn request_ok(headers: &HeaderMap, loopback_bind: bool) -> bool {
    if loopback_bind
        && !headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .is_some_and(loopback_host)
    {
        return false;
    }
    !headers.contains_key(header::ORIGIN) || crate::auth::same_origin(headers)
}

fn is_initialize(msg: &Value) -> bool {
    match msg {
        Value::Array(batch) => batch.iter().any(is_initialize),
        m => m.get("method").and_then(Value::as_str) == Some("initialize"),
    }
}

/// The client accepts an event stream and the body is one `tools/call` that
/// asked for progress.
fn wants_progress_stream(headers: &HeaderMap, msg: &Value) -> bool {
    let accepts = headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.to_ascii_lowercase().contains("text/event-stream"));
    accepts
        && msg.get("method").and_then(Value::as_str) == Some("tools/call")
        && msg.pointer("/params/_meta/progressToken").is_some()
}

/// Answers as `text/event-stream`: each progress notification, then the
/// reply, then the stream ends. The call runs in its own task, so a client
/// that drops the stream leaves it running; what it sends after that is
/// discarded.
fn progress_stream(
    server: Arc<McpServer>,
    msg: Value,
    session: Option<String>,
    host: Option<String>,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let reply = server
            .handle_with(
                msg,
                HandleCx {
                    session: session.as_deref(),
                    host: host.as_deref(),
                    notify: Some(tx.clone()),
                },
            )
            .await;
        // Same channel as the notifications, so the reply comes last.
        if let Some(v) = reply {
            let _ = tx.send(v);
        }
    });
    let events = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|v| {
        Ok::<_, std::convert::Infallible>(Event::default().event("message").data(v.to_string()))
    });
    Sse::new(events).into_response()
}

async fn handle(
    State(McpState {
        server,
        loopback_bind,
    }): State<McpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !request_ok(&headers, loopback_bind) {
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
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if wants_progress_stream(&headers, &msg) {
        return progress_stream(server, msg, session, host);
    }
    let reply = server
        .handle_with(
            msg,
            HandleCx {
                session: session.as_deref(),
                host: host.as_deref(),
                notify: None,
            },
        )
        .await;
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
