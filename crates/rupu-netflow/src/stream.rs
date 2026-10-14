//! A coordinator-launched root run's netflow stream (spec
//! 2026-10-14-rupu-remote-netflow-transport-design.md §N1): every ledger line
//! of every run beneath the root, tagged with its ledger id, after one `begin`
//! line that proves the host streams.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::LedgerLine;

/// The stream's file name inside a run directory.
pub const STREAM_FILE: &str = "netflow.jsonl";
/// The stream format version carried on the `begin` line.
pub const STREAM_VERSION: u32 = 1;

/// Where one run's ledger lines are teed: the root's stream file and the
/// ledger id (this run's own id) each line is tagged with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetflowStream {
    pub path: PathBuf,
    pub ledger_id: String,
}

/// `<runs_root>/<run_id>/netflow.jsonl`.
pub fn stream_path(runs_root: &Path, run_id: &str) -> PathBuf {
    runs_root.join(run_id).join(STREAM_FILE)
}

fn lock_path(stream: &Path) -> PathBuf {
    let mut name = stream.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    stream.with_file_name(name)
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Out<'a> {
    Begin {
        v: u32,
        run_id: &'a str,
    },
    Line {
        ledger: &'a str,
        line: &'a LedgerLine,
    },
}

/// Append `line` (a JSON object without its newline) as ONE write, holding the
/// stream's lock file so writers in several tasks or processes never interleave.
fn locked_append(path: &Path, line: &str) -> std::io::Result<()> {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    locked_append_bytes(path, buf.as_bytes())
}

/// Append `buf` (whole newline-terminated lines) as ONE write under the
/// stream's lock file.
fn locked_append_bytes(path: &Path, buf: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(path))?;
    lock.lock()?;
    let res = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(buf));
    let _ = lock.unlock();
    res
}

/// True when the stream's first line is a `begin` line.
pub fn stream_has_begin(path: &Path) -> bool {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    if std::io::BufReader::new(f).read_line(&mut first).is_err() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(first.trim())
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(|k| k == "begin"))
        .unwrap_or(false)
}

/// Start the stream: write the `begin` line unless the stream already has
/// one (a host-side `workflow resume` reuses its run's stream). Idempotent.
///
/// The check and the write are not one atomic step, but only one process
/// starts a given root, so two `begin` lines cannot race in.
pub fn write_stream_begin(path: &Path, run_id: &str) -> std::io::Result<()> {
    if stream_has_begin(path) {
        return Ok(());
    }
    let line = serde_json::to_string(&Out::Begin {
        v: STREAM_VERSION,
        run_id,
    })?;
    locked_append(path, &line)
}

/// Tee one ledger line into the stream, tagged with `ledger`.
pub fn append_stream_line(path: &Path, ledger: &str, line: &LedgerLine) -> std::io::Result<()> {
    let line = serde_json::to_string(&Out::Line { ledger, line })?;
    locked_append(path, &line)
}

/// Tee already-serialized ledger lines (`lines`: newline-separated JSON, as
/// written to the ledger) into the stream, tagged with `ledger`, in one
/// locked write — the ledger backfill's batches. A blank or non-JSON line is
/// skipped rather than written as a stream line the coordinator would count
/// as malformed.
pub fn append_stream_raw_lines(path: &Path, ledger: &str, lines: &[u8]) -> std::io::Result<()> {
    let tag = serde_json::to_string(ledger)?;
    let mut buf = Vec::with_capacity(lines.len() + 64);
    for raw in lines.split(|b| *b == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if raw.iter().all(u8::is_ascii_whitespace)
            || serde_json::from_slice::<&serde_json::value::RawValue>(raw).is_err()
        {
            continue;
        }
        buf.extend_from_slice(b"{\"kind\":\"line\",\"ledger\":");
        buf.extend_from_slice(tag.as_bytes());
        buf.extend_from_slice(b",\"line\":");
        buf.extend_from_slice(raw);
        buf.extend_from_slice(b"}\n");
    }
    if buf.is_empty() {
        return Ok(());
    }
    locked_append_bytes(path, &buf)
}
