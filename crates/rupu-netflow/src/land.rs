//! Landing a remote run's netflow stream on the coordinator (spec
//! 2026-10-14-rupu-remote-netflow-transport-design.md §N3): split it by
//! ledger id into `<global>/netflow/<ledger>.jsonl`, record the result in
//! `<global>/runs/<run_id>/netflow.landing.json`, and keep the workflow run's
//! roster of placed unit attempts.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The landing record's file name inside `<global>/runs/<run_id>/`.
pub const LANDING_FILE: &str = "netflow.landing.json";
/// The placed-unit roster's file name inside a workflow run's directory.
pub const ROSTER_FILE: &str = "netflow.units.jsonl";
/// Why a host that sent no stream at all is `unavailable`.
pub const NOT_STREAMED_REASON: &str =
    "the host did not stream its netflow (an older rupu, or the stream was lost)";
/// Why a stream without a `begin` line is `unavailable`.
pub const NO_BEGIN_REASON: &str =
    "the netflow stream had no begin line (an older rupu, or the stream failed to start)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandingStatus {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LandingRecord {
    pub v: u32,
    pub host: String,
    pub status: LandingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub ledgers: Vec<String>,
    /// The byte length of the stream this record landed (0 with no
    /// stream). A mirror compares its copy's current length against it to
    /// tell a copy that grew after a complete landing from the same copy
    /// mirrored again. Absent on records written before it: reads as 0.
    #[serde(default)]
    pub bytes: u64,
    pub at: DateTime<Utc>,
}

/// A stream split by ledger id; `lines` keep each ledger line's raw JSON
/// so a line kind this binary does not know survives the trip unchanged.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedStream {
    pub begin_seen: bool,
    pub ledgers: BTreeMap<String, Vec<String>>,
    pub malformed: usize,
    pub unsafe_ids: usize,
}

/// A ledger id that is safe as a file name: 1–128 chars of `[A-Za-z0-9_-]`.
pub fn is_safe_ledger_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One stream line as written, keeping a `line` entry's ledger line as its
/// original text so it lands byte-for-byte.
#[derive(Deserialize)]
struct RawEntry<'a> {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    ledger: Option<String>,
    #[serde(default, borrow)]
    line: Option<&'a serde_json::value::RawValue>,
}

/// Split `bytes` into ledgers. A `begin` line must be the first non-empty
/// line to count; an unparseable line, a `line` entry with no `ledger` or
/// `line`, or a later `begin` is malformed; an unsafe ledger id is counted
/// in `unsafe_ids` and dropped.
pub fn parse_stream(bytes: &[u8]) -> ParsedStream {
    let mut out = ParsedStream::default();
    let text = String::from_utf8_lossy(bytes);
    let mut first = true;
    for raw in text.split('\n') {
        let raw = raw.trim_end_matches('\r');
        if raw.trim().is_empty() {
            continue;
        }
        let was_first = std::mem::replace(&mut first, false);
        let Ok(entry) = serde_json::from_str::<RawEntry<'_>>(raw) else {
            out.malformed += 1;
            continue;
        };
        match entry.kind.as_deref() {
            Some("begin") if was_first => out.begin_seen = true,
            Some("line") => {
                let (Some(ledger), Some(line)) = (entry.ledger.as_deref(), entry.line) else {
                    out.malformed += 1;
                    continue;
                };
                if !is_safe_ledger_id(ledger) {
                    out.unsafe_ids += 1;
                    continue;
                }
                // The line's own bytes, as the host's ledger holds them.
                out.ledgers
                    .entry(ledger.to_string())
                    .or_default()
                    .push(line.get().to_string());
            }
            _ => out.malformed += 1,
        }
    }
    out
}

/// Write every parsed ledger to `<netflow_dir>/<ledger>.jsonl` through a
/// temp file whose name does not end in `.jsonl`, then a rename, replacing
/// any earlier landing of the same ledger.
fn write_ledgers(netflow_dir: &Path, parsed: &ParsedStream) -> std::io::Result<Vec<String>> {
    crate::ensure_netflow_dir(netflow_dir)?;
    let mut written = Vec::new();
    for (n, (ledger, lines)) in parsed.ledgers.iter().enumerate() {
        let dest = netflow_dir.join(format!("{ledger}.jsonl"));
        let tmp = netflow_dir.join(format!(".{ledger}.landing-{}-{n}.tmp", std::process::id()));
        let mut body = String::new();
        for l in lines {
            body.push_str(l);
            body.push('\n');
        }
        let res = std::fs::write(&tmp, body.as_bytes()).and_then(|_| std::fs::rename(&tmp, &dest));
        if let Err(e) = res {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        written.push(ledger.clone());
    }
    Ok(written)
}

/// `<runs_root>/<run_id>/netflow.landing.json`.
pub fn landing_record_path(runs_root: &Path, run_id: &str) -> PathBuf {
    runs_root.join(run_id).join(LANDING_FILE)
}

/// The run's landing record; `None` when absent, unreadable or malformed.
pub fn read_landing_record(runs_root: &Path, run_id: &str) -> Option<LandingRecord> {
    let bytes = std::fs::read(landing_record_path(runs_root, run_id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn sidecar_lock(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    path.with_file_name(name)
}

fn with_file_lock<T>(lock: &Path, f: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    if let Some(dir) = lock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)?;
    file.lock()?;
    let out = f();
    let _ = file.unlock();
    out
}

/// Land one run's stream and record the result, under the record's lock.
///
/// - `stream: None` → `unavailable` with `reason` (default
///   [`NOT_STREAMED_REASON`]); nothing landed.
/// - no `begin` line → `unavailable` ([`NO_BEGIN_REASON`]); nothing landed.
/// - otherwise the ledgers land; the record is `complete` only when
///   `complete` is true AND no line was malformed or unsafe, else `partial`
///   with `reason` plus the skipped counts.
/// - Never downgrade: when the existing record is `complete` and this one
///   would not be, nothing is written and the existing record is returned.
///
/// `runs_root` is `<global>/runs`; ledgers land in `<global>/netflow/`.
pub fn land_and_record(
    runs_root: &Path,
    run_id: &str,
    host: &str,
    stream: Option<&[u8]>,
    complete: bool,
    reason: Option<String>,
) -> std::io::Result<LandingRecord> {
    let global = runs_root.parent().unwrap_or(runs_root);
    let netflow_dir = crate::global_netflow_dir(global);
    let record_path = landing_record_path(runs_root, run_id);
    with_file_lock(&sidecar_lock(&record_path), || {
        let existing = read_landing_record(runs_root, run_id);
        let parsed = stream.map(parse_stream);
        let (status, why) = match &parsed {
            None => (
                LandingStatus::Unavailable,
                Some(reason.unwrap_or_else(|| NOT_STREAMED_REASON.to_string())),
            ),
            Some(p) if !p.begin_seen => (
                LandingStatus::Unavailable,
                Some(NO_BEGIN_REASON.to_string()),
            ),
            Some(p) => {
                let skipped = p.malformed + p.unsafe_ids;
                let mut parts: Vec<String> = reason.into_iter().collect();
                if skipped > 0 {
                    parts.push(format!(
                        "{skipped} malformed netflow stream line(s) skipped"
                    ));
                }
                let status = if complete && skipped == 0 {
                    LandingStatus::Complete
                } else {
                    LandingStatus::Partial
                };
                (status, (!parts.is_empty()).then(|| parts.join("; ")))
            }
        };
        if let Some(prev) = existing {
            if prev.status == LandingStatus::Complete && status != LandingStatus::Complete {
                return Ok(prev);
            }
        }
        let ledgers = match &parsed {
            Some(p) if p.begin_seen => write_ledgers(&netflow_dir, p)?,
            _ => Vec::new(),
        };
        let record = LandingRecord {
            v: 1,
            host: host.to_string(),
            status,
            reason: if status == LandingStatus::Complete {
                None
            } else {
                why
            },
            ledgers,
            bytes: stream.map_or(0, |s| s.len() as u64),
            at: Utc::now(),
        };
        let tmp = record_path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&record)?)?;
        std::fs::rename(&tmp, &record_path)?;
        Ok(record)
    })
}

/// One placed unit attempt, appended when the runner dispatches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterLine {
    pub unit_run_id: String,
    pub host: String,
    pub step_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// `<runs_root>/<workflow_run_id>/netflow.units.jsonl`.
pub fn roster_path(runs_root: &Path, workflow_run_id: &str) -> PathBuf {
    runs_root.join(workflow_run_id).join(ROSTER_FILE)
}

/// Append one roster line under the roster's lock file.
pub fn append_roster_line(
    runs_root: &Path,
    workflow_run_id: &str,
    line: &RosterLine,
) -> std::io::Result<()> {
    let path = roster_path(runs_root, workflow_run_id);
    let mut json = serde_json::to_string(line)?;
    json.push('\n');
    with_file_lock(&sidecar_lock(&path), || {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        f.write_all(json.as_bytes())
    })
}

/// Every roster line of a workflow run (malformed lines skipped); empty
/// when the roster does not exist.
pub fn read_roster(runs_root: &Path, workflow_run_id: &str) -> Vec<RosterLine> {
    let Ok(text) = std::fs::read_to_string(roster_path(runs_root, workflow_run_id)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<RosterLine>(l).ok())
        .collect()
}
