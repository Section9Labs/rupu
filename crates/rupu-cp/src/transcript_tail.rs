//! `TranscriptTail` — polling consumer of a transcript JSONL file, yielding
//! parsed [`rupu_transcript::Event`] values as a [`Stream`].
//!
//! This mirrors [`rupu_orchestrator::executor::FileTailRunSource`] but parses
//! the transcript Event type (`rupu_transcript::Event`) rather than the
//! orchestrator's step-level event. The transcript file and the orchestrator's
//! `events.jsonl` are different JSONL schemas, so they need separate tailers.
//!
//! Tailing is poll-based and single-owner: one spawned task owns a
//! [`rupu_transcript::JsonlCursor`] and, every 250 ms, reads only the bytes
//! appended since the previous tick (the first tick drains the whole backlog
//! from offset 0, so there is no second drainer to race). The cursor holds
//! back a partially written trailing line until its newline lands, decodes
//! UTF-8 per whole line (a write boundary inside a multi-byte character can't
//! drop a chunk), and restarts from 0 if the file shrinks or is replaced. A
//! file that does not exist yet simply yields nothing until it appears; the
//! task exits once the consumer drops the stream. (We deliberately do NOT use
//! a `notify` filesystem watcher — the macOS kqueue backend it's pinned to
//! panics a background thread on teardown when the stream is dropped, and the
//! 250 ms poll already covers append-tailing reliably.)

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use rupu_transcript::{Event, JsonlCursor};
use tokio::sync::mpsc;

/// Live tail of a transcript JSONL file. Each newly-appended, parseable line
/// becomes one [`Event`] on the stream. Lines that fail to parse are skipped
/// (they don't terminate the stream).
pub struct TranscriptTail {
    rx: mpsc::Receiver<Event>,
}

impl TranscriptTail {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(256);
        let path_buf: PathBuf = path.to_path_buf();

        tokio::spawn(async move {
            let mut cursor = JsonlCursor::new();
            loop {
                // Nobody is listening any more (including the case where the
                // file never appeared): stop polling instead of spinning for
                // the rest of the process.
                if tx.is_closed() {
                    return;
                }
                // Blocking read of only the appended bytes; bounded work per
                // tick. A drain I/O error is treated like "no data this
                // tick" — the next tick retries from the same offset.
                let p = path_buf.clone();
                let mut c = std::mem::take(&mut cursor);
                match tokio::task::spawn_blocking(move || {
                    let mut lines = Vec::new();
                    let _ = c.drain_with(&p, || {}, |l| lines.push(l.to_string()));
                    (c, lines)
                })
                .await
                {
                    Ok((c, lines)) => {
                        cursor = c;
                        for line in lines {
                            if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                                if tx.send(ev).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Only reachable if the blocking closure panicked; the
                        // cursor was lost with it, so restart from offset 0.
                        tracing::warn!(error = %e, "transcript tail drain failed; restarting cursor");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });

        Ok(Self { rx })
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

    /// Writes `n` events to a file BEFORE `TranscriptTail::open` so the first
    /// drain sees a multi-line backlog. Asserts EXACTLY `n` events arrive (no
    /// duplicates) within a generous timeout.
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
            schema: None,
            system_prompt: None,
            codename: None,
            customer: None,
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
    fn usage_line(model: &str) -> String {
        serde_json::to_string(&Event::Usage {
            provider: "anthropic".into(),
            model: model.into(),
            served_model: None,
            input_tokens: 1,
            output_tokens: 1,
            cached_tokens: 0,
            cache_write_tokens: 0,
            purpose: None,
        })
        .unwrap()
            + "\n"
    }

    #[tokio::test]
    async fn partial_line_is_delivered_once_completed_never_dropped() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.jsonl");
        let full = usage_line("m-partial");
        let (head, tail_part) = full.split_at(full.len() / 2);
        std::fs::write(&p, head).unwrap();
        let mut tail = TranscriptTail::open(&p).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(tail_part.as_bytes()).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(3), tail.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(got, Event::Usage { ref model, .. } if model == "m-partial"));
    }

    #[tokio::test]
    async fn large_backlog_is_emitted_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.jsonl");
        let body: String = (0..5000).map(|i| usage_line(&format!("m{i}"))).collect();
        std::fs::write(&p, body).unwrap();
        let mut tail = TranscriptTail::open(&p).await.unwrap();
        let mut n = 0;
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(800), tail.next()).await
        {
            n += 1;
        }
        assert_eq!(n, 5000);
    }

    #[tokio::test]
    async fn utf8_split_does_not_drop_chunk() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.jsonl");
        let line = serde_json::to_string(&Event::AssistantMessage {
            content: "build \u{2713} passed".into(),
            thinking: None,
        })
        .unwrap()
            + "\n";
        // serde_json emits the check mark as raw UTF-8 (E2 9C 93); cut inside it.
        let bytes = line.as_bytes();
        let mark = bytes
            .windows(3)
            .position(|w| w == [0xE2, 0x9C, 0x93])
            .unwrap();
        let (head, rest) = bytes.split_at(mark + 1);
        std::fs::write(&p, head).unwrap();
        let mut tail = TranscriptTail::open(&p).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(rest).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(3), tail.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(got, Event::AssistantMessage { ref content, .. } if content.contains('\u{2713}'))
        );
    }
}
