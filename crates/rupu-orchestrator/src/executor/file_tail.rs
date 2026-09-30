//! FileTailRunSource — polling consumer of events.jsonl for runs the
//! executor didn't start (CLI / cron / MCP). Yields parsed Event values
//! as a Stream.
//!
//! Tailing is poll-based and single-owner: one spawned task owns a
//! [`rupu_transcript::JsonlCursor`] and, every 250 ms, reads only the bytes
//! appended since the previous tick (the first tick drains the whole
//! backlog from offset 0, so there is no second drainer to race). The cursor
//! holds back a partially written trailing line until its newline lands,
//! decodes UTF-8 per whole line, and restarts from 0 if the file shrinks or
//! is replaced. A file that does not exist yet simply yields nothing until it
//! appears; the task exits once the consumer drops the stream.
//! (We deliberately do NOT use a `notify` filesystem watcher here — the
//! macOS kqueue backend it's pinned to panics a background thread on
//! teardown when the stream is dropped, and the 250 ms poll already
//! covers append-tailing reliably without that fragility.)

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Stream;
use rupu_transcript::JsonlCursor;
use tokio::sync::mpsc;

use crate::executor::Event;

pub struct FileTailRunSource {
    rx: mpsc::Receiver<Event>,
}

impl FileTailRunSource {
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
                        tracing::warn!(error = %e, "events.jsonl tail drain failed; restarting cursor");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });

        Ok(Self { rx })
    }
}

impl Stream for FileTailRunSource {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::io::Write;

    fn ev_line(step: &str) -> String {
        serde_json::to_string(&Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind: Default::default(),
            agent: None,
            host: None,
        })
        .unwrap()
            + "\n"
    }

    #[tokio::test]
    async fn partial_line_is_delivered_once_completed_never_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("events.jsonl");
        let full = ev_line("a");
        let (head, tail) = full.split_at(full.len() / 2);
        std::fs::write(&p, head).unwrap();
        let mut src = FileTailRunSource::open(&p).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(tail.as_bytes()).unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(3), src.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(got, Event::StepStarted { ref step_id, .. } if step_id == "a"));
    }

    #[tokio::test]
    async fn large_backlog_is_emitted_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("events.jsonl");
        let body: String = (0..5000).map(|i| ev_line(&format!("s{i}"))).collect();
        std::fs::write(&p, body).unwrap();
        let mut src = FileTailRunSource::open(&p).await.unwrap();
        let mut n = 0;
        while let Ok(Some(_)) =
            tokio::time::timeout(std::time::Duration::from_millis(800), src.next()).await
        {
            n += 1;
        }
        assert_eq!(n, 5000);
    }

    #[tokio::test]
    async fn file_appearing_late_is_tailed_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("events.jsonl");
        let mut src = FileTailRunSource::open(&p).await.unwrap();
        // Nothing yet: the missing file yields no event (and does not end the stream).
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(400), src.next())
                .await
                .is_err()
        );
        std::fs::write(&p, ev_line("late")).unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(3), src.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(got, Event::StepStarted { ref step_id, .. } if step_id == "late"));
    }
}
