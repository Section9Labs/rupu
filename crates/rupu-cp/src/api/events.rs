//! `GET /api/events/stream` — global SSE event stream.
//!
//! The optional `?run=<id>` query param tails a single specific run. Without
//! it, the endpoint returns a **multiplexed firehose**: events from every
//! active run (and any run that starts while connected) merged into one stream
//! — see [`crate::sse::tail_all_events_sse`]. This is what the Live Events page
//! consumes.
//!
//! The optional `?host=<id>` param scopes the request to a specific host. When
//! the host is remote, `stream_run_events` on the `HostConnector` is called and
//! its pre-formatted SSE byte stream is passed through as-is. The `?run=` param
//! is required when `?host=` names a remote host.

use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{sse::Sse, IntoResponse as _, Response},
    routing::get,
    Router,
};
use rupu_orchestrator::runs::RunStoreError;

use crate::{
    error::ApiError,
    host::connector::{EventByteStream, HostConnectorError},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/events/stream", get(events_stream))
}

/// Wrap a pre-formatted `EventByteStream` in an axum `Response` with the
/// correct `text/event-stream` content-type. The bytes are already SSE frames
/// (`data: {...}\n\n`), so we pass them through without re-encoding.
fn proxy_event_byte_stream(stream: EventByteStream) -> Response {
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("valid response builder")
        .into_response()
}

/// `GET /api/events/stream[?run=<id>][?host=<id>]`
///
/// 1. If `?host=<remote-id>`: proxy `connector.stream_run_events(run)` — `?run=`
///    is required in this case. Unknown host id or run id → 404.
/// 2. If `?run=<id>` (local): tail that one run's `events.jsonl` (404 if unknown).
/// 3. Otherwise: return the merged live firehose across all runs on the local host.
async fn events_stream(
    State(s): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let run_id = params.get("run").map(String::as_str);
    let host_id = params.get("host").map(String::as_str).unwrap_or("local");

    // --- remote host: proxy stream_run_events ---
    if host_id != "local" {
        let conn = s.hosts.resolve(host_id).map_err(|e| match e {
            HostConnectorError::NotFound(_) => {
                ApiError::not_found(format!("host {host_id} not found"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
        let id = run_id.ok_or_else(|| {
            ApiError::bad_request("?run= is required when ?host= names a remote host")
        })?;
        let stream = conn.stream_run_events(id).await.map_err(|e| match e {
            HostConnectorError::NotFound(_) => {
                ApiError::not_found(format!("run {id} not found on {host_id}"))
            }
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
        return Ok(proxy_event_byte_stream(stream));
    }

    // --- local: explicit run parameter ---
    if let Some(id) = run_id {
        s.run_store.load(id).map_err(|e| match e {
            RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
            other => ApiError::internal(other.to_string()),
        })?;
        let events_path = s.run_store.events_path(id);
        let sse = crate::sse::tail_events_sse(events_path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        return Ok(sse.into_response());
    }

    // --- local: merged firehose across every run ---
    let sse: Sse<_> = crate::sse::tail_all_events_sse(s.run_store.clone()).await;
    Ok(sse.into_response())
}
