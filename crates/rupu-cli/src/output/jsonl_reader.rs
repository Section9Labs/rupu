//! Incremental JSONL reader for line-stream output.
//!
//! `TranscriptTailer` maintains a byte-offset into a transcript file and
//! returns newly-added events on each `drain` call. Used by the workflow
//! and watch commands to tail live transcript files.

use rupu_transcript::Event;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Incremental byte-offset reader for a single JSONL transcript file.
pub struct TranscriptTailer {
    path: PathBuf,
    offset: u64,
}

/// Reverse pager for loading older complete transcript events on demand.
#[derive(Debug)]
pub struct TranscriptHistoryPager {
    path: PathBuf,
    cursor: u64,
    end_offset: u64,
}

impl TranscriptTailer {
    /// Create a new tailing reader starting at byte 0.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_offset(path, 0)
    }

    /// Create a new tailing reader starting at a specific byte offset.
    pub fn with_offset(path: impl Into<PathBuf>, offset: u64) -> Self {
        Self {
            path: path.into(),
            offset,
        }
    }

    /// Drain any new complete lines since the last call. Returns parsed
    /// events in order. Incomplete trailing lines (write in progress) are
    /// silently deferred to the next call.
    pub fn drain(&mut self) -> Vec<Event> {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(metadata) = file.metadata() else {
            return Vec::new();
        };
        if metadata.len() < self.offset {
            self.offset = 0;
        }
        if metadata.len() <= self.offset {
            return Vec::new();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut new_bytes = Vec::new();
        if file.read_to_end(&mut new_bytes).is_err() {
            return Vec::new();
        }
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

impl TranscriptHistoryPager {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let end_offset = stable_end_offset(&path).unwrap_or(0);
        Self {
            path,
            cursor: end_offset,
            end_offset,
        }
    }

    pub fn end_offset(&self) -> u64 {
        self.end_offset
    }

    pub fn exhausted(&self) -> bool {
        self.cursor == 0
    }

    pub fn load_previous(&mut self, max_events: usize) -> Vec<Event> {
        const BLOCK_BYTES: u64 = 64 * 1024;

        if max_events == 0 || self.cursor == 0 {
            return Vec::new();
        }

        let Ok(mut file) = std::fs::File::open(&self.path) else {
            self.cursor = 0;
            self.end_offset = 0;
            return Vec::new();
        };
        let Ok(metadata) = file.metadata() else {
            self.cursor = 0;
            self.end_offset = 0;
            return Vec::new();
        };
        self.end_offset = stable_end_offset(&self.path).unwrap_or(metadata.len());
        self.cursor = self.cursor.min(self.end_offset);
        if self.cursor == 0 {
            return Vec::new();
        }

        let segment_end = self.cursor;
        let mut buffer = Vec::new();
        let mut buffer_start = segment_end;
        let mut complete_lines = 0usize;

        while buffer_start > 0 && complete_lines < max_events {
            let start = buffer_start.saturating_sub(BLOCK_BYTES);
            let read_len = (buffer_start - start) as usize;
            if file.seek(SeekFrom::Start(start)).is_err() {
                break;
            }
            let mut chunk = vec![0u8; read_len];
            if file.read_exact(&mut chunk).is_err() {
                break;
            }
            chunk.extend_from_slice(&buffer);
            buffer = chunk;
            buffer_start = start;
            complete_lines = count_complete_lines(&buffer, buffer_start == 0);
        }

        let usable_start = if buffer_start == 0 {
            0
        } else {
            buffer
                .iter()
                .position(|&byte| byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(buffer.len())
        };
        let usable = &buffer[usable_start..];
        let lines = collect_complete_lines(usable, buffer_start == 0);
        if lines.is_empty() {
            self.cursor = buffer_start;
            return Vec::new();
        }

        let start_idx = lines.len().saturating_sub(max_events);
        let chosen = &lines[start_idx..];
        self.cursor = buffer_start + usable_start as u64 + chosen[0].offset as u64;

        chosen
            .iter()
            .filter_map(|line| serde_json::from_slice::<Event>(line.bytes).ok())
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
struct CompleteLine<'a> {
    offset: usize,
    bytes: &'a [u8],
}

fn count_complete_lines(bytes: &[u8], starts_at_line_boundary: bool) -> usize {
    collect_complete_lines(bytes, starts_at_line_boundary).len()
}

fn collect_complete_lines(bytes: &[u8], starts_at_line_boundary: bool) -> Vec<CompleteLine<'_>> {
    let mut lines = Vec::new();
    let mut cursor = 0usize;
    for chunk in bytes.split_inclusive(|&byte| byte == b'\n') {
        if !chunk.ends_with(b"\n") {
            break;
        }
        let line = &chunk[..chunk.len() - 1];
        if !(line.is_empty() && cursor == 0 && !starts_at_line_boundary) {
            if !line.is_empty() {
                lines.push(CompleteLine {
                    offset: cursor,
                    bytes: line,
                });
            }
        }
        cursor += chunk.len();
    }
    lines
}

fn stable_end_offset(path: &Path) -> std::io::Result<u64> {
    const BLOCK_BYTES: u64 = 64 * 1024;

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }

    let mut scan_end = len;
    let mut suffix = Vec::new();
    while scan_end > 0 {
        let start = scan_end.saturating_sub(BLOCK_BYTES);
        let read_len = (scan_end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        let mut chunk = vec![0u8; read_len];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&suffix);
        if let Some(index) = chunk.iter().rposition(|&byte| byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        suffix = chunk;
        scan_end = start;
    }

    Ok(0)
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

    #[test]
    fn history_pager_loads_previous_events_in_reverse_batches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");

        let events = [
            Event::TurnStart { turn_idx: 0 },
            Event::AssistantMessage {
                content: "one".into(),
                thinking: None,
            },
            Event::AssistantMessage {
                content: "two".into(),
                thinking: None,
            },
            Event::AssistantMessage {
                content: "three".into(),
                thinking: None,
            },
            Event::TurnEnd {
                turn_idx: 0,
                tokens_in: None,
                tokens_out: None,
            },
        ];

        let mut file = std::fs::File::create(&path).unwrap();
        for event in &events {
            writeln!(file, "{}", serde_json::to_string(event).unwrap()).unwrap();
        }
        drop(file);

        let mut pager = TranscriptHistoryPager::new(&path);
        let newest = pager.load_previous(2);
        assert_eq!(newest.len(), 2);
        assert!(matches!(newest[0], Event::AssistantMessage { .. }));
        assert!(matches!(newest[1], Event::TurnEnd { .. }));

        let older = pager.load_previous(2);
        assert_eq!(older.len(), 2);
        assert!(matches!(older[0], Event::AssistantMessage { .. }));
        assert!(matches!(older[1], Event::AssistantMessage { .. }));

        let oldest = pager.load_previous(2);
        assert_eq!(oldest.len(), 1);
        assert!(matches!(oldest[0], Event::TurnStart { .. }));
        assert!(pager.exhausted());
    }

    #[test]
    fn stable_end_offset_ignores_incomplete_trailing_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.jsonl");

        let complete = serde_json::to_string(&Event::TurnStart { turn_idx: 0 }).unwrap();
        let partial = "{\"assistant_message\":";
        std::fs::write(&path, format!("{complete}\n{partial}")).unwrap();

        let stable = stable_end_offset(&path).unwrap();
        assert_eq!(stable, (complete.len() + 1) as u64);

        let mut pager = TranscriptHistoryPager::new(&path);
        let events = pager.load_previous(4);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::TurnStart { .. }));

        let mut tailer = TranscriptTailer::with_offset(path.clone(), stable);
        assert!(tailer.drain().is_empty());
    }
}
