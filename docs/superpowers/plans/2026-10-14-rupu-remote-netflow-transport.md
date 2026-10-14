# Remote Netflow Transport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A placed unit's (and a wholly placed run's) netflow ledgers come back to the coordinator when the unit ends. They land under `<global>/netflow/<ledger id>.jsonl`, and every CP netflow scope reads them; a copy that is short or missing is reported as `incomplete`, never shown as whole.

**Architecture:** On the host, a root run started with `--run-id` writes `runs/<root>/netflow.jsonl`: a `begin` line, then every ledger line of every run beneath it, each tagged with the ledger's id. Each transport carries that file on a channel it already has (SSH terminal pull, tunnel `ArtifactFile` frames, bucket result objects, an HTTP GET). The coordinator splits the stream per ledger id into its global netflow directory and writes `runs/<run id>/netflow.landing.json`. That record is complete, partial or unavailable, and lists the ledger ids that landed. The runner also writes a roster of placed unit attempts, `netflow.units.jsonl`, in the workflow run's directory. The CP's id walk reads both the landing records and the roster.

**Tech Stack:** Rust 2021 (workspace MSRV 1.95, `#![deny(clippy::all)]`, no `unsafe`), tokio, serde/serde_json, chrono, thiserror, std `File::lock` for cross-process locks, axum (CP), insta (snapshots), React/TypeScript + vitest (CP web).

**Spec:** `docs/superpowers/specs/2026-10-14-rupu-remote-netflow-transport-design.md` (read it first). Sibling: `docs/superpowers/specs/2026-09-30-rupu-remote-findings-transport-design.md`, whose coverage stream is the pattern every task below copies.

## Global Constraints

- Workspace deps only: a new dependency goes in the root `Cargo.toml` `[workspace.dependencies]`, never with a version in a crate's `Cargo.toml`.
- `#![deny(clippy::all)]`; `unsafe_code` is forbidden. CI clippy is 1.95, so don't write `!x.is_some_and(..)`, and don't write a `match` with a single `if`-only arm.
- Integration tests: ONE binary per crate, as modules under `crates/<c>/tests/it/` registered in `main.rs`. Never add a new top-level `tests/*.rs`.
- Never run package-wide `cargo fmt`. Run `rustfmt --edition 2021 <file>` only on the files you changed (main is not fmt-clean).
- Never `git stash` / `git stash pop`; to set work aside, make a WIP commit. Commit unsigned: `git -c commit.gpgsign=false commit`.
- Capture must never fail a run: a netflow stream write that fails is logged (`tracing::warn!`) and the run continues.
- Missing netflow must never look like "the unit made no calls". A stream with no `begin` line is `unavailable`; a copy that wasn't confirmed whole is `partial`.
- `rupu-cli` is thin: no business logic in it beyond wiring. Landing and parsing live in `rupu-netflow`; transport code lives in `rupu-cp`.
- Run only the touched test modules (`cargo test -p <crate> --test it <module>::` or `cargo test -p <crate> --lib <path>::`), never a cold `cargo test --workspace`.
- File names and constants, verbatim:
  - stream `netflow.jsonl`
  - stream lock `netflow.jsonl.lock`
  - SSH partial marker `netflow.jsonl.partial`
  - landing record `netflow.landing.json` (lock `netflow.landing.json.lock`)
  - roster `netflow.units.jsonl` (lock `netflow.units.jsonl.lock`)
  - tunnel capability `mirror.netflow`
  - HTTP feature `run.netflow_stream`
  - HTTP route `GET /api/runs/:id/netflow/stream`
  - bucket keys `netflow.<seq:04>.jsonl`

## Review Focus

1. **A bucket poller re-mirrors `run.json` every tick after the run is terminal.** Landing must not re-copy every ledger on each tick once a `complete` record exists (Task 2: `land_if_terminal` returns early on an existing complete record).
2. **A tunnel or bucket node reports terminal through its `run.json` artifact before `RunFinished`.** The dispatcher sees the run terminal through `get_run`, so the landing record must exist before the mirrored status flips to terminal (Task 2: land *before* `run_store.update`).
3. **A forged or corrupt stream must not write outside `<global>/netflow/`.** A `ledger` id such as `../x` or `a/b`, or one that is empty or 200 characters long, is skipped and counted (Task 1: `is_safe_ledger_id` tests).
4. **A future ledger line kind from a newer host must survive the trip.** The stream's `line` is carried as raw JSON (`serde_json::Value`), never re-serialized through `LedgerLine` (Task 1: unknown-variant round-trip test).
5. **A `rupu workflow resume` on the host appends to an already begun stream.** It must not write a second `begin` line, and the coordinator's re-land must replace the earlier landing, not duplicate it (Task 1: `write_stream_begin` is idempotent; replace test).

---

## Execution layout (read before starting)

The tasks are written so that, after **Phase 0**, the Phase 1 tasks touch disjoint files and can run in parallel, each in its own git worktree branched from the Phase 0 commit. Phase 2 merges them and adds the end-to-end test and docs.

| Phase | Task | Crates / files owned |
|---|---|---|
| 0 | 1. Stream + landing primitives | `crates/rupu-netflow/src/{stream.rs,land.rs,lib.rs}`, `crates/rupu-netflow/tests/it/{main.rs,stream_land.rs}` |
| 0 | 2. CP contracts | `crates/rupu-cp/src/node/{protocol.rs,mirror.rs}`, `crates/rupu-cp/src/host/connector.rs`, every `impl HostConnector` (adds one method) |
| 1 | 3. Host tee | `crates/rupu-netflow/src/ledger/{writer.rs,paths.rs}`, `crates/rupu-runtime/src/{netflow.rs,assembly/*}`, `crates/rupu-runtime/tests/it/assembly.rs` (+ snapshot), `crates/rupu-cli/src/{netflow_sink.rs,cmd/run.rs,cmd/workflow.rs,resume.rs,cmd/session.rs,cmd/agentiflow.rs,cmd/transcript.rs}` |
| 1 | 4. SSH | `crates/rupu-cp/src/host/ssh.rs` |
| 1 | 5. Tunnel + bucket | `crates/rupu-cli/src/cmd/node.rs`, `crates/rupu-cp/src/host/bucket/poller.rs`, `crates/rupu-cp/src/node/server.rs` (tests only), `crates/rupu-cp/tests/it/node_tunnel.rs` |
| 1 | 6. HTTP | `crates/rupu-cp/src/api/runs.rs`, `crates/rupu-cp/src/host/http.rs`, `crates/rupu-cp/tests/it/host_http.rs` |
| 1 | 7. Orchestrator + dispatcher | `crates/rupu-orchestrator/src/runner.rs`, `crates/rupu-cli/src/fleet_unit_dispatcher.rs`, `crates/rupu-orchestrator/tests/it/{main.rs,remote_netflow_roster.rs}` |
| 1 | 8. CP read side | `crates/rupu-cp/src/api/netflow.rs`, `crates/rupu-cli/src/cmd/netflow.rs` |
| 1 | 9. Web disclosure | `crates/rupu-cp/web/src/components/netflow/ScopeDisclosure.tsx` (+ its test) |
| 2 | 10. End to end, docs | `crates/rupu-cli/tests/it/{main.rs,remote_netflow_e2e.rs}`, `CLAUDE.md`, the spec's "As built" |

Never edit a file another Phase 1 task owns. If you need a helper that lives in another task's file, it is already defined in Phase 0 (Tasks 1–2); use it.

---

## Phase 0

### Task 1: Stream and landing primitives (`rupu-netflow`)

**Files:**
- Create: `crates/rupu-netflow/src/stream.rs`
- Create: `crates/rupu-netflow/src/land.rs`
- Modify: `crates/rupu-netflow/src/lib.rs` (declare the modules; re-export)
- Create: `crates/rupu-netflow/tests/it/stream_land.rs`
- Modify: `crates/rupu-netflow/tests/it/main.rs` (add `mod stream_land;`)

**Interfaces:**
- Consumes: `rupu_netflow::{LedgerLine, global_netflow_dir, ensure_netflow_dir}` (existing; `crates/rupu-netflow/src/ledger/paths.rs:39,169`, `record.rs:147`).
- Produces (every later task uses these exact names):
  - `rupu_netflow::stream::{STREAM_FILE, STREAM_VERSION, NetflowStream, stream_path, write_stream_begin, stream_has_begin, append_stream_line}`
  - `rupu_netflow::land::{LANDING_FILE, ROSTER_FILE, LandingStatus, LandingRecord, LandReport, ParsedStream, parse_stream, is_safe_ledger_id, landing_record_path, read_landing_record, land_and_record, RosterLine, roster_path, append_roster_line, read_roster, NOT_STREAMED_REASON, NO_BEGIN_REASON}`

- [ ] **Step 1: Write the stream module.** Create `crates/rupu-netflow/src/stream.rs`:

```rust
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
    Begin { v: u32, run_id: &'a str },
    Line { ledger: &'a str, line: &'a LedgerLine },
}

/// Append `line` (a JSON object without its newline) as ONE write, holding the
/// stream's lock file so writers in several tasks or processes never interleave.
fn locked_append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(path))?;
    lock.lock()?;
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let res = f.write_all(buf.as_bytes());
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
pub fn write_stream_begin(path: &Path, run_id: &str) -> std::io::Result<()> {
    if stream_has_begin(path) {
        return Ok(());
    }
    let line = serde_json::to_string(&Out::Begin { v: STREAM_VERSION, run_id })?;
    locked_append(path, &line)
}

/// Tee one ledger line into the stream, tagged with `ledger`.
pub fn append_stream_line(path: &Path, ledger: &str, line: &LedgerLine) -> std::io::Result<()> {
    let line = serde_json::to_string(&Out::Line { ledger, line })?;
    locked_append(path, &line)
}
```

  `write_stream_begin` has a check-then-write race between two processes starting the same root. Only one process starts a given root, so that is acceptable; don't add more locking.

- [ ] **Step 2: Write the landing module.** Create `crates/rupu-netflow/src/land.rs`:

```rust
//! Landing a remote run's netflow stream on the coordinator (spec §N3):
//! split it by ledger id into `<global>/netflow/<ledger>.jsonl`, record the
//! result in `<global>/runs/<run_id>/netflow.landing.json`, and keep the
//! workflow run's roster of placed unit attempts.

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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LandReport {
    pub begin_seen: bool,
    pub ledgers: Vec<String>,
    pub malformed: usize,
    pub unsafe_ids: usize,
}

/// A ledger id that is safe as a file name: 1–128 chars of `[A-Za-z0-9_-]`.
pub fn is_safe_ledger_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            out.malformed += 1;
            continue;
        };
        match v.get("kind").and_then(|k| k.as_str()) {
            Some("begin") if was_first => out.begin_seen = true,
            Some("line") => {
                let (Some(ledger), Some(line)) =
                    (v.get("ledger").and_then(|l| l.as_str()), v.get("line"))
                else {
                    out.malformed += 1;
                    continue;
                };
                if !is_safe_ledger_id(ledger) {
                    out.unsafe_ids += 1;
                    continue;
                }
                out.ledgers
                    .entry(ledger.to_string())
                    .or_default()
                    .push(line.to_string());
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
    let mut lock_name = record_path.file_name().unwrap_or_default().to_os_string();
    lock_name.push(".lock");
    let lock = record_path.with_file_name(lock_name);
    with_file_lock(&lock, || {
        let existing = read_landing_record(runs_root, run_id);
        let parsed = stream.map(parse_stream);
        let (status, why) = match &parsed {
            None => (
                LandingStatus::Unavailable,
                Some(reason.clone().unwrap_or_else(|| NOT_STREAMED_REASON.to_string())),
            ),
            Some(p) if !p.begin_seen => (LandingStatus::Unavailable, Some(NO_BEGIN_REASON.to_string())),
            Some(p) => {
                let skipped = p.malformed + p.unsafe_ids;
                let mut parts: Vec<String> = reason.clone().into_iter().collect();
                if skipped > 0 {
                    parts.push(format!("{skipped} malformed netflow stream line(s) skipped"));
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
            reason: if status == LandingStatus::Complete { None } else { why },
            ledgers,
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
    let mut lock_name = path.file_name().unwrap_or_default().to_os_string();
    lock_name.push(".lock");
    let lock = path.with_file_name(lock_name);
    let mut json = serde_json::to_string(line)?;
    json.push('\n');
    with_file_lock(&lock, || {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
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
```

- [ ] **Step 3: Register and re-export.** In `crates/rupu-netflow/src/lib.rs`, add `pub mod land;` and `pub mod stream;` next to the other `pub mod` lines. Then, near the existing re-exports (lines 50–59), add:

```rust
pub use stream::{NetflowStream, STREAM_FILE as NETFLOW_STREAM_FILE};
```

  Keep every other item path-qualified (`rupu_netflow::land::…`, `rupu_netflow::stream::…`).

- [ ] **Step 4: Write the tests.** Create `crates/rupu-netflow/tests/it/stream_land.rs` and add `mod stream_land;` to `crates/rupu-netflow/tests/it/main.rs`:

```rust
use rupu_netflow::land::*;
use rupu_netflow::stream::*;
use rupu_netflow::LedgerLine;

fn dropped(n: u64) -> LedgerLine {
    LedgerLine::Dropped { count: n, ts: chrono::Utc::now() }
}

#[test]
fn begin_is_written_once_and_lines_are_tagged() {
    let tmp = tempfile::tempdir().unwrap();
    let p = stream_path(tmp.path(), "run_A");
    write_stream_begin(&p, "run_A").unwrap();
    write_stream_begin(&p, "run_A").unwrap();
    append_stream_line(&p, "run_A", &dropped(1)).unwrap();
    append_stream_line(&p, "sub_B", &dropped(2)).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].contains(r#""kind":"begin""#));
    assert!(lines[1].contains(r#""ledger":"run_A""#) && lines[1].contains(r#""type":"dropped""#));
    assert!(stream_has_begin(&p));
}

#[test]
fn parallel_writers_interleave_whole_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let p = stream_path(tmp.path(), "run_P");
    write_stream_begin(&p, "run_P").unwrap();
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let p = p.clone();
            std::thread::spawn(move || {
                for _ in 0..50 {
                    append_stream_line(&p, &format!("run_{t}"), &dropped(t)).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let parsed = parse_stream(&std::fs::read(&p).unwrap());
    assert!(parsed.begin_seen);
    assert_eq!(parsed.malformed, 0);
    assert_eq!(parsed.ledgers.values().map(Vec::len).sum::<usize>(), 400);
}

#[test]
fn landing_splits_by_ledger_and_records_complete() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let s = stream_path(tmp.path().join("host").as_path(), "run_U");
    write_stream_begin(&s, "run_U").unwrap();
    append_stream_line(&s, "run_U", &dropped(1)).unwrap();
    append_stream_line(&s, "sub_X", &dropped(2)).unwrap();
    let rec = land_and_record(&runs, "run_U", "host_ssh", Some(&std::fs::read(&s).unwrap()), true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Complete);
    assert_eq!(rec.ledgers, vec!["run_U".to_string(), "sub_X".to_string()]);
    assert!(tmp.path().join("netflow/run_U.jsonl").exists());
    assert!(tmp.path().join("netflow/sub_X.jsonl").exists());
    assert_eq!(read_landing_record(&runs, "run_U").unwrap(), rec);
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("netflow"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn no_stream_and_no_begin_are_unavailable_and_land_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let rec = land_and_record(&runs, "run_N", "h", None, true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Unavailable);
    assert_eq!(rec.reason.as_deref(), Some(NOT_STREAMED_REASON));
    let body = br#"{"kind":"line","ledger":"run_N","line":{"type":"dropped","count":1,"ts":"2026-10-14T00:00:00Z"}}"#;
    let rec = land_and_record(&runs, "run_M", "h", Some(body), true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Unavailable);
    assert_eq!(rec.reason.as_deref(), Some(NO_BEGIN_REASON));
    assert!(!tmp.path().join("netflow/run_N.jsonl").exists());
}

#[test]
fn unsafe_ids_and_malformed_lines_are_skipped_and_make_it_partial() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let body = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_Z"}"#, "\n",
        r#"{"kind":"line","ledger":"../escape","line":{"type":"x"}}"#, "\n",
        r#"{"kind":"line","ledger":"a/b","line":{"type":"x"}}"#, "\n",
        "not json\n",
        r#"{"kind":"line","ledger":"run_Z","line":{"type":"x"}}"#, "\n",
    );
    let rec = land_and_record(&runs, "run_Z", "h", Some(body.as_bytes()), true, None).unwrap();
    assert_eq!(rec.status, LandingStatus::Partial);
    assert!(rec.reason.unwrap().contains("3 malformed"));
    assert!(!tmp.path().join("escape.jsonl").exists());
    assert!(!tmp.path().join("netflow/escape.jsonl").exists());
    assert!(!is_safe_ledger_id(""));
    assert!(!is_safe_ledger_id(&"a".repeat(129)));
    assert!(is_safe_ledger_id("run_01ABC-x"));
}

#[test]
fn unknown_line_kinds_survive_landing_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let body = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_F"}"#, "\n",
        r#"{"kind":"line","ledger":"run_F","line":{"type":"from_the_future","x":[1,2]}}"#, "\n",
    );
    land_and_record(&runs, "run_F", "h", Some(body.as_bytes()), true, None).unwrap();
    let landed = std::fs::read_to_string(tmp.path().join("netflow/run_F.jsonl")).unwrap();
    assert_eq!(landed.trim(), r#"{"type":"from_the_future","x":[1,2]}"#);
}

#[test]
fn complete_is_never_downgraded_and_a_newer_complete_replaces() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let one = concat!(
        r#"{"kind":"begin","v":1,"run_id":"run_R"}"#, "\n",
        r#"{"kind":"line","ledger":"run_R","line":{"type":"a"}}"#, "\n",
    );
    land_and_record(&runs, "run_R", "h", Some(one.as_bytes()), true, None).unwrap();
    let rec = land_and_record(&runs, "run_R", "h", None, false, Some("later".into())).unwrap();
    assert_eq!(rec.status, LandingStatus::Complete, "kept");
    let two = format!("{one}{}\n", r#"{"kind":"line","ledger":"run_R","line":{"type":"b"}}"#);
    land_and_record(&runs, "run_R", "h", Some(two.as_bytes()), true, None).unwrap();
    let landed = std::fs::read_to_string(tmp.path().join("netflow/run_R.jsonl")).unwrap();
    assert_eq!(landed.lines().count(), 2, "replaced, not appended: {landed}");
}

#[test]
fn partial_then_complete_upgrades() {
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let body = concat!(r#"{"kind":"begin","v":1,"run_id":"run_Q"}"#, "\n");
    let r = land_and_record(&runs, "run_Q", "h", Some(body.as_bytes()), false, Some("snapshot".into())).unwrap();
    assert_eq!((r.status, r.reason.as_deref()), (LandingStatus::Partial, Some("snapshot")));
    let r = land_and_record(&runs, "run_Q", "h", Some(body.as_bytes()), true, None).unwrap();
    assert_eq!((r.status, r.reason), (LandingStatus::Complete, None));
}

#[test]
fn roster_appends_and_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let l = RosterLine { unit_run_id: "run_u1".into(), host: "h".into(), step_id: "s".into(), index: Some(2) };
    append_roster_line(tmp.path(), "run_wf", &l).unwrap();
    append_roster_line(tmp.path(), "run_wf", &RosterLine { index: None, ..l.clone() }).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(roster_path(tmp.path(), "run_wf"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"garbage\n"))
        .unwrap();
    let got = read_roster(tmp.path(), "run_wf");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], l);
    assert!(read_roster(tmp.path(), "run_none").is_empty());
}
```

  If `tempfile` is not yet a dev-dependency of `rupu-netflow`, add `tempfile.workspace = true` under `[dev-dependencies]` (it is used by `ledger/paths.rs` tests already, so it should be there).

- [ ] **Step 5: Run the tests to verify they fail, then pass.**

  Run: `cargo test -p rupu-netflow --test it stream_land::`

  Expected: before Steps 1–3 are in place, the build fails; after them, all 9 tests pass. Then run `cargo clippy -p rupu-netflow --all-targets -- -D warnings`.

- [ ] **Step 6: Commit.**

```bash
git add crates/rupu-netflow
git -c commit.gpgsign=false commit -m "feat(netflow): run-scoped netflow stream + landing primitives

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: CP contracts (protocol, mirror landing hook, connector method)

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs`
  - after line 313: two constants
  - `cp_capabilities` (320–325)
  - `host_features` (371–387)
  - `ArtifactFile` (436–455)
- Modify: `crates/rupu-cp/src/node/mirror.rs`
  - `append_blocking` (231–306)
  - `finish_blocking` (549–572)
  - new methods next to `coverage_path` (436)
- Modify: `crates/rupu-cp/src/host/connector.rs`: the trait (after `unit_coverage`, ~402), a new enum `NetflowLanding`, and the test doubles `StubConnector` (1468) and `Bare` (1597).
- Modify (one method each): `crates/rupu-cp/src/host/{ssh.rs:2933, tunnel.rs:346, local.rs:144, http.rs:406, bucket/connector.rs:207}`, `crates/rupu-cp/src/api/graph.rs:1138`, `crates/rupu-cp/src/api/usage.rs:1497`, `crates/rupu-cp/src/api/runs.rs:3136,4024,4590`, `crates/rupu-cp/tests/it/host_registry.rs:31`, `crates/rupu-cli/src/fleet_unit_dispatcher.rs:1202,1397,1935,2466,2639`.

**Interfaces:**
- Consumes: Task 1's `rupu_netflow::land::{land_and_record, read_landing_record, LandingStatus, LandingRecord}` and `rupu_netflow::stream::{stream_path, STREAM_FILE}`.
- Produces:
  - `rupu_cp::node::protocol::{CAP_MIRROR_NETFLOW = "mirror.netflow", CAP_RUN_NETFLOW_STREAM = "run.netflow_stream", ArtifactFile::Netflow}`
  - `NodeMirror::netflow_path(&self, run_id: &str) -> PathBuf`
  - `NodeMirror::replace_netflow(&self, run_id: &str, node_id: &str, body: &str, complete: bool, reason: Option<String>) -> Result<(), MirrorError>` (async)
  - `NodeMirror::land_netflow_if_terminal` (private, blocking)
  - `rupu_cp::host::connector::NetflowLanding`
  - `HostConnector::land_run_netflow(&self, run_id: &str, terminal: bool, runs_root: &Path) -> Result<NetflowLanding, HostConnectorError>`

- [ ] **Step 1: Protocol.** In `protocol.rs`:
  - Add the constants:

```rust
/// A tunnel CP that mirrors [`ArtifactFile::Netflow`] (sent in `Welcome`);
/// a node sends netflow frames only to a CP that advertised it.
pub const CAP_MIRROR_NETFLOW: &str = "mirror.netflow";
/// An HTTP host whose CP serves `GET /api/runs/:id/netflow/stream`.
pub const CAP_RUN_NETFLOW_STREAM: &str = "run.netflow_stream";
```

  - Add `CAP_MIRROR_NETFLOW.to_string()` to `cp_capabilities()`'s vec, and `CAP_RUN_NETFLOW_STREAM` to `host_features()` next to `CAP_RUN_COVERAGE_STREAM`.
  - Add the variant after `Coverage`:

```rust
    /// The run's netflow stream (`runs/<run_id>/netflow.jsonl`, spec
    /// 2026-10-14 §N1). Sent only to a CP that advertised
    /// [`CAP_MIRROR_NETFLOW`] — an older CP fails to parse an unknown variant.
    Netflow,
```

  - Add a protocol unit test next to the coverage one (~614): assert that `serde_json::to_string(&ArtifactFile::Netflow)` is `"\"netflow\""`, and that `cp_capabilities()` contains `CAP_MIRROR_NETFLOW`.

- [ ] **Step 2: Write the failing mirror tests.** In `mirror.rs`'s test module, next to the coverage mirror tests (705–900), reusing their setup helper (the one that creates a run owned by `node_id`; read the 705 test and copy its setup):

```rust
#[tokio::test]
async fn netflow_frames_append_and_land_before_the_run_turns_terminal() {
    // set up: mirror + a run "run_N1" owned by "node_a" (same as the coverage tests)
    let begin = r#"{"kind":"begin","v":1,"run_id":"run_N1"}"#;
    let line = r#"{"kind":"line","ledger":"run_N1","line":{"type":"dropped","count":1,"ts":"2026-10-14T00:00:00Z"}}"#;
    mirror.append("run_N1", "node_a", ArtifactFile::Netflow, begin).await.unwrap();
    mirror.append("run_N1", "node_a", ArtifactFile::Netflow, line).await.unwrap();
    assert!(mirror.netflow_path("run_N1").exists());
    // no landing yet: the run is still running
    assert!(rupu_netflow::land::read_landing_record(&run_store.root, "run_N1").is_none());
    // terminal run.json lands first
    let mut rec = run_store.load("run_N1").unwrap();
    rec.status = rupu_orchestrator::runs::RunStatus::Completed;
    mirror
        .append("run_N1", "node_a", ArtifactFile::RunJson, &serde_json::to_string(&rec).unwrap())
        .await
        .unwrap();
    let landed = rupu_netflow::land::read_landing_record(&run_store.root, "run_N1").unwrap();
    assert_eq!(landed.status, rupu_netflow::land::LandingStatus::Complete);
    assert_eq!(landed.host, "node_a");
    let global = run_store.root.parent().unwrap();
    assert!(global.join("netflow/run_N1.jsonl").exists());
}

#[tokio::test]
async fn finish_without_a_stream_records_unavailable() {
    // run "run_N2" owned by "node_a", no netflow frames
    mirror.finish("run_N2", "node_a", "completed").await.unwrap();
    let r = rupu_netflow::land::read_landing_record(&run_store.root, "run_N2").unwrap();
    assert_eq!(r.status, rupu_netflow::land::LandingStatus::Unavailable);
}

#[tokio::test]
async fn an_ssh_partial_copy_lands_partial() {
    // run "run_N3" owned by "host_ssh"
    let body = "{\"kind\":\"begin\",\"v\":1,\"run_id\":\"run_N3\"}\n";
    mirror
        .replace_netflow("run_N3", "host_ssh", body, false, Some("the host's run.json was not terminal".into()))
        .await
        .unwrap();
    mirror.finish("run_N3", "host_ssh", "completed").await.unwrap();
    let r = rupu_netflow::land::read_landing_record(&run_store.root, "run_N3").unwrap();
    assert_eq!(r.status, rupu_netflow::land::LandingStatus::Partial);
    assert!(r.reason.unwrap().contains("not terminal"));
}

#[tokio::test]
async fn re_mirroring_terminal_run_json_does_not_re_land_a_complete_copy() {
    // after the first test's terminal landing, take the landed file's mtime,
    // append the terminal run.json again, and assert the record's `at` and
    // the landed file are unchanged
}
```

  Write the fourth test in full, following its comment. It reads the landing record before and after the second terminal `RunJson` append and asserts the record is unchanged (`at` equal).

- [ ] **Step 3: Run them to verify they fail.**

  Run: `cargo test -p rupu-cp --lib node::mirror::`

  Expected: compile errors (`ArtifactFile::Netflow` not handled; `netflow_path` and `replace_netflow` missing).

- [ ] **Step 4: Implement the mirror.** In `mirror.rs`:
  - Add `ArtifactFile::Netflow => { append_line(&self.netflow_path(run_id), line)?; }` to `append_blocking`'s match.
  - Add the methods below next to `coverage_path`.
  - In `append_blocking`'s `RunJson` arm, just before `self.run_store.update(&incoming)?;`, add:

```rust
                if is_terminal(&incoming.status) {
                    self.land_netflow_if_terminal(run_id, node_id);
                }
```

  - In `finish_blocking`, after the ownership check and before `run_store.update`, add `self.land_netflow_if_terminal(run_id, node_id);`.
  - If no `is_terminal(&RunStatus)` helper exists in `mirror.rs`, use `RunStatus`'s own terminal predicate (search `fn is_terminal` in `crates/rupu-orchestrator/src/runs.rs`). If there is none, match `Completed | Failed | Cancelled | Rejected` explicitly.

```rust
    /// The mirrored copy of a run's netflow stream.
    pub fn netflow_path(&self, run_id: &str) -> PathBuf {
        rupu_netflow::stream::stream_path(&self.run_store.root, run_id)
    }

    fn netflow_partial_marker(&self, run_id: &str) -> PathBuf {
        let mut p = self.netflow_path(run_id).into_os_string();
        p.push(".partial");
        PathBuf::from(p)
    }

    /// Replace the mirrored stream with `body` (SSH's terminal pull). When
    /// `complete` is false a `netflow.jsonl.partial` marker holding `reason`
    /// says the copy was not confirmed whole; when true the marker is removed.
    pub async fn replace_netflow(
        &self,
        run_id: &str,
        node_id: &str,
        body: &str,
        complete: bool,
        reason: Option<String>,
    ) -> Result<(), MirrorError> {
        let this = self.clone();
        let (run_id, node_id, body) = (run_id.to_string(), node_id.to_string(), body.to_string());
        tokio::task::spawn_blocking(move || {
            validate_run_id(&run_id)?;
            let existing = this.run_store.load(&run_id)?;
            if existing.worker_id.as_deref() != Some(node_id.as_str()) {
                return Err(MirrorError::WrongNode(run_id.clone()));
            }
            let path = this.netflow_path(&run_id);
            let marker = this.netflow_partial_marker(&run_id);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension(format!("jsonl.{}.tmp", ulid::Ulid::new()));
            if let Err(e) = std::fs::write(&tmp, body.as_bytes()).and_then(|_| std::fs::rename(&tmp, &path)) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
            if complete {
                let _ = std::fs::remove_file(&marker);
            } else {
                std::fs::write(&marker, reason.unwrap_or_default().as_bytes())?;
            }
            Ok(())
        })
        .await
        .map_err(|e| MirrorError::Io(std::io::Error::other(e.to_string())))?
    }

    /// Land the run's mirrored netflow stream once its run turns terminal
    /// (spec §N3): complete unless an SSH `.partial` marker says otherwise;
    /// `unavailable` when no stream arrived. Best-effort: a failure is
    /// logged and the run's terminal update proceeds (the missing record
    /// reads as "not collected"). A run that already landed complete is
    /// left alone, so a bucket poller re-mirroring `run.json` every tick
    /// costs one small read.
    fn land_netflow_if_terminal(&self, run_id: &str, node_id: &str) {
        use rupu_netflow::land::{land_and_record, read_landing_record, LandingStatus};
        let root = &self.run_store.root;
        if read_landing_record(root, run_id).is_some_and(|r| r.status == LandingStatus::Complete) {
            return;
        }
        let stream = std::fs::read(self.netflow_path(run_id)).ok();
        let partial = std::fs::read_to_string(self.netflow_partial_marker(run_id)).ok();
        let complete = partial.is_none();
        let reason = partial.map(|r| {
            if r.trim().is_empty() { "the copy was not confirmed whole".to_string() } else { r }
        });
        if let Err(e) = land_and_record(root, run_id, node_id, stream.as_deref(), complete, reason) {
            tracing::warn!(run_id, node_id, error = %e, "netflow landing failed; the run's flows will read as not collected");
        }
    }
```

  `NodeMirror` must be `Clone` for `replace_netflow`. Its only field is an `Arc`, so add `#[derive(Clone)]` if missing. Match the existing `replace_coverage` for how it hops to the blocking pool and maps errors; if `MirrorError` has no `Io` variant, use the variant `replace_coverage` uses. `is_some_and` is fine here because it is not negated.

- [ ] **Step 5: Add the connector method.** In `connector.rs`, next to `CoverageRead` (~1041), add:

```rust
/// How a connector delivers a unit's netflow stream (spec §N2/§N3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetflowLanding {
    /// The connector's mirror lands it when the run turns terminal
    /// (SSH, tunnel, bucket); read the landing record.
    ByMirror,
    /// The unit's ledgers are already on this machine (local).
    NotNeeded,
    /// The connector landed it now (HTTP); its record has this status.
    Landed(rupu_netflow::land::LandingStatus),
}
```

  Then add this to the trait, right after `unit_coverage`, with no default:

```rust
    /// Bring a unit's netflow stream to the coordinator, or say who does.
    /// `terminal` is false for a snapshot read (a poll error after the run
    /// was observed, the wall timeout). `runs_root` is the coordinator's
    /// `<global>/runs`. Required: every transport must say how.
    async fn land_run_netflow(
        &self,
        run_id: &str,
        terminal: bool,
        runs_root: &std::path::Path,
    ) -> Result<NetflowLanding, HostConnectorError>;
```

- [ ] **Step 6: Implement it on every impl.**
  - SSH, tunnel, bucket connectors: `Ok(NetflowLanding::ByMirror)`.
  - `LocalHostConnector`: `Ok(NetflowLanding::NotNeeded)`.
  - `HttpHostConnector`: `Err(HostConnectorError::Unsupported("netflow stream collection over HTTP is not implemented".into()))`. Task 6 replaces this; it is the only temporary body in the plan.
  - Test doubles: `MirrorBackedConnector`, `MirrorOnlyHostConnector`, `FakeHostConnector`, `StubConnector`, `Bare`, `RollupOnlyConnector`, `StubLocal` and every `rupu-cli` fleet double get `Ok(NetflowLanding::NotNeeded)`. `ResumeHostConnector` gets `unimplemented!("not used")`, matching its other methods.
  - Each impl takes the same three parameters (prefix unused ones with `_`).

- [ ] **Step 7: Run the tests.**

  Run: `cargo test -p rupu-cp --lib node:: && cargo test -p rupu-cp --lib host::connector:: && cargo build -p rupu-cli --tests`

  Expected: PASS and builds. Then run `cargo clippy -p rupu-cp -p rupu-cli --all-targets -- -D warnings`.

- [ ] **Step 8: Commit.**

```bash
git add crates/rupu-cp crates/rupu-cli/src/fleet_unit_dispatcher.rs
git -c commit.gpgsign=false commit -m "feat(cp): netflow mirror landing hook, ArtifactFile::Netflow, HostConnector::land_run_netflow

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Phase 1 (parallel; each task in its own worktree from the Phase 0 commit)

### Task 3: Host tee (writer, assembler, CLI roots)

**Files:**
- Modify: `crates/rupu-netflow/src/ledger/paths.rs` (`NetflowPaths` gains `stream`)
- Modify: `crates/rupu-netflow/src/ledger/writer.rs` (`run_writer` / `write_line` tee)
- Modify: `crates/rupu-runtime/src/netflow.rs` (`for_run` gains `stream: Option<NetflowStream>`)
- Modify: `crates/rupu-runtime/src/assembly/origin.rs` (a `NetflowStreamRoot` column in `OriginDefaults` / `defaults_for`; `ParentRun.netflow_stream`)
- Modify: `crates/rupu-runtime/src/assembly/mod.rs:573-582` (resolve the stream before `for_run`)
- Modify: `crates/rupu-runtime/tests/it/assembly.rs` (`Derived.netflow_stream`) and `crates/rupu-runtime/tests/it/snapshots/it__assembly__origin_matrix.snap`
- Modify: every `for_run` caller:
  - `crates/rupu-cli/src/netflow_sink.rs:39`
  - `crates/rupu-cli/src/cmd/run.rs:897`
  - `crates/rupu-cli/src/cmd/workflow.rs:3763,4756,5547`
  - `crates/rupu-cli/src/resume.rs:370`
  - `crates/rupu-cli/src/cmd/session.rs:7763,8306`
  - `crates/rupu-cli/src/cmd/agentiflow.rs:276`
- Modify: `crates/rupu-cli/src/cmd/transcript.rs:2388-2394` (doc comment only)

**Interfaces:**
- Consumes: Task 1's `rupu_netflow::stream::{NetflowStream, stream_path, write_stream_begin, stream_has_begin, append_stream_line}`.
- Produces:
  - `rupu_netflow::NetflowPaths { root, flows, stream: Option<NetflowStream> }`, with `NetflowPaths::with_stream(self, Option<NetflowStream>) -> Self`
  - `rupu_runtime::netflow::for_run(global, project_root, run_id, transcript_path, stream: Option<NetflowStream>)`
  - `rupu_runtime::netflow::stream_for(global: &Path, root_run_dir: &Path, ledger_id: &str) -> Option<NetflowStream>`, which returns `Some` only when `<root_run_dir>/netflow.jsonl` has a `begin` line
  - `rupu_runtime::netflow::link_stream(global, link: &LaunchLink, ledger_id) -> Option<NetflowStream>`, which uses `link.usage_root?.parent()`

**Rules (spec §N1):**
- A *root* writes `begin`:
  - `rupu run` when `args.run_id.is_some()` and there is no launch link;
  - `rupu workflow run` when `run_id_override.is_some()` in `run_with_outcome` and there is no launch link.
- Every run then tees iff its candidate stream has a `begin` line. The candidate is chosen in this order:
  1. a `LaunchLink` → `link.usage_root.parent()/netflow.jsonl`;
  2. `Origin::SubAgent` → the parent's stream;
  3. `Origin::WorkflowStep { workflow_run_id }` → `<global>/runs/<workflow_run_id>/netflow.jsonl`;
  4. `Origin::Standalone` → `<global>/runs/<run_id>/netflow.jsonl`;
  5. `SessionTurn`, `FlowLead` and `FlowUnit` without a link → none.

- [ ] **Step 1: Write the failing writer test.** In `writer.rs`'s `mod tests`:

```rust
#[tokio::test]
async fn ledger_lines_are_teed_into_a_configured_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let stream = crate::stream::stream_path(&tmp.path().join("runs"), "run_root");
    crate::stream::write_stream_begin(&stream, "run_root").unwrap();
    let paths = NetflowPaths::for_run(tmp.path(), "sub_1").with_stream(Some(crate::stream::NetflowStream {
        path: stream.clone(),
        ledger_id: "sub_1".into(),
    }));
    let handle = NetflowWriterHandle::spawn_with_capacity_and_interval(paths, 1, std::time::Duration::from_millis(50)).unwrap();
    // fill the channel so one record is dropped, then shut down: the final Dropped line must be teed too
    let rec = /* build a FlowRecord the way `writer_appends_flow_and_complete_lines` does */;
    handle.writer.record(&rec);
    handle.writer.record(&rec);
    handle.writer.record(&rec);
    handle.shutdown().await;
    let parsed = crate::land::parse_stream(&std::fs::read(&stream).unwrap());
    assert!(parsed.begin_seen);
    let lines = &parsed.ledgers["sub_1"];
    assert!(lines.iter().any(|l| l.contains(r#""type":"flow""#)));
    let ledger = std::fs::read_to_string(tmp.path().join("sub_1.jsonl")).unwrap();
    assert_eq!(ledger.lines().count(), lines.len(), "every ledger line is teed, Dropped included");
}

#[tokio::test]
async fn a_failing_stream_never_drops_ledger_lines() {
    let tmp = tempfile::TempDir::new().unwrap();
    // the stream's parent is a FILE, so every stream write fails
    std::fs::write(tmp.path().join("blocker"), b"").unwrap();
    let paths = NetflowPaths::for_run(tmp.path(), "run_x").with_stream(Some(crate::stream::NetflowStream {
        path: tmp.path().join("blocker").join("netflow.jsonl"),
        ledger_id: "run_x".into(),
    }));
    let handle = NetflowWriterHandle::spawn(paths).unwrap();
    let rec = /* same FlowRecord */;
    handle.writer.record(&rec);
    handle.shutdown().await;
    let ledger = std::fs::read_to_string(tmp.path().join("run_x.jsonl")).unwrap();
    assert!(ledger.contains(r#""type":"flow""#));
    assert!(!ledger.contains(r#""type":"dropped""#), "a stream failure is not ledger loss");
}
```

  Copy the `FlowRecord` construction from the existing `writer_appends_flow_and_complete_lines` test; the comments above mark where it goes.

- [ ] **Step 2: Run it to verify it fails.**

  Run: `cargo test -p rupu-netflow --lib ledger::writer::`

  Expected: compile error (`with_stream` missing).

- [ ] **Step 3: Implement the tee.**
  - In `paths.rs`: add `pub stream: Option<crate::stream::NetflowStream>` to `NetflowPaths`, and set `stream: None` in `for_run`.
  - Add:

```rust
    /// Tee every ledger line this writer writes into `stream` (spec §N1).
    pub fn with_stream(mut self, stream: Option<crate::stream::NetflowStream>) -> Self {
        self.stream = stream;
        self
    }
```

  - In `writer.rs`, make `write_line` return whether the ledger write succeeded. `run_writer` passes `paths.stream.as_ref()` down, and after a successful ledger write calls:

```rust
async fn tee(stream: Option<&crate::stream::NetflowStream>, line: &LedgerLine) {
    let Some(s) = stream else { return };
    let (path, ledger, line) = (s.path.clone(), s.ledger_id.clone(), line.clone());
    let res = tokio::task::spawn_blocking(move || crate::stream::append_stream_line(&path, &ledger, &line)).await;
    match res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "netflow stream write failed; a coordinator will see this run's flows as partial"),
        Err(e) => tracing::warn!(error = %e, "netflow stream write task failed"),
    }
}
```

  - `flush_dropped` must tee its `Dropped` line through the same path. Make `flush_dropped` call `write_line` and then `tee`.

- [ ] **Step 4: Run it to verify it passes.**

  Run: `cargo test -p rupu-netflow --lib ledger::`

  Expected: PASS, including all the existing writer tests.

- [ ] **Step 5: Add the runtime helpers and widen `for_run`.** In `crates/rupu-runtime/src/netflow.rs`:

```rust
/// The stream a run tees into when `<root_run_dir>/netflow.jsonl` has been
/// begun by a coordinator-launched root (spec §N1); `None` otherwise.
pub fn stream_for(root_run_dir: &Path, ledger_id: &str) -> Option<rupu_netflow::NetflowStream> {
    let path = root_run_dir.join(rupu_netflow::NETFLOW_STREAM_FILE);
    rupu_netflow::stream::stream_has_begin(&path).then(|| rupu_netflow::NetflowStream {
        path,
        ledger_id: ledger_id.to_string(),
    })
}

/// A process child's stream: its root's run directory is the directory of
/// the root's usage ledger its link carries.
pub fn link_stream(link: &crate::argv::LaunchLink, ledger_id: &str) -> Option<rupu_netflow::NetflowStream> {
    let dir = link.usage_root.as_deref()?.parent()?;
    stream_for(dir, ledger_id)
}
```

  Then add `stream: Option<rupu_netflow::NetflowStream>` as `for_run`'s last parameter and build the paths with `NetflowPaths::for_run(&netflow_dir, run_id).with_stream(stream)`. Update the doc comment.

- [ ] **Step 6: Wire the assembler.**
  - In `origin.rs`, add `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum NetflowStreamRoot { None, Own, WorkflowRun, Parent }` and a field `netflow_stream: NetflowStreamRoot` on `OriginDefaults`. Values in `defaults_for`:

    | Origin | `netflow_stream` |
    |---|---|
    | `Standalone` | `Own` |
    | `WorkflowStep` | `WorkflowRun` |
    | `SubAgent` | `Parent` |
    | `SessionTurn` | `None` |
    | `FlowLead` | `None` |
    | `FlowUnit` | `None` |

  - Add `pub netflow_stream: Option<rupu_netflow::NetflowStream>` to `ParentRun`, and set it wherever a `ParentRun` is built (grep `ParentRun {` across the workspace; the launcher in `rupu-launch` builds one for sub-agents from the parent's assembled run, so expose the run's resolved stream on `AssembledRun`/`ToolContext` services the same way `coverage_stream` is exposed).
  - In `mod.rs` before line 573, resolve:

```rust
        let runs = self.ctx.global.join("runs");
        let netflow_stream = match (&spec.link, defaults.netflow_stream, &spec.origin) {
            (Some(link), _, _) => crate::netflow::link_stream(link, &spec.run_id),
            (None, NetflowStreamRoot::Own, _) => crate::netflow::stream_for(&runs.join(&spec.run_id), &spec.run_id),
            (None, NetflowStreamRoot::WorkflowRun, Origin::WorkflowStep { workflow_run_id, .. }) if !workflow_run_id.is_empty() => {
                crate::netflow::stream_for(&runs.join(workflow_run_id), &spec.run_id)
            }
            (None, NetflowStreamRoot::Parent, Origin::SubAgent { parent }) => parent
                .netflow_stream
                .as_ref()
                .map(|s| rupu_netflow::NetflowStream { path: s.path.clone(), ledger_id: spec.run_id.clone() }),
            _ => None,
        };
```

  - Pass `netflow_stream.clone()` to `for_run`, and store it where `coverage_stream` is stored, so children and the snapshot can read it.
  - When `spec.services.netflow` is `Some`, the site built the sink and owns the decision; the assembler still records the resolved stream for its children.
- [ ] **Step 7: Update the `origin_matrix` snapshot.**
  - In `tests/it/assembly.rs`, add `netflow_stream: Option<String>` to `Derived` (after `coverage_stream`), filled from the run's resolved stream via `rel(home.root(), p)`.
  - The origins in `origins(&home)` have no begun stream, so every row is `~`. Then add one test:

```rust
#[tokio::test]
async fn a_begun_root_stream_is_teed_by_steps_and_sub_agents() {
    // home as in origin_matrix; begin <global>/runs/run_wf/netflow.jsonl
    // assemble Origin::WorkflowStep { workflow_run_id: "run_wf", .. } with run id "run_step"
    // assert its resolved stream path == <global>/runs/run_wf/netflow.jsonl, ledger_id == "run_step"
    // assemble Origin::Standalone run "run_solo" with no begun stream → None
}
```

  Write it out in full using the `origins` and `derived` helpers in that file.
  - Run: `cargo insta test -p rupu-runtime --test it -- assembly::`, then `cargo insta accept` after checking the diff is only the new `netflow_stream: ~` column.

- [ ] **Step 8: Wire the CLI roots and the remaining `for_run` callers.**
  - `crates/rupu-cli/src/cmd/run.rs`, after `start_coverage_stream` at l.862:

```rust
    let netflow_stream = if let Some(link) = launch_link.as_ref() {
        rupu_runtime::netflow::link_stream(link, &run_id)
    } else if args.run_id.is_some() {
        let path = rupu_netflow::stream::stream_path(&global.join("runs"), &run_id);
        if let Err(e) = rupu_netflow::stream::write_stream_begin(&path, &run_id) {
            warn!(error = %e, path = %path.display(), "could not start this run's netflow stream; a coordinator will report its flows as not collected");
        }
        rupu_runtime::netflow::stream_for(&global.join("runs").join(&run_id), &run_id)
    } else {
        None
    };
```

    Make sure `launch_link` is computed before this point; it is computed at l.1138 today, so move that computation up or compute `args.launch.link()` here too. Pass `netflow_stream` to the `for_run` at l.897.
  - `cmd/workflow.rs::run_with_outcome` (l.4752): after minting `run_id`, if `run_id_override.is_some()` and the invocation carries no launch link, call `write_stream_begin` on `<global>/runs/<run_id>/netflow.jsonl` with the same warning. Then, at the three `for_run` sites (3763 resume, 4756, 5547), pass:
    - with a link: `link_stream(link, &run_id)`;
    - otherwise: `stream_for(&global.join("runs").join(&run_id), &run_id)`.

    Read how each site gets its `LaunchLink` (the run's recorded `launch.json` on resume).
  - `resume.rs:370`: `stream_for(&global.join("runs").join(run_id), run_id)`.
  - `session.rs` (7763, 8306), `agentiflow.rs:276` and `netflow_sink.rs` tests: pass `None`.
  - `netflow_sink.rs::for_run`: add the same parameter and forward it.

- [ ] **Step 9: Add a CLI test for the begin rule.** In `crates/rupu-cli/tests/serial/` (or `tests/it/` if the existing `rupu run` mock-provider tests live there; follow the pattern of the test that asserts the coverage `begin` line, found by grepping `write_stream_begin` / `"ledger":"begin"` in `crates/rupu-cli/tests`):
  - a `rupu run --run-id run_T1 …` with the mock provider leaves `<RUPU_HOME>/runs/run_T1/netflow.jsonl`, whose first line is `{"kind":"begin",…}`;
  - a `rupu run` without `--run-id` leaves no `netflow.jsonl`.

  Hold the crate's `ENV_LOCK` as the neighbouring tests do.

- [ ] **Step 10: Update the housekeeping comment.** In `cmd/transcript.rs:2388-2394`, extend the doc comment on `stream_only_run_dir`: a stream-only run dir may hold `coverage.jsonl` and/or `netflow.jsonl` (+ locks), and the rule (no `run.json`) already covers both. No code change.

- [ ] **Step 11: Run the tests.**

  Run:
  - `cargo test -p rupu-netflow --lib ledger::`
  - `cargo test -p rupu-runtime --test it assembly::`
  - the CLI test from Step 9
  - `cargo build -p rupu-cli`
  - `cargo clippy -p rupu-netflow -p rupu-runtime -p rupu-cli --all-targets -- -D warnings`

  Expected: all PASS.

- [ ] **Step 12: Commit.** Commit with message `feat(netflow): coordinator-launched roots tee every ledger line into runs/<root>/netflow.jsonl`, plus the co-author line.

### Task 4: SSH transport

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs`
  - constants 1244–1248
  - `ledger_pull_command` 1258–1264
  - `pump_catch_up_ledgers` 1333–1380
  - `FakeExec` 4526–4736
  - the tests at 10265 and 10287, plus new tests

**Interfaces:**
- Consumes: Task 2's `NodeMirror::replace_netflow(run_id, node_id, body, complete, reason)`. Landing then happens in the mirror when `pump_finalize_if_terminal` appends the terminal `RunJson` (1632) or calls `finish` (1639). The stream-end fallback (2648–2714) does the same.
- Produces: nothing new for other tasks.

- [ ] **Step 1: Write the failing tests.**
  - Update `ledger_pull_command_frames_both_ledgers_under_the_remote_run_dir` (10265) to expect `"for f in usage.jsonl coverage.jsonl netflow.jsonl; do printf '==> %s <==\\n' \"$f\"; cat \"$HOME/.rupu/runs/run_01LEDGER/$f\" 2>/dev/null; printf '\\n==> end %s <==\\n' \"$?\"; done"`, and rename it `..._frames_every_ledger_...`.
  - Extend `FakeExec` with `cat_netflow_stdout: Option<String>`, and widen `ledger_cat_status` to `(i32, i32, i32)`. Update all five constructors and the framing array (4721–4736) to emit a third section `(LEDGER_NETFLOW, &self.cat_netflow_stdout, self.ledger_cat_status.2)`. Fix every existing use of `ledger_cat_status` (grep it).
  - Add these tests, modelled on `tail_pump_mirrors_the_coverage_stream_and_replaces_it_at_terminal` (7868) and using `make_conn` and `wait_until_finished`:

```rust
#[tokio::test]
async fn terminal_pull_lands_the_netflow_stream_complete() {
    // FakeExec::ok with a terminal run.json and
    //   cat_netflow_stdout = Some(begin + one tagged line for "run_X" + one for "sub_Y")
    // launch a unit run (as the coverage test does), wait_until_finished
    // assert read_landing_record(&run_store.root, run_id).status == Complete
    //   and ledgers == ["run_X", "sub_Y"], and <global>/netflow/sub_Y.jsonl exists
}

#[tokio::test]
async fn a_netflow_section_that_did_not_arrive_lands_unavailable() {
    // ledger_cat_status.2 = 1 (cat failed: an older host without the file)
    // assert the record is Unavailable with NOT_STREAMED_REASON
}

#[tokio::test]
async fn a_netflow_copy_pulled_before_terminal_lands_partial() {
    // the stream-end fallback path with a non-terminal run.json (copy
    // `the_stream_end_fallback_never_marks_a_non_terminal_runs_coverage_complete`, 8033)
    // then the mirror's finish → record Partial, reason mentions "not terminal"
}
```

  Write each in full, following its comments and the referenced test.

- [ ] **Step 2: Run them to verify they fail.**

  Run: `cargo test -p rupu-cp --lib host::ssh::`

  Expected: the new and updated tests FAIL.

- [ ] **Step 3: Implement.**
  - Add `const LEDGER_NETFLOW: &str = "netflow.jsonl";` and change the prefix to `"for f in usage.jsonl coverage.jsonl netflow.jsonl;"`. `ledger_pull_command` needs no other change; it loops over the prefix's list.
  - In `pump_catch_up_ledgers`, after the coverage block:

```rust
    match sections.get(LEDGER_NETFLOW).filter(|b| !b.is_empty()) {
        Some(netflow) => {
            let reason = (!run_terminal).then(|| "the host's run.json was not terminal when its netflow was pulled".to_string());
            if let Err(e) = mirror.replace_netflow(run_id, host_id, netflow, run_terminal, reason).await {
                tracing::warn!(run_id, host_id, error = %e, "terminal netflow replace failed");
            }
        }
        None => {
            // nothing arrived: leave any tailed copy alone; with no copy
            // the mirror lands `unavailable` at terminal
        }
    }
```

    The netflow stream is not added to the `tail -F` list (spec §N2: SSH pulls it only at the end).
  - Update the `tail_cmd` comment at 2395–2401 to say `netflow.jsonl` is pulled, not tailed.

- [ ] **Step 4: Run them to verify they pass.**

  Run: `cargo test -p rupu-cp --lib host::ssh::`

  Expected: PASS, including every existing ledger and coverage test.

- [ ] **Step 5: Commit.** Commit with message `feat(cp/ssh): pull and land a unit's netflow stream in the terminal ledger pull`.

### Task 5: Tunnel and bucket transports

**Files:**
- Modify: `crates/rupu-cli/src/cmd/node.rs`
  - `FileOffsets` (145–155, 722–728, test `offsets()` 2668)
  - the drain loop (622–689), the terminal sequence (668–686) and the cancel path (742–772)
  - `BucketStreams` (179–231)
  - helpers near `coverage_path` (1637–1671)
  - tests near 4962–5100 and 2956–3200
- Modify: `crates/rupu-cp/src/host/bucket/poller.rs`: `classify_key` (174–196) and its test (492)
- Modify (tests only): `crates/rupu-cp/tests/it/node_tunnel.rs`

**Interfaces:**
- Consumes: Task 2's `ArtifactFile::Netflow` and `CAP_MIRROR_NETFLOW`. The mirror lands at the terminal `RunJson` / `finish`.
- Produces: nothing new for other tasks.

- [ ] **Step 1: Write the failing node tests.** Model them on `drain_coverage_reads_the_run_stream_incrementally` (4962) and the `ship_coverage_*` tests (5040–5100):

```rust
#[tokio::test]
async fn ship_netflow_sends_frames_only_when_the_cp_advertised_it() {
    // run_dir with netflow.jsonl = begin + 2 lines
    // ship_netflow(&mut sink, "run_1", &run_dir, &mut off, false) → no frames, off == 0
    // ship_netflow(..., true) → 3 ArtifactFile::Netflow frames, off == file length
    // append one more line; ship again → exactly 1 new frame
}

#[tokio::test]
async fn bucket_terminal_pass_uploads_netflow_chunks_before_finished() {
    // copy the shape of the bucket_terminal_pass test at 2956 with RecordingBucket;
    // run_dir has netflow.jsonl; assert a result key "netflow.0000.jsonl" was put
    // and that it was put before the finished marker
}
```

- [ ] **Step 2: Run them to verify they fail.**

  Run: `cargo test -p rupu-cli --lib cmd::node::`

  Expected: compile errors (`ship_netflow` missing).

- [ ] **Step 3: Implement the node side.**
  - Add `netflow: u64` to `FileOffsets` (and to every literal of it).
  - Add helpers:

```rust
fn netflow_path(run_dir: &Path) -> PathBuf {
    run_dir.join(rupu_netflow::stream::STREAM_FILE)
}

async fn ship_netflow<S>(sink: &mut S, run_id: &str, run_dir: &Path, offset: &mut u64, mirror_netflow: bool)
where /* same bounds as ship_coverage */
{
    if !mirror_netflow {
        return;
    }
    // identical to ship_coverage, reading netflow_path(run_dir) with drain_new_lines
    // and sending ArtifactFile::Netflow
}
```

    Write the body by copying `ship_coverage`; don't call `ship_coverage` with a different path, because its file kind is fixed.
  - In `connect_and_run_with`, add `let mirror_netflow = welcome_advertises(&welcome_frame, rupu_cp::node::protocol::CAP_MIRROR_NETFLOW);` after `mirror_coverage` (556).
  - Call `ship_netflow` right after every `ship_coverage` call: in the per-drain block (649), in the terminal block (before `RunJson`, 673–680), and in the cancel path (756–763).
- [ ] **Step 4: Implement the bucket worker.** Add `netflow: BucketStream` to `BucketStreams`, and in `upload_all` after coverage (222–229):

```rust
        ok &= upload_new_lines(bucket, rid, "netflow", &netflow_path(run_dir), &mut self.netflow).await;
```

  Use the same `&=` form the coverage line uses. `finish_bucket_run` already holds the run until `upload_all` returns true, so netflow chunks land before `run.json` and `finished`.
- [ ] **Step 5: Implement the poller.** In `poller.rs::classify_key`, add `if key.starts_with("netflow") { return Some(ArtifactFile::Netflow); }` inside the `.jsonl` block, and extend `classify_key_maps_known_suffixes` with `netflow.0001.jsonl` → `Netflow`.
- [ ] **Step 6: Add a tunnel integration test.** In `crates/rupu-cp/tests/it/node_tunnel.rs`, copy its coverage end-to-end test (grep `Coverage` there) and add `netflow_frames_land_when_the_run_finishes`. The node sends `Netflow` frames, then a terminal `RunJson`, then `RunFinished`. Assert the coordinator's `read_landing_record(&run_store.root, run_id)` is `Complete` and the ledger file exists under `<global>/netflow/`.
- [ ] **Step 7: Run the tests.**

  Run:
  - `cargo test -p rupu-cli --lib cmd::node::`
  - `cargo test -p rupu-cp --lib host::bucket::`
  - `cargo test -p rupu-cp --test it node_tunnel::`
  - `cargo clippy -p rupu-cp -p rupu-cli --all-targets -- -D warnings`

  Expected: PASS.

- [ ] **Step 8: Commit.** Commit with message `feat(node,bucket): ship a run's netflow stream to a CP that mirrors it`.

### Task 6: HTTP transport

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs` (route at 31; handler next to `get_run_coverage` at 45)
- Modify: `crates/rupu-cp/src/host/http.rs` (`land_run_netflow`, replacing Task 2's temporary body)
- Modify: `crates/rupu-cp/tests/it/host_http.rs` (next to the coverage tests at 1352–1576)

**Interfaces:**
- Consumes: Task 1's `rupu_netflow::stream::stream_path` and `rupu_netflow::land::land_and_record`; Task 2's `CAP_RUN_NETFLOW_STREAM` and `NetflowLanding`.
- Produces: `GET /api/runs/:id/netflow/stream` (content type `application/x-ndjson`).

- [ ] **Step 1: Write the failing tests.** In `host_http.rs`, following the coverage tests' real-router setup:

```rust
#[tokio::test]
async fn netflow_stream_endpoint_serves_the_runs_stream() {
    // seed <global>/runs/run_H1/netflow.jsonl on the "remote" CP; GET /api/runs/run_H1/netflow/stream
    // → 200, application/x-ndjson, exact bytes; an absent stream → 200 with an empty body;
    // an invalid id → 400
}

#[tokio::test]
async fn land_run_netflow_fetches_and_lands_complete_on_a_terminal_run() {
    // connector against the real router; peer advertises run.netflow_stream
    // conn.land_run_netflow("run_H1", true, &coord_runs_root) → Landed(Complete)
    // coordinator: read_landing_record(&coord_runs_root, "run_H1") Complete; <coord global>/netflow/run_H1.jsonl exists
}

#[tokio::test]
async fn land_run_netflow_on_a_snapshot_lands_partial() {
    // terminal=false → Landed(Partial), reason mentions "before the unit finished"
}

#[tokio::test]
async fn a_peer_without_the_feature_is_recorded_unavailable() {
    // peer whose /api/host/info features lack run.netflow_stream (copy the coverage refusal test)
    // → Landed(Unavailable); the record's reason names the missing feature
}
```

- [ ] **Step 2: Run them to verify they fail.**

  Run: `cargo test -p rupu-cp --test it host_http::`

  Expected: FAIL (404 route; `Unsupported` from Task 2's temporary body).

- [ ] **Step 3: Implement the endpoint.** In `runs.rs`, add `.route("/api/runs/:id/netflow/stream", get(get_run_netflow_stream))` and the handler:

```rust
/// `GET /api/runs/:id/netflow/stream` — the run's netflow stream as this
/// host wrote it (spec 2026-10-14 §N2); empty when the run wrote none.
async fn get_run_netflow_stream(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    if !crate::host::connector::valid_run_id(&id) {
        return Err(ApiError::bad_request(format!("invalid run id {id:?}")));
    }
    let path = rupu_netflow::stream::stream_path(&s.run_store.root, &id);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(ApiError::internal(e.to_string())),
    };
    Ok(([(axum::http::header::CONTENT_TYPE, "application/x-ndjson")], bytes).into_response())
}
```

  Use whatever run-id validator `mirror_unit_coverage` uses (grep `fn valid_run_id` / `validate_run_id`; reuse it, don't write a new one). Make sure the route does not collide with the existing `/api/runs/:id/netflow` route registered in `api/netflow.rs`; axum treats the two as distinct paths.

- [ ] **Step 4: Implement the connector.** In `http.rs`, replace the temporary body:

```rust
    async fn land_run_netflow(
        &self,
        run_id: &str,
        terminal: bool,
        runs_root: &std::path::Path,
    ) -> Result<NetflowLanding, HostConnectorError> {
        if !valid_run_id(run_id) {
            return Err(HostConnectorError::Invalid(format!("invalid run id {run_id:?}")));
        }
        let fetched: Result<Vec<u8>, String> = match self
            .require_feature(CAP_RUN_NETFLOW_STREAM, "this unit's netflow cannot be collected")
            .await
        {
            Err(e) => Err(e.to_string()),
            Ok(()) => /* GET /api/runs/{run_id}/netflow/stream exactly as unit_coverage GETs
                         /coverage; a transport or body error → Err(e.to_string()) */,
        };
        let (runs_root, run_id, host) = (runs_root.to_path_buf(), run_id.to_string(), self.host_id.clone());
        let rec = tokio::task::spawn_blocking(move || match fetched {
            Ok(bytes) => rupu_netflow::land::land_and_record(
                &runs_root, &run_id, &host, Some(&bytes), terminal,
                (!terminal).then(|| "read before the unit finished".to_string()),
            ),
            Err(why) => rupu_netflow::land::land_and_record(&runs_root, &run_id, &host, None, false, Some(why)),
        })
        .await
        .map_err(|e| HostConnectorError::Internal(e.to_string()))?
        .map_err(|e| HostConnectorError::Internal(e.to_string()))?;
        Ok(NetflowLanding::Landed(rec.status))
    }
```

  Fill in the GET by copying `unit_coverage` (830–859). Use the connector's own host-id field name, which may not be `host_id`.

- [ ] **Step 5: Run the tests.**

  Run: `cargo test -p rupu-cp --test it host_http::`, then `cargo clippy -p rupu-cp --all-targets -- -D warnings`

  Expected: PASS.

- [ ] **Step 6: Commit.** Commit with message `feat(cp/http): serve and land a run's netflow stream behind run.netflow_stream`.

### Task 7: Orchestrator roster and dispatcher collection

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs`: the three dispatch sites (`dispatch_placed_step` ~8085, fan-out primary ~9392, fallback retry ~9552)
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs`
  - `FleetUnitDispatcher` gets `runs_root: PathBuf`
  - `build_dispatcher_if_needed` (992) sets it
  - `launch_and_wait` (448): the three post-launch outcome points 602–616, 625–689 and 728–739
  - `FakeConnector` (1202) gets configurable netflow behaviour
- Create: `crates/rupu-orchestrator/tests/it/remote_netflow_roster.rs`; register it in `tests/it/main.rs`

**Interfaces:**
- Consumes: Task 1's `rupu_netflow::land::{append_roster_line, RosterLine, read_landing_record, LandingStatus}`; Task 2's `HostConnector::land_run_netflow` and `NetflowLanding`.
- Produces: the roster file that Task 8 reads, and `StepWarning`s (through `UnitOutcome.warnings` / `UnitFailure.warnings`, which `emit_unit_warnings` already turns into `Event::StepWarning`).

- [ ] **Step 1: Write the failing orchestrator test.** Create `tests/it/remote_netflow_roster.rs`, modelled on `tests/it/remote_coverage_ingest.rs` (its `Scripted` dispatcher at :80):

```rust
// A placed step on "host_a" and a distribute fan-out over ["host_a","host_b"] where unit 0's first
// attempt fails and is retried on the fallback host:
// after the run, read_roster(&store.root, workflow_run_id) lists every dispatched attempt —
// the placed step, each fan-out unit, AND the failed first attempt plus its retry — with
// host, step_id and index; a step with host: local adds no roster line.
```

  Write it in full. The scripted dispatcher's `dispatch_unit` records `unit.run_id`, so the test can compare the roster's `unit_run_id`s against the ids dispatched.

- [ ] **Step 2: Run it to verify it fails.**

  Run: `cargo test -p rupu-orchestrator --test it remote_netflow_roster::`

  Expected: FAIL (the roster is empty).

- [ ] **Step 3: Implement the roster.** In `runner.rs`, add the helper next to `record_attempt` (7747):

```rust
/// Name a placed unit attempt in the workflow run's netflow roster before
/// it is dispatched (spec 2026-10-14 §N3), so its flows — or the gap where
/// they should be — are found even if this process dies before the unit
/// ends. A local unit's ledgers are already here; an in-memory run has no
/// store. Best-effort: a failure is logged.
fn record_netflow_roster(
    store: Option<&RunStore>,
    workflow_run_id: &str,
    unit_run_id: &str,
    host: &str,
    step_id: &str,
    index: Option<usize>,
) {
    let Some(store) = store.filter(|_| !workflow_run_id.is_empty()) else { return };
    if host == "local" {
        return;
    }
    let line = rupu_netflow::land::RosterLine {
        unit_run_id: unit_run_id.to_string(),
        host: host.to_string(),
        step_id: step_id.to_string(),
        index,
    };
    if let Err(e) = rupu_netflow::land::append_roster_line(&store.root, workflow_run_id, &line) {
        warn!(step = step_id, host, error = %e, "could not record the unit in the netflow roster");
    }
}
```

  Call it immediately before each of the three `dispatch_unit_unless_terminating` calls:
  - at 8085 with `(opts.run_store.as_deref(), workflow_run_id, run_id, host, &step.id, None)`;
  - at 9392 with `(store_for_task.as_deref(), &workflow_run_id, &run_id, &host, &step_id, Some(idx))`;
  - at 9552 with `(store_for_task.as_deref(), &workflow_run_id, &retry_run_id, retry_host, &step_id, Some(idx))`.

  Use the variable names actually in scope at each site.

- [ ] **Step 4: Write the failing dispatcher tests.** In `fleet_unit_dispatcher.rs` tests, give `FakeConnector` a field `netflow: Option<NetflowLanding>`, where `None` means `Ok(NotNeeded)`. Add `netflow_record: Option<LandingRecord>`, which the fake's `land_run_netflow` writes with `land_and_record` before answering `ByMirror`, simulating the mirror. Add `netflow_fails: bool` → `Err(Unreachable)`. Then:

```rust
#[tokio::test]
async fn a_complete_landing_adds_no_warning() { /* ByMirror + Complete record → outcome.warnings has no netflow line */ }

#[tokio::test]
async fn an_unavailable_landing_is_a_step_warning() {
    // ByMirror + an Unavailable record → warnings contains "netflow from this unit was not collected: <reason>"
}

#[tokio::test]
async fn a_mirrored_unit_with_no_record_yet_is_a_warning() {
    // ByMirror, no record → "netflow from this unit was not collected yet"
}

#[tokio::test]
async fn a_poll_timeout_asks_for_a_snapshot_landing() {
    // the never-terminal connector path (StageOkPollNeverTerminalConnector) → land_run_netflow called with terminal=false
}

#[tokio::test]
async fn a_local_unit_never_warns() { /* NotNeeded → no netflow warning */ }
```

- [ ] **Step 5: Implement the dispatcher.**
  - Add `runs_root: PathBuf` to `FleetUnitDispatcher`. Set it in `build_dispatcher_if_needed` from the coordinator `RunStore` it already builds (`run_store.root.clone()`), and in every test constructor (a tempdir).
  - Add:

```rust
/// Collect a unit's netflow (spec 2026-10-14 §N3) and say what is missing:
/// `None` when nothing needs saying. `prefix_host` adds the host later.
async fn collect_netflow(
    conn: &Arc<dyn HostConnector>,
    run_id: &str,
    terminal: bool,
    runs_root: &Path,
) -> Option<String> {
    use rupu_cp::host::connector::NetflowLanding;
    use rupu_netflow::land::{read_landing_record, LandingStatus};
    let landing = match conn.land_run_netflow(run_id, terminal, runs_root).await {
        Ok(l) => l,
        Err(e) => return Some(format!("netflow from this unit was not collected: {e}")),
    };
    if landing == NetflowLanding::NotNeeded {
        return None;
    }
    let (root, id) = (runs_root.to_path_buf(), run_id.to_string());
    let record = tokio::task::spawn_blocking(move || read_landing_record(&root, &id)).await.ok().flatten();
    match record {
        None => Some(if terminal {
            "netflow from this unit was not collected yet".to_string()
        } else {
            "netflow from this unit was not collected yet: the unit had not finished when the coordinator stopped waiting".to_string()
        }),
        Some(r) => match r.status {
            LandingStatus::Complete => None,
            LandingStatus::Partial => Some(format!(
                "netflow from this unit may be incomplete: {}",
                r.reason.unwrap_or_default()
            )),
            LandingStatus::Unavailable => Some(format!(
                "netflow from this unit was not collected: {}",
                r.reason.unwrap_or_default()
            )),
        },
    }
}
```

  - Call it at each post-launch outcome in `launch_and_wait`, after `collect_coverage`:
    - poll error (602–616): `terminal = false`;
    - terminal (642): `terminal = true`, after `await_run_mirror`;
    - timeout (728–737): `terminal = false`, but only when the run was observed, as with coverage.

    Push `Some(msg)` into the outcome's or failure's `warnings` vec; they are built with `Vec::new()` today, so build them with `collect_netflow(...).await.into_iter().collect()`.
  - `prefix_host` already prefixes every warning with `host <id>: `.

- [ ] **Step 6: Run the tests.**

  Run:
  - `cargo test -p rupu-orchestrator --test it remote_netflow_roster:: remote_coverage_ingest:: placed_step_e2e:: distributed_fanout_e2e::`
  - `cargo test -p rupu-cli --lib fleet_unit_dispatcher::`
  - `cargo clippy -p rupu-orchestrator -p rupu-cli --all-targets -- -D warnings`

  Expected: PASS.

- [ ] **Step 7: Commit.** Commit with message `feat(orchestrator): netflow roster of placed unit attempts; dispatcher collects each unit's netflow`.

### Task 8: CP read side

**Files:**
- Modify: `crates/rupu-cp/src/api/netflow.rs`
  - a new `netflow_ids` next to `run_and_unit_ids` (710)
  - `RunMetaIndex` (755–800)
  - `store_fingerprint` (966)
  - `scope_gap_from_workers` (1765)
  - `run_scoped_flows_and_dropped_with` (1232)
  - `collect_run_netflow` (1276–1320)
  - `agentiflow_netflow_scope` (1350) and its struct (1335)
  - `collect_agentiflow_netflow` (1408), `build_agentiflow_explorer` (1439)
  - `explorer_run_scope` (2367–2470)
  - `get_run_netflow` (2950–3010)
- Modify: `crates/rupu-cli/src/cmd/netflow.rs:635` (prune protection)

**Interfaces:**
- Consumes: Task 1's `rupu_netflow::land::{read_landing_record, read_roster, LandingStatus}`.
- Produces: `pub struct NetflowIds { pub ids: Vec<String>, pub gaps: Vec<IncompleteSource> }` and `pub fn netflow_ids(store: &RunStore, global_dir: &Path, run_id: &str) -> NetflowIds`. Keep `run_and_unit_ids` as the inner walk.

- [ ] **Step 1: Write the failing tests.** In `netflow.rs`'s test module, using `seed_record`, `placed_run`, `ledger`, `state_with_host` and the step-result pattern at 3789–3814:

```rust
#[test]
fn landed_unit_ledgers_join_the_run_and_its_gaps_are_listed() {
    // global store; run "run_wf" (worker_id None) with a step_results record whose items carry
    //   run_id "run_u1" (copy the StepResultRecord literal, put one ItemResultRecord in `items`)
    // roster for run_wf: run_u1 (host_a), run_u0 (host_a, a failed first attempt), run_u2 (host_b, never landed)
    // landing records (land_and_record): run_u1 Complete with ledgers [run_u1, sub_k];
    //   run_u0 Partial (reason "snapshot") with [run_u0]
    // netflow_ids(&store, global, "run_wf"):
    //   ids ⊇ {run_wf, run_u1, sub_k, run_u0, run_u2}
    //   gaps == [{host_a, "...snapshot"}, {host_b, "...not collected yet"}] (order-insensitive)
}

#[test]
fn a_wholly_placed_run_with_a_complete_landing_needs_no_live_fetch() {
    // placed_run("run_p", "host_ssh", ws) + a Complete landing for run_p with ledgers [run_p, run_step]
    // run_landed_complete(global, "run_p") == true
    // RunMetaIndex::insert_run → remote_workers is empty, map contains run_step
}

#[tokio::test]
async fn run_scope_reads_landed_ledgers_and_reports_gaps() {
    // state_with_host with a StubConnector whose run_netflow panics if called (None → Unsupported is fine too);
    // ledger(global/netflow, "sub_k", [flow A], 0) as the landed copy; GET run netflow for run_wf
    // → flows include A; incomplete lists host_b's gap
}

#[test]
fn project_and_global_scope_attribute_landed_ledgers_and_list_gaps() {
    // project_run_meta / global_run_meta over the same fixture → attribution(sub_k) == (run_wf, wf name);
    // the scope's incomplete (scope_gap_from_workers(&meta)) includes host_b's gap
}

#[test]
fn a_new_landing_invalidates_the_run_meta_cache() {
    // build the global index via global_run_meta_cached; then land a record for a unit of an
    // existing run; the fingerprint changes and the rebuilt index attributes the new ledger
}

#[test]
fn agentiflow_scope_lists_landing_gaps() {
    // an agentiflow dir whose flow_runs include run_wf → the agentiflow response's incomplete has host_b's gap
}
```

  Write each in full. Each test's comments are its specification.

- [ ] **Step 2: Run them to verify they fail.**

  Run: `cargo test -p rupu-cp --lib api::netflow::`

  Expected: compile errors (`netflow_ids` missing).

- [ ] **Step 3: Implement `netflow_ids`.**

```rust
/// Every ledger id a run's netflow reads, plus what is known to be missing
/// (spec 2026-10-14 §N4): [`run_and_unit_ids`], the workflow run's roster of
/// placed unit attempts, and each id's landing record — whose `ledgers` add
/// a remote unit's own sub-agent and child ledgers, and whose status (or
/// absence, for a roster unit) becomes an [`IncompleteSource`].
pub fn netflow_ids(store: &RunStore, global_dir: &StdPath, run_id: &str) -> NetflowIds {
    use rupu_netflow::land::{read_landing_record, read_roster, LandingStatus};
    let runs = global_dir.join("runs");
    let mut ids = run_and_unit_ids(store, run_id);
    let roster = read_roster(&store.root, run_id);
    let roster_hosts: HashMap<String, String> =
        roster.iter().map(|l| (l.unit_run_id.clone(), l.host.clone())).collect();
    ids.extend(roster.into_iter().map(|l| l.unit_run_id));
    let mut gaps = Vec::new();
    let mut landed = Vec::new();
    // Only a roster unit or the run itself (a wholly placed run) can carry a
    // landing record — never read one per step id: every open counts at
    // project/global scope (CP load-time rule).
    let candidates: Vec<String> = std::iter::once(run_id.to_string())
        .chain(roster_hosts.keys().cloned())
        .collect();
    for id in &candidates {
        match read_landing_record(&runs, id) {
            Some(r) => {
                landed.extend(r.ledgers.iter().cloned());
                if r.status != LandingStatus::Complete {
                    gaps.push(IncompleteSource {
                        host_id: r.host.clone(),
                        reason: format!(
                            "netflow from unit run {id} {}: {}",
                            if r.status == LandingStatus::Partial { "may be incomplete" } else { "was not collected" },
                            r.reason.unwrap_or_default()
                        ),
                    });
                }
            }
            None => {
                if let Some(host) = roster_hosts.get(id) {
                    gaps.push(IncompleteSource {
                        host_id: host.clone(),
                        reason: format!("netflow from unit run {id} was not collected yet"),
                    });
                }
            }
        }
    }
    ids.extend(landed);
    ids.retain(|i| !i.is_empty());
    ids.sort();
    ids.dedup();
    NetflowIds { ids, gaps }
}

/// True when a run's own landing record is complete — a wholly placed run
/// whose ledgers are all local now and needs no live fetch.
pub fn run_landed_complete(global_dir: &StdPath, run_id: &str) -> bool {
    rupu_netflow::land::read_landing_record(&global_dir.join("runs"), run_id)
        .is_some_and(|r| r.status == rupu_netflow::land::LandingStatus::Complete)
}
```

- [ ] **Step 4: Switch every caller.** Do not leave any id walk on `run_and_unit_ids` except inside `netflow_ids`.
  - `RunMetaIndex`:
    - give it `global_dir: PathBuf` (set by the constructors `project_run_meta` / `global_run_meta`; `Default` is still used by tests, so add `fn with_global(global_dir)`) and `landing_gaps: Vec<IncompleteSource>`;
    - `insert_run` uses `netflow_ids(store, &self.global_dir, &record.id)`, extends `landing_gaps` with its gaps, and inserts into `remote_workers` only when `!run_landed_complete(&self.global_dir, &record.id)`.
  - `scope_gap_from_workers` returns the worker gaps followed by `meta.landing_gaps`, deduplicated by `(host_id, reason)`. Its four callers then pick up landing gaps unchanged.
  - `run_scoped_flows_and_dropped_with` and `collect_run_netflow`'s capture loop use `netflow_ids(store, global_dir, run_id).ids`. `collect_run_netflow` appends the gaps to `incomplete` after the remote ones.
  - In `get_run_netflow` (2967, 2994) and `explorer_run_scope` (2382, 2424), replace the `worker_host_flows(&s, &run).await` call with:

```rust
if run_landed_complete(&s.global_dir, &run.id) {
    (Vec::new(), 0, Vec::new())
} else {
    worker_host_flows(&s, &run).await
}
```

    `run_landed_complete` reads a file, so call it inside the existing `run_blocking` or wrap it in `tokio::task::spawn_blocking`. Never read a file on the async runtime (see the FIFO tests' rule in that file).
  - `agentiflow_netflow_scope` builds `ids` and `gaps` through `netflow_ids`. Add `gaps: Vec<IncompleteSource>` to `AgentiflowNetflowScope`, and set `resp.incomplete = scope.gaps.clone()` in `collect_agentiflow_netflow` and `build_agentiflow_explorer`.
  - `store_fingerprint` (966): add `mtime(netflow.units.jsonl)` and `mtime(netflow.landing.json)` to each run directory's stamp, so a new roster line or landing changes the fingerprint.
  - In `crates/rupu-cli/src/cmd/netflow.rs:635`, use `rupu_cp::api::netflow::netflow_ids(store, &global, &rec.id).ids`. Read how `global` is available there.

- [ ] **Step 5: Run the tests.**

  Run: `cargo test -p rupu-cp --lib api::netflow::`, `cargo test -p rupu-cli --lib cmd::netflow::`, then `cargo clippy -p rupu-cp -p rupu-cli --all-targets -- -D warnings`

  Expected: PASS, including every existing netflow test. The worker-gap texts and tests must be unchanged.

- [ ] **Step 6: Commit.** Commit with message `feat(cp): netflow scopes read landed remote ledgers and list collection gaps`.

### Task 9: Web disclosure

**Files:**
- Modify: `crates/rupu-cp/web/src/components/netflow/ScopeDisclosure.tsx`
- Modify: its test (find it with `grep -rl "AGENTIFLOW_SCOPE_NOTE\|SUBPROCESS_CAPTURE_LIMITS\|ScopeDisclosure" crates/rupu-cp/web/src --include=*.test.*`)

**Interfaces:** none.

- [ ] **Step 1: Write the failing tests.** In the test file:
  - assert `SUBPROCESS_CAPTURE_LIMITS` contains `a remote host's bash flows arrive when its unit finishes` and does not contain `stay on that host`;
  - assert that no text at any scope (`disclosureText` for `'run' | 'project' | 'global' | 'agentiflow'`, and the agentiflow note) contains `keeps its flows on that host` or `so they are not included here`;
  - assert the agentiflow text contains `appears once it finishes`.
- [ ] **Step 2: Run them to verify they fail.**

  Run: `cd crates/rupu-cp/web && npx vitest run src/components/netflow`

  Expected: FAIL.
- [ ] **Step 3: Implement.**
  - `SUBPROCESS_CAPTURE_LIMITS`: replace `and a remote host's bash flows stay on that host.` with `and a remote host's bash flows arrive when its unit finishes.`
  - `AGENTIFLOW_SCOPE_NOTE`: replace the last sentence with `A step of a workflow it launched that runs on a remote host (host: or distribute:) appears once it finishes; one whose flows could not be collected is listed as incomplete.`
  - Grep the file for any other sentence saying placed or remote steps are missing, and reword it the same way.
- [ ] **Step 4: Run them to verify they pass.**

  Run: `npx vitest run src/components/netflow && npx tsc --noEmit -p .`

  Expected: PASS.
- [ ] **Step 5: Commit.** Commit with message `docs(cp/web): netflow disclosure no longer says remote-placed steps are missing`.

---

## Phase 2

### Task 10: Integration, end to end, docs

**Files:**
- Create: `crates/rupu-cli/tests/it/remote_netflow_e2e.rs` (register it in `tests/it/main.rs`)
- Modify: `CLAUDE.md` (the `rupu-cp` and `rupu-orchestrator` crate entries, one sentence each, plus a "Read first" line)
- Modify: `docs/superpowers/specs/2026-10-14-rupu-remote-netflow-transport-design.md` (an "As built" section only where the build differs)

- [ ] **Step 1: Merge.** Merge each Phase 1 branch into the integration branch in Task order 3→9. Resolve conflicts:
  - `fleet_unit_dispatcher.rs` is touched by Tasks 2 and 7 only, and Task 2 is already in the base.
  - `rupu-cp` `Cargo.lock` churn: take either side.

  Then rebase on fresh `origin/main` (`git fetch origin main:refs/remotes/origin/main`).
- [ ] **Step 2: Write the end-to-end test.** In `remote_netflow_e2e.rs`:
  - Build a `FleetUnitDispatcher` with `Resolver::Fixed` over a fake connector. Its `launch_agent` and `get_run` return a terminal run. Its `land_run_netflow` calls `land_and_record` with a scripted stream carrying ledgers `run_<unit>` (one flow `A`) and `sub_k` (one flow `B`), then answers `ByMirror`.
  - Run a one-step `host: host_a` workflow through `WorkflowRuntime` against a temp `RUPU_HOME`.
  - Assert:
    - `rupu_cp::api::netflow::run_scoped_flows_and_dropped(&reader, &store, run_id, ws, global, &TimeRange::unbounded())` contains A and B;
    - `netflow_ids(...).gaps` is empty.
  - Run a second unit whose stream lands `Partial`, and assert one `StepWarning` and one gap.
  - Follow `crates/rupu-cli/tests/it` neighbours for the workflow-runtime and fake-connector scaffolding. `remote_coverage` e2e tests there are the closest: grep `land` / `unit_coverage`.
- [ ] **Step 3: Run the touched suites.**
  - Every command listed in Tasks 1–9's run steps.
  - The e2e test: `cargo test -p rupu-cli --test it remote_netflow_e2e::`.
  - `cargo clippy --workspace --all-targets -- -D warnings`. This is the one workspace-wide command, and it is a check, not a test run.
- [ ] **Step 4: Update the docs.**
  - `CLAUDE.md`, `rupu-cp` entry: one sentence covering `ArtifactFile::Netflow` / `mirror.netflow` / `run.netflow_stream`, `HostConnector::land_run_netflow`, and the `NodeMirror` landing at terminal.
  - `CLAUDE.md`, `rupu-orchestrator` entry: one sentence on the netflow roster.
  - A "Read first" line for this spec and plan.
  - The spec's "As built" section: only the deviations.
- [ ] **Step 5: Verify against real data.** Done by the controller, not a sub-agent.
  - Install the branch build on the coordinator and on one SSH host.
  - Run a local workflow with a `host:` step.
  - Check, using one batched ssh invocation for any host-side probe:
    - the host's `runs/<unit>/netflow.jsonl` begins with `begin`;
    - the coordinator has `runs/<unit>/netflow.landing.json` `complete` and `<global>/netflow/<unit>.jsonl`;
    - the CP's run Network view shows the unit's flows with no `incomplete` entry.
- [ ] **Step 6: Commit.**

```bash
git add -A
git -c commit.gpgsign=false commit -m "test+docs: remote netflow end to end; CLAUDE.md

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
