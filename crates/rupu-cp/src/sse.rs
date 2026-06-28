//! SSE bridge helpers — convert a [`FileTailRunSource`] stream into an axum
//! [`Sse`] response.

use std::collections::HashSet;
use std::convert::Infallible;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::{Stream, StreamExt as _};
use rupu_orchestrator::executor::{Event, FileTailRunSource};
use rupu_orchestrator::runs::{RunStatus, RunStore};
use tokio::sync::mpsc;

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

/// A `Stream` over a plain mpsc receiver — lets us return the merged
/// multi-run event channel as a `Stream` without pulling in `tokio-stream`
/// (mirrors the wrapper [`FileTailRunSource`] uses internally).
struct MergedEvents {
    rx: mpsc::Receiver<Event>,
}

impl Stream for MergedEvents {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.get_mut().rx.poll_recv(cx)
    }
}

/// Tail **every** run's `events.jsonl` and merge them into a single live SSE
/// firehose — the global Live Events stream (no `?run` selector).
///
/// A coordinator task lists runs once per second and attaches a
/// [`FileTailRunSource`] to each run we haven't tailed yet:
///   - On the **first** pass it attaches only to currently-active
///     (Running / Pending / AwaitingApproval) runs, so the firehose isn't
///     flooded with the entire history of every terminal run on disk.
///   - On **later** passes it attaches to any run id not seen before — i.e. a
///     run created after the client connected — regardless of status, so a
///     short run launched live is shown start-to-finish.
///
/// This replaces the Phase-1 single-run tail: when many runs execute
/// concurrently, every run's events flow through the one stream, and a run
/// whose `events.jsonl` is missing or empty no longer blocks the whole feed.
pub async fn tail_all_events_sse(
    run_store: Arc<RunStore>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (tx, rx) = mpsc::channel::<Event>(256);

    tokio::spawn(async move {
        let mut seen: HashSet<String> = HashSet::new();
        // One forwarder task per tailed run. A `FileTailRunSource` never ends on
        // its own (it polls forever), so when the client disconnects we abort
        // these explicitly — otherwise each would stay parked, keeping its
        // 250 ms file-poll loop alive indefinitely.
        let mut forwarders: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        let mut first = true;
        loop {
            if tx.is_closed() {
                for h in &forwarders {
                    h.abort();
                }
                break;
            }
            match run_store.list() {
                Ok(runs) => {
                    for r in &runs {
                        let active = matches!(
                            r.status,
                            RunStatus::Running | RunStatus::Pending | RunStatus::AwaitingApproval
                        );
                        // First pass: only currently-active runs. Later passes:
                        // any id we haven't seen (a run started after connect).
                        let take = if first { active } else { !seen.contains(&r.id) };
                        if take && seen.insert(r.id.clone()) {
                            let path = run_store.events_path(&r.id);
                            if let Ok(mut src) = FileTailRunSource::open(&path).await {
                                let tx_run = tx.clone();
                                forwarders.push(tokio::spawn(async move {
                                    while let Some(ev) = src.next().await {
                                        if tx_run.send(ev).await.is_err() {
                                            break;
                                        }
                                    }
                                }));
                            }
                        }
                    }
                    // Mark every currently-known id as seen so subsequent passes
                    // treat only genuinely-new runs as new.
                    if first {
                        for r in &runs {
                            seen.insert(r.id.clone());
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "events firehose: failed to list runs");
                }
            }
            first = false;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let stream = MergedEvents { rx }.map(|ev| {
        let sse = SseEvent::default()
            .json_data(&ev)
            .unwrap_or_else(|_| SseEvent::default().comment("event serialize error"));
        Ok::<_, Infallible>(sse)
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
