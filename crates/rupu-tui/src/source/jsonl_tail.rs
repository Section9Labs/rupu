use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use notify::{recommended_watcher, EventKind, RecursiveMode, Watcher, WatcherKind};
use tracing::warn;

use super::{EventSource, SourceEvent};
use crate::err::TuiResult;

/// Watches a run directory and streams parsed transcript events as
/// they're appended. Holds per-file byte offsets to handle truncated
/// trailing lines (writer mid-flush).
///
/// Each event's `step_id` is derived from the transcript filename
/// stem (e.g. `run_01KQQNWH….jsonl` → `run_01KQQNWH…`). For v0
/// single-agent runs this is the right identifier. Workflow-run
/// step-id resolution (file → workflow step) happens at the App
/// layer, which can read `step_results.jsonl` to translate.
pub struct JsonlTailSource {
    run_dir: PathBuf,
    offsets: BTreeMap<PathBuf, u64>,
    rx: Receiver<notify::Result<notify::Event>>,
    _watcher: Box<dyn Watcher + Send + Sync>,
}

impl JsonlTailSource {
    pub fn new(run_dir: PathBuf) -> TuiResult<Self> {
        let (tx, rx) = channel();
        let watcher: Box<dyn Watcher + Send + Sync> = match recommended_watcher({
            let tx = tx.clone();
            move |res| {
                let _ = tx.send(res);
            }
        }) {
            Ok(mut w) => {
                if w.watch(&run_dir, RecursiveMode::Recursive).is_ok() {
                    Box::new(w)
                } else {
                    tracing::info!("notify watch failed; falling back to mtime polling");
                    Box::new(MtimePoller::new(run_dir.clone(), tx))
                }
            }
            Err(e) => {
                tracing::info!(error = %e, "notify init failed; falling back to mtime polling");
                Box::new(MtimePoller::new(run_dir.clone(), tx))
            }
        };
        Ok(Self {
            run_dir,
            offsets: BTreeMap::new(),
            rx,
            _watcher: watcher,
        })
    }

    /// Drain all transcript files (`transcripts/*.jsonl`) from their
    /// last-known byte offset, and also check `run.json` for changes.
    fn drain_transcripts(&mut self) -> Vec<SourceEvent> {
        let mut out = Vec::new();
        self.drain_run_json(&mut out);
        let transcripts = self.run_dir.join("transcripts");
        if let Ok(rd) = std::fs::read_dir(&transcripts) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                self.tail_file(&path, &mut out);
            }
        }
        out
    }

    fn drain_run_json(&mut self, out: &mut Vec<SourceEvent>) {
        let path = self.run_dir.join("run.json");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        // Compare against last-known mtime via a sentinel offset (file size).
        let last_size = self.offsets.get(&path).copied().unwrap_or(u64::MAX);
        if last_size == bytes.len() as u64 {
            return;
        }
        self.offsets.insert(path.clone(), bytes.len() as u64);
        match serde_json::from_slice::<rupu_orchestrator::RunRecord>(&bytes) {
            Ok(rec) => out.push(SourceEvent::RunUpdate(Box::new(rec))),
            Err(e) => warn!(error = %e, "malformed run.json; skipped"),
        }
    }

    fn tail_file(&mut self, path: &std::path::Path, out: &mut Vec<SourceEvent>) {
        let step_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let last_offset = self.offsets.get(path).copied().unwrap_or(0) as usize;
        if bytes.len() <= last_offset {
            return;
        }
        let new_bytes = &bytes[last_offset..];
        let mut consumed = 0_usize;
        for line in new_bytes.split_inclusive(|&b| b == b'\n') {
            if !line.ends_with(b"\n") {
                break;
            }
            let s = match std::str::from_utf8(&line[..line.len() - 1]) {
                Ok(s) => s,
                Err(e) => {
                    warn!(?e, "non-utf8 transcript line; skipped");
                    consumed += line.len();
                    continue;
                }
            };
            match serde_json::from_str::<rupu_transcript::Event>(s) {
                Ok(event) => out.push(SourceEvent::StepEvent {
                    step_id: step_id.clone(),
                    event,
                }),
                Err(e) => warn!(error = %e, "malformed jsonl line; skipped"),
            }
            consumed += line.len();
        }
        let new_offset = last_offset + consumed;
        self.offsets.insert(path.to_path_buf(), new_offset as u64);
    }
}

impl EventSource for JsonlTailSource {
    fn poll(&mut self) -> Vec<SourceEvent> {
        let mut had_signal = false;
        while let Ok(ev) = self.rx.try_recv() {
            if let Ok(ev) = ev {
                if matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    had_signal = true;
                }
            }
        }
        if had_signal {
            self.drain_transcripts()
        } else {
            Vec::new()
        }
    }

    fn wait(&mut self, dur: Duration) -> Option<SourceEvent> {
        let recv = self.rx.recv_timeout(dur).ok()?;
        if recv.is_err() {
            return None;
        }
        let out = self.drain_transcripts();
        out.into_iter().next()
    }
}

/// Fallback when notify can't watch the FS (NFS, sandboxes). Polls
/// every 250ms; sends a synthetic event when any file under run_dir
/// has a newer mtime than the last poll.
struct MtimePoller {
    _handle: std::thread::JoinHandle<()>,
}

impl MtimePoller {
    fn new(run_dir: PathBuf, tx: std::sync::mpsc::Sender<notify::Result<notify::Event>>) -> Self {
        let handle = std::thread::spawn(move || {
            let mut last: BTreeMap<PathBuf, std::time::SystemTime> = BTreeMap::new();
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let mut signaled = false;
                for entry in walkdir::WalkDir::new(&run_dir)
                    .into_iter()
                    .filter_map(Result::ok)
                {
                    let p = entry.path().to_path_buf();
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if last.get(&p).is_none_or(|t| *t < mtime) {
                                last.insert(p, mtime);
                                signaled = true;
                            }
                        }
                    }
                }
                if signaled && tx.send(Ok(notify::Event::default())).is_err() {
                    // Receiver dropped; stop polling.
                    return;
                }
            }
        });
        Self { _handle: handle }
    }
}

impl Watcher for MtimePoller {
    fn new<F: notify::EventHandler>(
        _event_handler: F,
        _config: notify::Config,
    ) -> notify::Result<Self> {
        // MtimePoller is only ever constructed via MtimePoller::new directly;
        // the trait's new() is required but never called.
        Err(notify::Error::generic(
            "MtimePoller::new via Watcher trait not supported; use MtimePoller::new directly",
        ))
    }

    fn watch(
        &mut self,
        _path: &std::path::Path,
        _recursive_mode: RecursiveMode,
    ) -> notify::Result<()> {
        Ok(())
    }

    fn unwatch(&mut self, _path: &std::path::Path) -> notify::Result<()> {
        Ok(())
    }

    fn configure(&mut self, _option: notify::Config) -> notify::Result<bool> {
        Ok(true)
    }

    fn kind() -> WatcherKind {
        WatcherKind::PollWatcher
    }
}

#[doc(hidden)]
pub fn _coerce_send_sync<T: Send + Sync>(_: &T) {}
