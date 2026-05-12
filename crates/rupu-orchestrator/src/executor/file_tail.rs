//! FileTailRunSource — notify-driven consumer of events.jsonl for
//! runs the executor didn't start (CLI / cron / MCP). Yields parsed
//! Event values as a Stream.
//!
//! On macOS kqueue the `notify` backend may not reliably deliver
//! `Modify` events for appends on some kernel/volume configurations.
//! A polling fallback (250 ms interval) runs alongside the watcher
//! and covers those gaps. Both paths share an `Arc<AtomicU64>` byte
//! offset so events are never duplicated.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use futures_util::Stream;
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::executor::Event;

pub struct FileTailRunSource {
    rx: mpsc::Receiver<Event>,
    _watcher: notify::RecommendedWatcher,
}

impl FileTailRunSource {
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Event>(64);
        let path_buf: PathBuf = path.to_path_buf();
        let parent = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        // Shared offset between the initial-drain task, the notify
        // watcher callback, and the polling fallback.
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

        // --- notify watcher ---
        let tx_for_watcher = tx.clone();
        let path_for_watcher = path_buf.clone();
        let offset_for_watcher = offset.clone();
        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                if let Ok(evt) = res {
                    if matches!(
                        evt.kind,
                        notify::EventKind::Modify(_) | notify::EventKind::Create(_)
                    ) {
                        // Only react if the changed path is our file
                        let touches_target = evt.paths.iter().any(|p| p == &path_for_watcher);
                        if !touches_target {
                            return;
                        }
                        drain_new_bytes(
                            &path_for_watcher,
                            &offset_for_watcher,
                            &tx_for_watcher,
                        );
                    }
                }
            },
        )
        .map_err(std::io::Error::other)?;

        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(std::io::Error::other)?;

        // --- polling fallback (250 ms) ---
        // Covers kqueue gaps where notify doesn't fire for appends.
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

        Ok(Self {
            rx,
            _watcher: watcher,
        })
    }
}

/// Synchronous drain called from the notify callback thread.
fn drain_new_bytes(
    path: &Path,
    offset: &Arc<AtomicU64>,
    tx: &mpsc::Sender<Event>,
) {
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
            let _ = tx.blocking_send(ev);
        }
    }
    offset.store(bytes.len() as u64, Ordering::SeqCst);
}

/// Async drain called from the polling fallback task.
async fn drain_new_bytes_async(
    path: &Path,
    offset: &Arc<AtomicU64>,
    tx: &mpsc::Sender<Event>,
) {
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
