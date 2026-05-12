//! Per-step transcript file watcher. Same notify-driven pattern as
//! FileTailRunSource, but the parser yields `TranscriptLine` instead
//! of `Event`. Watches the JSONL file at the path supplied to `open`.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::Stream;
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// One record from a per-step transcript JSONL file.
#[derive(Debug, Clone)]
pub struct TranscriptLine {
    /// e.g. `"tool_call"`, `"tool_result"`, `"agent_text"`, `"user_text"`.
    pub kind: String,
    /// Full parsed JSON of the record.
    pub payload: serde_json::Value,
}

/// Stream of [`TranscriptLine`]s from a per-step transcript JSONL file.
pub struct TranscriptTail {
    rx: mpsc::Receiver<TranscriptLine>,
    _watcher: notify::RecommendedWatcher,
}

impl TranscriptTail {
    /// Open a tail on `path`. The file need not exist yet — the initial
    /// drain task polls until the file appears.
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<TranscriptLine>(128);
        let path_buf: PathBuf = path.to_path_buf();
        let parent: PathBuf = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

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
                    if let Some(tl) = parse_line(line) {
                        if tx_for_drain.send(tl).await.is_err() {
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
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(evt) = res {
                if matches!(
                    evt.kind,
                    notify::EventKind::Modify(_) | notify::EventKind::Create(_)
                ) {
                    let touches_target = evt.paths.iter().any(|p| p == &path_for_watcher);
                    if !touches_target {
                        return;
                    }
                    drain_new(&path_for_watcher, &offset_for_watcher, &tx_for_watcher);
                }
            }
        })
        .map_err(std::io::Error::other)?;

        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(std::io::Error::other)?;

        // --- polling fallback (250 ms, covers kqueue gaps on macOS) ---
        let tx_for_poll = tx;
        let path_for_poll = path_buf;
        let offset_for_poll = offset;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if tx_for_poll.is_closed() {
                    return;
                }
                drain_new_async(&path_for_poll, &offset_for_poll, &tx_for_poll).await;
            }
        });

        Ok(Self {
            rx,
            _watcher: watcher,
        })
    }
}

fn parse_line(line: &str) -> Option<TranscriptLine> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("unknown")
        .to_string();
    Some(TranscriptLine { kind, payload: v })
}

fn drain_new(path: &Path, offset: &AtomicU64, tx: &mpsc::Sender<TranscriptLine>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let off = offset.load(Ordering::SeqCst);
    if (bytes.len() as u64) <= off {
        return;
    }
    let new = &bytes[off as usize..];
    for line in std::str::from_utf8(new).unwrap_or("").lines() {
        if let Some(tl) = parse_line(line) {
            let _ = tx.blocking_send(tl);
        }
    }
    offset.store(bytes.len() as u64, Ordering::SeqCst);
}

async fn drain_new_async(path: &Path, offset: &AtomicU64, tx: &mpsc::Sender<TranscriptLine>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let off = offset.load(Ordering::SeqCst);
    if (bytes.len() as u64) <= off {
        return;
    }
    let new = &bytes[off as usize..];
    for line in std::str::from_utf8(new).unwrap_or("").lines() {
        if let Some(tl) = parse_line(line) {
            if tx.send(tl).await.is_err() {
                return;
            }
        }
    }
    offset.store(bytes.len() as u64, Ordering::SeqCst);
}

impl Stream for TranscriptTail {
    type Item = TranscriptLine;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<TranscriptLine>> {
        let this = self.get_mut();
        this.rx.poll_recv(cx)
    }
}
