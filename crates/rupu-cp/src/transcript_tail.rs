//! `TranscriptTail` — polling consumer of a transcript JSONL file, yielding
//! parsed [`rupu_transcript::Event`] values as a [`Stream`].
//!
//! This mirrors [`rupu_orchestrator::executor::FileTailRunSource`] but parses
//! the transcript Event type (`rupu_transcript::Event`) rather than the
//! orchestrator's step-level event. The transcript file and the orchestrator's
//! `events.jsonl` are different JSONL schemas, so they need separate tailers.
//!
//! Tailing is poll-based: an initial-drain task reads any pre-existing backlog,
//! then a 250 ms poll loop emits newly-appended lines. Both share an
//! `Arc<AtomicU64>` byte offset so events are never duplicated. (We deliberately
//! do NOT use a `notify` filesystem watcher — the macOS kqueue backend it's
//! pinned to panics a background thread on teardown when the stream is dropped,
//! and the 250 ms poll already covers append-tailing reliably.)
//!
//! ## Double-emit prevention
//!
//! Two drain paths (initial-drain task, 250 ms poll) each do: load offset → read
//! file → emit lines in `bytes[off..]` → store new offset. If they raced on the
//! same `off` they would both emit the same range. The fix: each drainer
//! atomically *claims* the byte range via `compare_exchange` **before** emitting.
//! Only the drainer that wins the CAS emits; all others return immediately.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::Stream;
use rupu_transcript::Event;
use tokio::sync::mpsc;

/// Live tail of a transcript JSONL file. Each newly-appended, parseable line
/// becomes one [`Event`] on the stream. Lines that fail to parse are skipped
/// (they don't terminate the stream).
pub struct TranscriptTail {
    rx: mpsc::Receiver<Event>,
}

impl TranscriptTail {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(64);
        let path_buf: PathBuf = path.to_path_buf();

        // Shared offset between the initial-drain task and the polling loop,
        // so the two never emit the same byte range twice.
        let offset = Arc::new(AtomicU64::new(0));

        // --- initial-drain task ---
        let tx_for_drain = tx.clone();
        let path_for_drain = path_buf.clone();
        let offset_for_drain = offset.clone();
        tokio::spawn(async move {
            while !path_for_drain.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            drain_and_emit_async(&path_for_drain, &offset_for_drain, &tx_for_drain).await;
        });

        // --- polling loop (250 ms) ---
        // Emits newly-appended lines. This is the sole tailing mechanism
        // (no notify watcher — see the module docs).
        let tx_for_poll = tx.clone();
        let path_for_poll = path_buf.clone();
        let offset_for_poll = offset.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if tx_for_poll.is_closed() {
                    return;
                }
                drain_and_emit_async(&path_for_poll, &offset_for_poll, &tx_for_poll).await;
            }
        });

        Ok(Self {
            rx,
        })
    }
}

/// Async drain called from the initial-drain task and the 250 ms polling loop.
///
/// Uses `tokio::fs::read` to avoid blocking the runtime. Atomically claims the
/// new byte range via `compare_exchange` before emitting, so if the other
/// drainer has already advanced the offset past `old_off` this call is a no-op
/// — preventing double-emission when both drain paths fire concurrently.
async fn drain_and_emit_async(path: &Path, offset: &Arc<AtomicU64>, tx: &mpsc::Sender<Event>) {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return;
    };
    let new_len = bytes.len() as u64;
    let old_off = offset.load(Ordering::SeqCst);
    if new_len <= old_off {
        return;
    }
    // Claim [old_off, new_len): only the winner of this CAS emits.
    if offset
        .compare_exchange(old_off, new_len, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // another drainer already claimed this range
    }
    for line in std::str::from_utf8(&bytes[old_off as usize..])
        .unwrap_or("")
        .lines()
    {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(ev) = serde_json::from_str::<Event>(line) {
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    }
}

impl Stream for TranscriptTail {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use futures_util::StreamExt as _;
    use rupu_transcript::RunMode;
    use std::time::Duration;

    /// Build N transcript `AssistantMessage` events as JSONL bytes.
    fn make_jsonl_events(n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..n {
            let ev = Event::AssistantMessage {
                content: format!("msg {i}"),
                thinking: None,
            };
            let mut line = serde_json::to_vec(&ev).unwrap();
            line.push(b'\n');
            out.extend_from_slice(&line);
        }
        out
    }

    /// Writes `n` events to a file BEFORE `TranscriptTail::open` so that the
    /// initial-drain task has a multi-line backlog and the first 250 ms poll
    /// tick may fire concurrently with it. Asserts EXACTLY `n` events arrive
    /// (no duplicates) within a generous timeout.
    #[tokio::test]
    async fn no_duplicate_events_on_backlog_open() {
        const N: usize = 3;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");

        // Write the full backlog BEFORE opening the tail.
        std::fs::write(&path, make_jsonl_events(N)).unwrap();

        let mut tail = TranscriptTail::open(&path).await.unwrap();

        // Collect events until we have N or until 2 s elapse.
        let collected: Vec<Event> = tokio::time::timeout(Duration::from_secs(2), async {
            let mut v = Vec::new();
            while v.len() < N {
                match tail.next().await {
                    Some(ev) => v.push(ev),
                    None => break,
                }
            }
            v
        })
        .await
        .unwrap_or_default();

        // Exactly N events — no duplicates.
        assert_eq!(
            collected.len(),
            N,
            "expected {N} events, got {}: {collected:?}",
            collected.len()
        );
        // Each message is distinct (content matches the emitted index).
        for (i, ev) in collected.iter().enumerate() {
            match ev {
                Event::AssistantMessage { content, .. } => {
                    assert_eq!(content, &format!("msg {i}"));
                }
                other => panic!("unexpected event at index {i}: {other:?}"),
            }
        }
    }

    /// Smoke test: open the tail with no pre-existing file, append one event
    /// after opening, and confirm it arrives on the stream.
    #[tokio::test]
    async fn stream_receives_appended_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.jsonl");

        // File does not exist yet.
        let mut tail = TranscriptTail::open(&path).await.unwrap();

        // Write one event after the tail is open.
        let ev = Event::RunStart {
            run_id: "r1".into(),
            workspace_id: "ws1".into(),
            agent: "agent".into(),
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            started_at: chrono::Utc
                .with_ymd_and_hms(2026, 6, 16, 12, 0, 0)
                .unwrap(),
            mode: RunMode::Ask,
        };
        let mut line = serde_json::to_vec(&ev).unwrap();
        line.push(b'\n');
        std::fs::write(&path, &line).unwrap();

        let got = tokio::time::timeout(Duration::from_secs(5), tail.next())
            .await
            .expect("timed out waiting for event")
            .expect("stream closed");

        assert_eq!(got, ev);
    }
}
