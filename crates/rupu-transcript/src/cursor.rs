//! Incremental, offset-based JSONL line reader shared by every live consumer
//! (CP usage index, CP transcript/run-event tailers, CLI live view).
//!
//! Contract (spec 2026-09-29 §4.4):
//! - reads only bytes appended since the last call (seek, never re-read);
//! - consumes up to the last `\n`; a partial trailing line is held back until
//!   its newline lands (never skipped, never parsed half-written);
//! - decodes UTF-8 per whole line (a read boundary inside a multi-byte char
//!   can't drop data);
//! - a file that shrank, or was replaced by a different inode, resets to
//!   offset 0 and calls `on_reset` before any line of the new content.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdent {
    dev: u64,
    ino: u64,
}

fn ident(meta: &std::fs::Metadata) -> Option<FileIdent> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdent {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Incremental reader state for one file. Cheap to clone; holds no fd.
#[derive(Debug, Clone, Default)]
pub struct JsonlCursor {
    offset: u64,
    ident: Option<FileIdent>,
}

/// What one [`JsonlCursor::drain_with`] call did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainStats {
    /// The file shrank or was replaced; the cursor restarted at 0.
    pub reset: bool,
    /// Non-blank lines delivered to `on_line`.
    pub lines: usize,
    /// Bytes consumed (the offset advance, including newlines).
    pub bytes: u64,
}

impl JsonlCursor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Byte offset of the first unconsumed byte.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Deliver every complete, non-blank line appended since the last call.
    /// A missing file is "no data yet" (`Ok`, nothing delivered, no reset).
    pub fn drain_with(
        &mut self,
        path: &Path,
        on_reset: impl FnOnce(),
        mut on_line: impl FnMut(&str),
    ) -> std::io::Result<DrainStats> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DrainStats::default()),
            Err(e) => return Err(e),
        };
        let mut stats = DrainStats::default();
        let id = ident(&meta);
        let replaced = matches!((self.ident, id), (Some(a), Some(b)) if a != b);
        if meta.len() < self.offset || replaced {
            self.offset = 0;
            stats.reset = true;
            on_reset();
        }
        self.ident = id;
        if meta.len() == self.offset {
            return Ok(stats);
        }
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut carry: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            carry.extend_from_slice(&buf[..n]);
            let Some(last_nl) = carry.iter().rposition(|b| *b == b'\n') else {
                continue;
            };
            for raw in carry[..last_nl].split(|b| *b == b'\n') {
                let Ok(line) = std::str::from_utf8(raw) else {
                    continue;
                };
                let line = line.trim_end_matches('\r');
                if line.trim().is_empty() {
                    continue;
                }
                stats.lines += 1;
                on_line(line);
            }
            let consumed = (last_nl + 1) as u64;
            self.offset += consumed;
            stats.bytes += consumed;
            carry.drain(..=last_nl);
        }
        Ok(stats)
    }
}

/// Identity of a transcript file: its stem (`run_<ULID>` for the flat layout),
/// or the parent directory name for the nested sub-run layout
/// `…/<sub_id>/transcript.jsonl`. Ledger rows and known-transcript sets are
/// both keyed by this, so they can never disagree on identity.
pub fn transcript_key(path: &Path) -> Option<String> {
    match path.file_stem().and_then(|s| s.to_str()) {
        Some("transcript") => path
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|s| s.to_str())
            .map(str::to_string),
        Some(stem) if !stem.is_empty() => Some(stem.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn collect(c: &mut JsonlCursor, p: &Path) -> (Vec<String>, DrainStats) {
        let mut out = Vec::new();
        let st = c.drain_with(p, || {}, |l| out.push(l.to_string())).unwrap();
        (out, st)
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = JsonlCursor::new();
        let (lines, st) = collect(&mut c, &dir.path().join("nope.jsonl"));
        assert!(lines.is_empty());
        assert!(!st.reset);
        assert_eq!(c.offset(), 0);
    }

    #[test]
    fn reads_only_new_complete_lines_and_holds_back_partial() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        write!(f, "{{\"a\":1}}\n{{\"b\":").unwrap();
        f.flush().unwrap();
        let mut c = JsonlCursor::new();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec!["{\"a\":1}"]);
        assert_eq!(c.offset(), 8);
        writeln!(f, "2}}").unwrap();
        f.flush().unwrap();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec!["{\"b\":2}"]);
        let (lines, _) = collect(&mut c, &p);
        assert!(lines.is_empty(), "no new bytes → no lines");
    }

    #[test]
    fn utf8_split_across_reads_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("u.jsonl");
        let line = "{\"t\":\"héllo✓\"}\n";
        let bytes = line.as_bytes();
        // Write up to the middle of the multi-byte '✓' (3 bytes).
        let cut = line.find('✓').unwrap() + 1;
        std::fs::write(&p, &bytes[..cut]).unwrap();
        let mut c = JsonlCursor::new();
        let (lines, _) = collect(&mut c, &p);
        assert!(lines.is_empty());
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&bytes[cut..]).unwrap();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec![line.trim_end()]);
    }

    #[test]
    fn shrink_resets_and_rereads_from_start() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, "{\"a\":1}\n{\"a\":2}\n").unwrap();
        let mut c = JsonlCursor::new();
        let _ = collect(&mut c, &p);
        std::fs::write(&p, "{\"z\":9}\n").unwrap();
        let mut resets = 0;
        let mut lines = Vec::new();
        let st = c
            .drain_with(&p, || resets += 1, |l| lines.push(l.to_string()))
            .unwrap();
        assert!(st.reset);
        assert_eq!(resets, 1);
        assert_eq!(lines, vec!["{\"z\":9}"]);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_file_same_length_resets() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        std::fs::write(&p, "{\"a\":1}\n").unwrap();
        let mut c = JsonlCursor::new();
        let _ = collect(&mut c, &p);
        let tmp = dir.path().join("r.tmp");
        std::fs::write(&tmp, "{\"b\":2}\n{\"c\":3}\n").unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        let (lines, st) = collect(&mut c, &p);
        assert!(st.reset);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn blank_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.jsonl");
        std::fs::write(&p, "\n{\"a\":1}\n   \n").unwrap();
        let mut c = JsonlCursor::new();
        let (lines, _) = collect(&mut c, &p);
        assert_eq!(lines, vec!["{\"a\":1}"]);
    }

    #[test]
    fn large_file_streams_in_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.jsonl");
        let mut body = String::new();
        for i in 0..50_000 {
            body.push_str(&format!("{{\"i\":{i},\"pad\":\"{}\"}}\n", "x".repeat(200)));
        }
        std::fs::write(&p, &body).unwrap();
        let mut c = JsonlCursor::new();
        let (lines, st) = collect(&mut c, &p);
        assert_eq!(lines.len(), 50_000);
        assert_eq!(st.bytes, body.len() as u64);
    }

    #[test]
    fn transcript_key_flat_and_nested() {
        assert_eq!(
            transcript_key(Path::new("/g/transcripts/run_01ABC.jsonl")).as_deref(),
            Some("run_01ABC")
        );
        assert_eq!(
            transcript_key(Path::new("/g/runs/run_P/sub/sub_01X/transcript.jsonl")).as_deref(),
            Some("sub_01X")
        );
        assert_eq!(transcript_key(Path::new("/")), None);
    }
}
