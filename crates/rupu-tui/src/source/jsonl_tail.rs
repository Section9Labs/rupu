use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use notify::{recommended_watcher, EventKind, RecursiveMode, Watcher};
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
        let mut watcher = recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        watcher.watch(&run_dir, RecursiveMode::Recursive)?;
        Ok(Self {
            run_dir,
            offsets: BTreeMap::new(),
            rx,
            _watcher: Box::new(watcher),
        })
    }

    /// Drain all transcript files (`transcripts/*.jsonl`) from their
    /// last-known byte offset. Returns parsed StepEvents.
    fn drain_transcripts(&mut self) -> Vec<SourceEvent> {
        let transcripts = self.run_dir.join("transcripts");
        let Ok(rd) = std::fs::read_dir(&transcripts) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            self.tail_file(&path, &mut out);
        }
        out
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

#[doc(hidden)]
pub fn _coerce_send_sync<T: Send + Sync>(_: &T) {}
