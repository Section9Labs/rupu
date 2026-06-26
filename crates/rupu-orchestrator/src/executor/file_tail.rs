//! FileTailRunSource — polling consumer of events.jsonl for runs the
//! executor didn't start (CLI / cron / MCP). Yields parsed Event values
//! as a Stream.
//!
//! Tailing is poll-based: an initial-drain task reads any pre-existing
//! backlog, then a 250 ms poll loop emits newly-appended lines. Both
//! share an `Arc<AtomicU64>` byte offset so events are never duplicated.
//! (We deliberately do NOT use a `notify` filesystem watcher here — the
//! macOS kqueue backend it's pinned to panics a background thread on
//! teardown when the stream is dropped, and the 250 ms poll already
//! covers append-tailing reliably without that fragility.)

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::Stream;
use tokio::sync::mpsc;

use crate::executor::Event;

pub struct FileTailRunSource {
    rx: mpsc::Receiver<Event>,
}

impl FileTailRunSource {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(64);
        let path_buf: PathBuf = path.to_path_buf();

        // Shared offset between the initial-drain task and the polling
        // loop, so the two never emit the same byte range twice.
        let offset = Arc::new(AtomicU64::new(0));

        // --- initial-drain task ---
        let tx_for_drain = tx.clone();
        let path_for_drain = path_buf.clone();
        let offset_for_drain = offset.clone();
        tokio::spawn(async move {
            while !path_for_drain.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if let Ok(bytes) = std::fs::read(&path_for_drain) {
                for line in std::str::from_utf8(&bytes).unwrap_or("").lines() {
                    if let Ok(ev) = serde_json::from_str::<Event>(line) {
                        if tx_for_drain.send(ev).await.is_err() {
                            return;
                        }
                    }
                }
                offset_for_drain.store(bytes.len() as u64, Ordering::SeqCst);
            }
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
                drain_new_bytes_async(&path_for_poll, &offset_for_poll, &tx_for_poll).await;
            }
        });

        Ok(Self { rx })
    }
}

/// Async drain called from the polling loop.
async fn drain_new_bytes_async(path: &Path, offset: &Arc<AtomicU64>, tx: &mpsc::Sender<Event>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let off = offset.load(Ordering::SeqCst);
    if (bytes.len() as u64) <= off {
        return;
    }
    let new = &bytes[off as usize..];
    for line in std::str::from_utf8(new).unwrap_or("").lines() {
        if let Ok(ev) = serde_json::from_str::<Event>(line) {
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    }
    offset.store(bytes.len() as u64, Ordering::SeqCst);
}

impl Stream for FileTailRunSource {
    type Item = Event;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}
