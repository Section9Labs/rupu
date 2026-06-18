//! `GET /api/events/stream` — global SSE event stream (Phase-1).
//!
//! **Phase-1 scope**: single-run tailing. The optional `?run=<id>` query param
//! selects a specific run to tail; if omitted, the most-recent non-terminal
//! run is chosen (falling back to the most-recent run overall). A true
//! global multiplex that fans events from every concurrent run through one
//! stream is deferred to a later phase (when the in-process executor's
//! broadcast channel is wired into the control plane).

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse as _, Response,
    },
    routing::get,
    Router,
};
use futures_util::stream;
use rupu_orchestrator::runs::{RunStatus, RunStoreError};
use tracing::info;

use crate::{error::ApiError, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/events/stream", get(events_stream))
}

/// `GET /api/events/stream[?run=<id>]`
///
/// Streams SSE events for a selected run. Selection logic:
/// 1. If `?run=<id>` is given, tail that run (404 if unknown).
/// 2. Otherwise, pick the most-recent non-terminal run (Running /
///    Pending / AwaitingApproval) if any; else the most-recent run overall.
/// 3. If there are no runs at all, return an idle SSE stream that holds the
///    connection open (with 15 s keep-alives) until the client disconnects.
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

    // --- pick a run automatically ---
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    if runs.is_empty() {
        info!("GET /api/events/stream: no runs to tail; holding connection open with idle stream");
        let idle: stream::Pending<Result<SseEvent, Infallible>> = stream::pending();
        let sse = Sse::new(idle).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)));
        return Ok(sse.into_response());
    }

    // Prefer a non-terminal run (newest first, list() returns newest-first).
    let chosen = runs
        .iter()
        .find(|r| {
            matches!(
                r.status,
                RunStatus::Running | RunStatus::Pending | RunStatus::AwaitingApproval
            )
        })
        .or_else(|| runs.first())
        .expect("runs is non-empty; a first() always exists");

    let events_path = s.run_store.events_path(&chosen.id);
    let sse = crate::sse::tail_events_sse(events_path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(sse.into_response())
}

/// Type alias to keep the idle-stream arm tidy in callers that need the
/// concrete type. Not currently exported; kept for potential reuse.
#[allow(dead_code)]
type IdleSseStream = Sse<stream::Pending<Result<SseEvent, Infallible>>>;
