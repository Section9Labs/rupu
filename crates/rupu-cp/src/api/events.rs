//! `GET /api/events/stream` — global SSE event stream.
//!
//! The optional `?run=<id>` query param tails a single specific run. Without
//! it, the endpoint returns a **multiplexed firehose**: events from every
//! active run (and any run that starts while connected) merged into one stream
//! — see [`crate::sse::tail_all_events_sse`]. This is what the Live Events page
//! consumes.

use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    response::{sse::Sse, IntoResponse as _, Response},
    routing::get,
    Router,
};
use rupu_orchestrator::runs::RunStoreError;

use crate::{error::ApiError, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/events/stream", get(events_stream))
}

/// `GET /api/events/stream[?run=<id>]`
///
/// 1. If `?run=<id>` is given, tail that one run (404 if unknown).
/// 2. Otherwise, return the merged live firehose across all runs. When there
///    are no runs the firehose simply holds the connection open (15 s
///    keep-alives) and starts emitting as soon as a run begins.
async fn events_stream(
    State(s): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // --- explicit run parameter ---
    if let Some(id) = params.get("run") {
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

    // --- merged firehose across every run ---
    let sse: Sse<_> = crate::sse::tail_all_events_sse(s.run_store.clone()).await;
    Ok(sse.into_response())
}
