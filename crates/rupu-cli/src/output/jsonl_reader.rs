//! Incremental JSONL reader for line-stream output.
//!
//! `TranscriptTailer` maintains a byte-offset into a transcript file and
//! returns newly-added events on each `drain` call. Used by the workflow
//! and watch commands to tail live transcript files without the TUI.

use rupu_transcript::Event;
use std::path::{Path, PathBuf};

/// Incremental byte-offset reader for a single JSONL transcript file.
pub struct TranscriptTailer {
    path: PathBuf,
    offset: u64,
}

impl TranscriptTailer {
    /// Create a new tailing reader starting at byte 0.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
        }
    }

    /// Drain any new complete lines since the last call. Returns parsed
    /// events in order. Incomplete trailing lines (write in progress) are
    /// silently deferred to the next call.
    pub fn drain(&mut self) -> Vec<Event> {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return Vec::new();
        };
        let from = self.offset as usize;
        if bytes.len() <= from {
            return Vec::new();
        }
        let new_bytes = &bytes[from..];
        let mut consumed = 0usize;
        let mut events = Vec::new();
        for line in new_bytes.split_inclusive(|&b| b == b'\n') {
            if !line.ends_with(b"\n") {
                // Incomplete line — writer is mid-flush; defer.
                break;
            }
            consumed += line.len();
            let trimmed = &line[..line.len() - 1]; // strip trailing \n
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_slice::<Event>(trimmed) {
                events.push(ev);
            }
        }
        self.offset += consumed as u64;
        events
    }

    /// Path of the file being tailed.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_transcript::{Event, RunMode};
    use std::io::Write;

    #[test]
    fn drain_parses_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");

        // Write a RunStart event.
        let ev = Event::RunStart {
            run_id: "run_TEST".into(),
            workspace_id: "ws_1".into(),
            agent: "test-agent".into(),
            provider: "anthropic".into(),
            model: "claude-test".into(),
            started_at: chrono::Utc::now(),
            mode: RunMode::Bypass,
        };
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&ev).unwrap()).unwrap();
        drop(f);

        let mut tailer = TranscriptTailer::new(&path);
        let events = tailer.drain();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::RunStart { .. }));
    }

    #[test]
    fn drain_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("incr.jsonl");

        let ev1 = Event::TurnStart { turn_idx: 0 };
        let ev2 = Event::TurnEnd {
            turn_idx: 0,
            tokens_in: None,
            tokens_out: None,
        };

        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&ev1).unwrap()).unwrap();
        drop(f);

        let mut tailer = TranscriptTailer::new(&path);
        let first = tailer.drain();
        assert_eq!(first.len(), 1);

        // Append second event.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{}", serde_json::to_string(&ev2).unwrap()).unwrap();
        drop(f);

        let second = tailer.drain();
        assert_eq!(second.len(), 1);
        assert!(matches!(second[0], Event::TurnEnd { .. }));
    }

    #[test]
    fn drain_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let mut tailer = TranscriptTailer::new(path);
        assert!(tailer.drain().is_empty());
    }
}
