//! SSE bridge helpers — convert a [`FileTailRunSource`] stream into an axum
//! [`Sse`] response.

use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::{Stream, StreamExt as _};
use rupu_orchestrator::executor::FileTailRunSource;

/// Tail a run's `events.jsonl` as an SSE stream. Each rupu [`Event`] becomes
/// one SSE `data:` line of JSON. The stream is live — it stays open and emits
/// events as the run progresses, never terminating on its own.
///
/// [`Event`]: rupu_orchestrator::executor::Event
pub async fn tail_events_sse(
    events_path: PathBuf,
) -> std::io::Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let source = FileTailRunSource::open(&events_path).await?;
    let stream = source.map(|ev| {
        let sse = SseEvent::default()
            .json_data(&ev)
            .unwrap_or_else(|_| SseEvent::default().comment("event serialize error"));
        Ok::<_, Infallible>(sse)
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
