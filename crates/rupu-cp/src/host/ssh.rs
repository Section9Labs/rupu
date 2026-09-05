//! SSH transport: dispatch/observe/control runs on a host reachable over `ssh`.
//!
//! Auth is delegated entirely to the system `ssh` (ssh-agent / `~/.ssh/config`
//! / default keys); rupu stores no key material. Every remote argument is
//! shell-escaped before being joined into the remote command, because `ssh`
//! re-parses remote args through the remote login shell.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::stream::Stream;
use futures_util::StreamExt as _;
use rupu_orchestrator::runs::RunStore;
use ulid::Ulid;

use crate::{
    agent_launcher::AgentLaunchRequest,
    host::connector::{
        mirror_stream_run_events, read_transcript_file, EventByteStream, FeedGuard,
        HostCapabilities, HostConnector, HostConnectorError, HostInfo, RunKind, RunListQuery,
        RunStartEvidence, MAX_WORKSPACE_BYTES,
    },
    launcher::LaunchRequest,
    node::{
        protocol::{ArtifactFile, RunSpec, RunSpecKind},
        NodeMirror,
    },
    session_sender::SendMessageRequest,
    session_starter::SessionStartRequest,
};

// ── Pure builder functions ────────────────────────────────────────────────────

/// POSIX single-quote escaping: wrap in single quotes, replacing each embedded
/// `'` with `'\''`.
pub(crate) fn shell_escape(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Join an argv into a single shell command string with each token escaped.
pub(crate) fn build_remote_command(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shell_escape(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `id` is shaped like a locally-minted rupu run id (`run_<ULID>`):
/// non-empty and `[A-Za-z0-9_]` only.
///
/// Every remote command that interpolates a run id UNQUOTED into a
/// `$HOME/.rupu/...` path — the tail pump's `tail -F` / `cat` commands, the
/// launch wrapper's stderr redirect, `launch_diagnostics`' `cat` — depends on
/// this alphabet: it contains no shell metacharacters, so the concatenation
/// cannot be broken out of, and the paths can't be single-quoted because
/// `$HOME` has to expand on the remote shell. Ids minted here satisfy it by
/// construction; ids that arrive from outside (the CP HTTP API, a dispatcher
/// poll) MUST be checked with this before any such interpolation.
pub(crate) fn is_safe_run_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The run directory a launched run will populate on the executing host.
/// Unquoted `$HOME`: must expand remotely. Callers guarantee
/// [`is_safe_run_id`].
fn remote_run_dir(run_id: &str) -> String {
    format!("$HOME/.rupu/runs/{run_id}")
}

/// Where a detached launch's stderr is captured on the executing host —
/// `$HOME/.rupu/runs/<run_id>/launch.log`, INSIDE the run directory so it
/// shares the run's lifecycle: `rupu run delete` / `RunStore::delete` remove
/// the whole directory, so the logs never accumulate outside their runs.
/// Callers guarantee [`is_safe_run_id`].
fn remote_launch_log(run_id: &str) -> String {
    format!("{}/launch.log", remote_run_dir(run_id))
}

/// Token the run-start evidence probe prints when the host DOES show the run
/// started, and the one it prints when it does not. The probe always exits 0
/// and answers on stdout so a transport failure (ssh exit 255, no output) is
/// distinguishable from a real "no trace" answer — testing the exit code
/// alone conflates the two, and blaming a launch because the network blinked
/// is exactly the false positive this whole probe exists to remove.
pub(crate) const RUN_START_EVIDENCE_YES: &str = "rupu_run_started";
pub(crate) const RUN_START_EVIDENCE_NO: &str = "rupu_run_no_trace";

/// The remote shell command that answers "did this run's process actually
/// start on this host?".
///
/// Four independent pieces of evidence, OR-ed, cheapest first:
///
/// 1. **A non-empty transcript.** `run_agent` opens
///    `$HOME/.rupu/transcripts/<run_id>.jsonl` and writes `RunStart` into it
///    as its very first act, so this is true within seconds of the remote
///    `rupu` starting — and stays true. This is the signal that makes the
///    probe useful at all: `run.json`, the thing `get_run` reads, is not
///    written until the agent FINISHES.
/// 2. **A non-empty `run.json` / `events.jsonl` / `step_results.jsonl`** in
///    the run directory — a workflow run's equivalent early artifacts.
/// 3. **A live process carrying this run's id.** Both launch argv builders
///    pass `--run-id <run_id>`, so this answers the question directly and
///    without depending on where the remote `rupu` decided to put its
///    transcript (a staged workspace containing its own `.rupu/` moves it).
///
/// Deliberately NOT evidence: the run DIRECTORY, and `launch.log`. The
/// detached launch wrapper (`detach_launch`) creates both itself, BEFORE the
/// remote `rupu` runs — they exist just as much for a launch that died
/// instantly, so keying on them would make the probe answer "started" for
/// exactly the failure it must still catch.
///
/// The `pgrep` pattern is written `[-]-run-id` on purpose: the shell running
/// this command has the pattern in its OWN `/proc/<pid>/cmdline`, and a
/// literal `--run-id <id>` there would make the probe match itself and report
/// every dead run as alive. The character class breaks the literal match
/// while matching the same target text — the classic `grep [p]attern` guard.
///
/// `$HOME` is interpolated unquoted so it expands on the remote shell, which
/// is why `run_id` must satisfy [`is_safe_run_id`] — callers check it.
pub(crate) fn remote_start_evidence_cmd(run_id: &str) -> String {
    let dir = remote_run_dir(run_id);
    format!(
        "if [ -s $HOME/.rupu/transcripts/{run_id}.jsonl ] \
|| [ -s {dir}/run.json ] || [ -s {dir}/events.jsonl ] \
|| [ -s {dir}/step_results.jsonl ] \
|| pgrep -f -- '[-]-run-id {run_id}' >/dev/null 2>&1; \
then echo {RUN_START_EVIDENCE_YES}; else echo {RUN_START_EVIDENCE_NO}; fi"
    )
}

/// Run [`remote_start_evidence_cmd`] and classify the answer.
///
/// Shared by [`HostConnector::run_start_evidence`] and the tail pump so the
/// coordinator's poll loop and the mirror can never disagree about whether a
/// run started. Anything that is not one of the two expected tokens — a
/// failed ssh, an empty answer, a host whose shell printed something else —
/// is [`RunStartEvidence::Unknown`], never `NoTrace`.
pub(crate) async fn probe_run_start_evidence(
    exec: &dyn RemoteExec,
    run_id: &str,
) -> RunStartEvidence {
    if !is_safe_run_id(run_id) {
        return RunStartEvidence::Unknown;
    }
    let Ok(out) = exec.run(&remote_start_evidence_cmd(run_id)).await else {
        return RunStartEvidence::Unknown;
    };
    match out.stdout.trim() {
        RUN_START_EVIDENCE_YES => RunStartEvidence::Started,
        RUN_START_EVIDENCE_NO => RunStartEvidence::NoTrace,
        _ => RunStartEvidence::Unknown,
    }
}

/// Upper bound on the launch-log excerpt `launch_diagnostics` returns. The
/// log is the process's whole stderr; the diagnosis of a dead launch is in
/// its last lines (the `Error: …` a failing `rupu run` prints on exit), so
/// the excerpt keeps the TAIL.
pub(crate) const LAUNCH_DIAGNOSTICS_MAX_CHARS: usize = 4096;

/// The last `max_chars` chars of `s` (on a char boundary), prefixed with an
/// ellipsis when anything was cut.
pub(crate) fn tail_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    let skip = total - max_chars;
    let start = s
        .char_indices()
        .nth(skip)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("…{}", &s[start..])
}

/// Boundary check on the working dir `rupu __workspace stage` printed.
///
/// The value is REMOTE OUTPUT — a line of stdout from the executing host —
/// and it goes on to be interpolated into three more remote commands (the
/// launch's `cd`, `__workspace collect`, `__workspace discard`). Each of
/// those shell-escapes it, so this is defence in depth rather than the only
/// line: a legitimate stage prints exactly one absolute path, and anything
/// else (an embedded newline, a control character, a relative path) means a
/// garbled transport or a host that is not running the helper it claims to,
/// and is refused HERE so the value never reaches a point of use at all.
fn validate_staged_working_dir(dir: &str) -> Result<(), HostConnectorError> {
    if !dir.starts_with('/') {
        return Err(HostConnectorError::Invalid(format!(
            "remote stage returned a non-absolute working dir: {dir:?}"
        )));
    }
    if dir.chars().any(char::is_control) {
        return Err(HostConnectorError::Invalid(format!(
            "remote stage returned a working dir containing control characters: {dir:?}"
        )));
    }
    Ok(())
}

/// Connect timeout (seconds) for the short request/response ssh calls —
/// `RemoteExec::run` and `RemoteExec::run_bytes`, which back
/// `remote_json_rows` / `remote_json_item` / `remote_workflow` / `info` /
/// `dashboard_summary`. Bounds a dead host to ~3s instead of stalling the
/// dashboard fan-out — the SSH analogue of the HTTP connector's 5s/30s bound.
pub(crate) const SHORT_CALL_CONNECT_TIMEOUT_SECS: u32 = 3;

/// Connect timeout (seconds) for the launch-path pump's long-lived `tail -F`
/// ssh (`RemoteExec::spawn_lines`, used only by `spawn_tail_pump`). That is a
/// streaming connection meant to stay open for the run's duration, not a
/// probe — it keeps the original, more generous bound rather than the
/// tightened short-call one.
pub(crate) const PUMP_CONNECT_TIMEOUT_SECS: u32 = 10;

/// Build the args (after the `ssh` program) to run `remote_command` on `host`.
///
/// Flags emitted:
/// - `-o BatchMode=yes`  — fail fast on missing key rather than prompting
/// - `-o ConnectTimeout=<connect_timeout_secs>` — don't hang indefinitely on
///   unreachable hosts. Callers pass [`SHORT_CALL_CONNECT_TIMEOUT_SECS`] for
///   one-shot request/response calls and [`PUMP_CONNECT_TIMEOUT_SECS`] for the
///   long-lived tail pump — see those constants' docs.
/// - `-i <identity_file>` — if provided
/// - `-p <port>` — if provided
/// - `<host>` — always present
/// - `<remote_command>` — always last
pub(crate) fn ssh_argv(
    host: &str,
    port: Option<u16>,
    identity_file: Option<&Path>,
    remote_command: &str,
    connect_timeout_secs: u32,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={connect_timeout_secs}"),
    ];
    if let Some(id) = identity_file {
        argv.push("-i".to_string());
        argv.push(id.to_string_lossy().into_owned());
    }
    if let Some(p) = port {
        argv.push("-p".to_string());
        argv.push(p.to_string());
    }
    argv.push(host.to_string());
    argv.push(remote_command.to_string());
    argv
}

/// If `line` is a `tail` file-header (`==> <path> <==`), return the path.
pub(crate) fn parse_tail_marker(line: &str) -> Option<&str> {
    let t = line.trim();
    let inner = t.strip_prefix("==> ")?.strip_suffix(" <==")?;
    if inner.is_empty() {
        None
    } else {
        Some(inner)
    }
}

/// Sentinel the remote prints instead of a body when the file is absent, so
/// "no such file" (a complete, empty answer once the run is terminal) is
/// distinguishable from "ssh never reached the host" (no stdout at all).
pub(crate) const NO_FILE_SENTINEL: &str = "__RUPU_NO_FILE__";

/// One-shot read of a remote transcript. Single-quoted path; stderr dropped.
pub(crate) fn single_cat_command(remote: &str) -> String {
    format!(
        "cat {} 2>/dev/null || printf '{NO_FILE_SENTINEL}\\n'",
        shell_escape(remote)
    )
}

fn cache_io_err(e: std::io::Error) -> HostConnectorError {
    HostConnectorError::Invalid(format!("transcript cache io: {e}"))
}

/// Write `body` to `cache` atomically (tmp + rename), then the `.complete`
/// sidecar when `complete` (spec §6.1 step 4).
///
/// The tmp name carries a fresh ULID. A fixed `{cache}.tmp` is shared by every
/// concurrent pull of the same path — and pulls are PER REQUEST, so two
/// viewers opening the same transcript are enough: both `write` the same tmp
/// (truncating each other), and the spliced result gets renamed in — marked
/// `.complete`, and so authoritative forever, when the pull was terminal.
///
/// Do not use this while a lazy tail is feeding `cache`: `rename` swaps the
/// dentry, leaving the feed appending to an unlinked inode. Use
/// [`write_cache_file_inplace`] there instead.
pub(crate) fn write_cache_file(
    cache: &Path,
    body: &str,
    complete: bool,
) -> Result<(), HostConnectorError> {
    let io = cache_io_err;
    if let Some(dir) = cache.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = PathBuf::from(format!("{}.{}.tmp", cache.display(), Ulid::new()));
    std::fs::write(&tmp, body).map_err(io)?;
    std::fs::rename(&tmp, cache).map_err(io)?;
    if complete {
        std::fs::write(crate::host::transcript_paths::complete_marker(cache), b"").map_err(io)?;
    }
    Ok(())
}

/// Same result as [`write_cache_file`], but rewrites the file IN PLACE
/// (`O_TRUNC` on the existing inode) instead of renaming a new one over it.
///
/// Used only when a lazy tail is live on `cache`. The feed holds an append
/// handle on that inode; a `rename` would leave it writing into an unlinked
/// file while every reader opens the new one, so the SSE stream goes
/// permanently quiet — and `alive()` stays true, so later subscribers join the
/// same zombie feed. Truncating the same inode keeps the feed's fd valid: it
/// is in append mode, so its next write lands after the authoritative body,
/// and what it delivers is the same `tail -n +1` replay of the same bytes.
///
/// Not atomic — a reader can observe a partially written file — which is why
/// the non-feed path still prefers the rename.
pub(crate) fn write_cache_file_inplace(
    cache: &Path,
    body: &str,
    complete: bool,
) -> Result<(), HostConnectorError> {
    use std::io::Write as _;
    let io = cache_io_err;
    if let Some(dir) = cache.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(cache)
        .map_err(io)?;
    f.write_all(body.as_bytes()).map_err(io)?;
    f.flush().map_err(io)?;
    if complete {
        std::fs::write(crate::host::transcript_paths::complete_marker(cache), b"").map_err(io)?;
    }
    Ok(())
}

// ── Remote-CLI → CP wire-row reshaping ─────────────────────────────────────────
//
// SSH hosts can't serve the CP HTTP API, so list views are sourced by shelling
// `rupu` over ssh and reshaping the CLI's report rows into the CP wire shapes
// the web UI expects. The mappings are lossy where the CLI omits a field
// (per-run cost/turns/duration, cycle ran/skipped/failed counts) — those render
// blank rather than wrong.

/// A zero `UsageSummary` JSON object with `total_tokens` set to `total`.
fn usage_json(total: u64, runs: u64) -> serde_json::Value {
    serde_json::json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "cached_tokens": 0,
        "total_tokens": total,
        "cost_usd": serde_json::Value::Null,
        "priced": false,
        "runs": runs,
    })
}

/// `"-"` / `""` / missing → JSON null; otherwise the string value.
fn dash_or_null(row: &serde_json::Value, key: &str) -> serde_json::Value {
    match row.get(key).and_then(|v| v.as_str()) {
        Some("-") | Some("") | None => serde_json::Value::Null,
        Some(s) => serde_json::Value::String(s.to_string()),
    }
}

/// `rupu transcript list` row → `AgentRunRow` wire shape (`/api/runs/agents`).
pub(crate) fn transcript_row_to_agent_run(row: &serde_json::Value) -> serde_json::Value {
    let total = row
        .get("total_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let null = serde_json::Value::Null;
    serde_json::json!({
        "run_id": row.get("run_id").cloned().unwrap_or(null.clone()),
        "source": "standalone",
        "agent": row.get("agent").cloned().unwrap_or(null.clone()),
        "session_id": null,
        "trigger_source": null,
        "status": row.get("status").cloned().unwrap_or(null.clone()),
        "started_at": row.get("started_at").cloned().unwrap_or(null.clone()),
        "transcript_path": null,
        "usage": usage_json(total, 1),
        "turns": 0,
        "duration_ms": null,
    })
}

/// `rupu autoflow history` row → `AutoflowEventRow` wire shape
/// (`/api/runs/autoflows/events`).
pub(crate) fn history_row_to_autoflow_event(row: &serde_json::Value) -> serde_json::Value {
    // event_id must be a stable non-null key for the UI list; prefer the wake
    // id, else synthesize from cycle_id + timestamp.
    let event_id = match row.get("wake").and_then(|v| v.as_str()) {
        Some(w) if w != "-" && !w.is_empty() => w.to_string(),
        _ => format!(
            "{}:{}",
            row.get("cycle_id").and_then(|v| v.as_str()).unwrap_or(""),
            row.get("at").and_then(|v| v.as_str()).unwrap_or(""),
        ),
    };
    let null = serde_json::Value::Null;
    serde_json::json!({
        "event_id": event_id,
        "cycle_id": row.get("cycle_id").cloned().unwrap_or(null.clone()),
        "at": row.get("at").cloned().unwrap_or(null.clone()),
        "kind": row.get("event").cloned().unwrap_or(null.clone()),
        "workflow": dash_or_null(row, "workflow"),
        "issue_display_ref": dash_or_null(row, "issue"),
        "run_id": dash_or_null(row, "run"),
        "status": null,
        "worker_name": dash_or_null(row, "worker"),
        "usage": usage_json(0, 0),
    })
}

/// Aggregate `rupu autoflow history` event rows into `AutoflowCycleRow` wire
/// shapes (`/api/runs/autoflows`), grouped by `cycle_id`, newest-first. The CLI
/// event stream lacks the ran/skipped/failed breakdown, so those are 0.
pub(crate) fn history_rows_to_autoflow_cycles(
    rows: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    use std::collections::BTreeMap;
    // (mode, worker, earliest_at, latest_at, workflows, run_ids)
    type CycleAccum = (
        String,
        Option<String>,
        String,
        String,
        Vec<String>,
        Vec<String>,
    );
    // Preserve first-seen order (rows arrive newest-first from the CLI).
    let mut order: Vec<String> = Vec::new();
    let mut by_cycle: BTreeMap<String, CycleAccum> = BTreeMap::new();
    for row in rows {
        let cycle_id = match row.get("cycle_id").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() && c != "-" => c.to_string(),
            _ => continue,
        };
        let at = row
            .get("at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mode = row
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let worker = row
            .get("worker")
            .and_then(|v| v.as_str())
            .filter(|w| *w != "-" && !w.is_empty())
            .map(|w| w.to_string());
        let workflow = row
            .get("workflow")
            .and_then(|v| v.as_str())
            .filter(|w| *w != "-" && !w.is_empty())
            .map(|w| w.to_string());
        let run = row
            .get("run")
            .and_then(|v| v.as_str())
            .filter(|r| *r != "-" && !r.is_empty())
            .map(|r| r.to_string());

        let entry = by_cycle.entry(cycle_id.clone()).or_insert_with(|| {
            order.push(cycle_id.clone());
            (
                mode.clone(),
                worker.clone(),
                at.clone(),
                at.clone(),
                Vec::new(),
                Vec::new(),
            )
        });
        // entry = (mode, worker, earliest_at, latest_at, workflows, run_ids)
        if !at.is_empty() {
            if at < entry.2 {
                entry.2 = at.clone();
            }
            if at > entry.3 {
                entry.3 = at.clone();
            }
        }
        if let Some(w) = workflow {
            if !entry.4.contains(&w) {
                entry.4.push(w);
            }
        }
        if let Some(r) = run {
            if !entry.5.contains(&r) {
                entry.5.push(r);
            }
        }
    }
    order
        .into_iter()
        .map(|cid| {
            let (mode, worker, started_at, finished_at, workflows, run_ids) =
                by_cycle.remove(&cid).unwrap();
            serde_json::json!({
                "cycle_id": cid,
                "mode": mode,
                "worker_name": worker,
                "started_at": started_at,
                "finished_at": finished_at,
                "workflow_count": workflows.len(),
                "ran_cycles": 0,
                "skipped_cycles": 0,
                "failed_cycles": 0,
                "run_ids": run_ids,
                "usage": usage_json(0, 0),
            })
        })
        .collect()
}

// ── RemoteExec trait + types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct RemoteOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoteExecError {
    #[error("ssh spawn failed: {0}")]
    Spawn(String),
    #[error("remote command exited with {code:?}: {stderr}")]
    NonZero { code: Option<i32>, stderr: String },
}

/// A pinned, boxed stream of lines from a remote command.
pub(crate) type LineStream = Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>>;

/// Port: run a command on the remote host.
///
/// The real impl (`SshExec`) shells out to the system `ssh`; tests inject a fake.
#[async_trait::async_trait]
pub(crate) trait RemoteExec: Send + Sync {
    /// Run `remote_command` to completion and collect its output.
    async fn run(&self, remote_command: &str) -> Result<RemoteOutput, RemoteExecError>;

    /// Spawn `remote_command` and return a stream of its stdout lines.
    ///
    /// The ssh child is kept alive for the stream's duration. When the stream
    /// is dropped the child is killed via `kill_on_drop(true)`.
    fn spawn_lines(&self, remote_command: &str) -> Result<LineStream, RemoteExecError>;

    /// Run `remote_command`, writing `stdin` to it (if any), and return its
    /// raw stdout bytes. Binary-safe — unlike `run`, which lossily decodes
    /// UTF-8. A spawn/connection failure is `Spawn`; a nonzero remote exit is
    /// `NonZero { code, stderr }`.
    async fn run_bytes(
        &self,
        remote_command: &str,
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, RemoteExecError>;
}

// ── Internal stream wrapper ───────────────────────────────────────────────────

/// Owns both the ssh `Child` and the `LinesStream` so the child process is
/// killed when this stream is dropped.
///
/// No `async-stream` macro is used. Both fields are `Unpin`, so the wrapper is
/// `Unpin` too and `poll_next` can delegate to the inner stream without unsafe.
struct SshLineStream {
    /// Kept for its `Drop` impl: `kill_on_drop(true)` kills the child when this
    /// field is dropped.
    _child: tokio::process::Child,
    /// The actual line producer. Stored boxed so `SshLineStream` stays `Unpin`.
    inner: Pin<Box<dyn Stream<Item = std::io::Result<String>> + Send>>,
}

impl Stream for SshLineStream {
    type Item = std::io::Result<String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Both Self and inner are Unpin, so no pin projection needed.
        self.inner.as_mut().poll_next(cx)
    }
}

// ── SshExec real implementation ───────────────────────────────────────────────

pub(crate) struct SshExec {
    pub host: String,
    pub port: Option<u16>,
    pub identity_file: Option<std::path::PathBuf>,
}

#[async_trait::async_trait]
impl RemoteExec for SshExec {
    async fn run(&self, remote_command: &str) -> Result<RemoteOutput, RemoteExecError> {
        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
            SHORT_CALL_CONNECT_TIMEOUT_SECS,
        );
        let out = tokio::process::Command::new("ssh")
            .args(&argv)
            .output()
            .await
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        Ok(RemoteOutput {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        })
    }

    fn spawn_lines(&self, remote_command: &str) -> Result<LineStream, RemoteExecError> {
        use tokio::io::AsyncBufReadExt as _;

        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
            PUMP_CONNECT_TIMEOUT_SECS,
        );
        let mut child = tokio::process::Command::new("ssh")
            .args(&argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RemoteExecError::Spawn("no stdout pipe".into()))?;
        let reader = tokio::io::BufReader::new(stdout);
        let lines = tokio_stream::wrappers::LinesStream::new(reader.lines());

        // Wrap `_child` + `inner` together so the child is killed when the
        // stream is dropped. No async-stream or unsafe needed.
        let stream = SshLineStream {
            _child: child,
            inner: Box::pin(lines),
        };
        Ok(Box::pin(stream))
    }

    async fn run_bytes(
        &self,
        remote_command: &str,
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, RemoteExecError> {
        use tokio::io::AsyncWriteExt;
        let argv = ssh_argv(
            &self.host,
            self.port,
            self.identity_file.as_deref(),
            remote_command,
            SHORT_CALL_CONNECT_TIMEOUT_SECS,
        );
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args(&argv)
            .stdin(if stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        if let Some(bytes) = stdin {
            let mut si = child
                .stdin
                .take()
                .ok_or_else(|| RemoteExecError::Spawn("no stdin pipe".into()))?;
            si.write_all(&bytes)
                .await
                .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
            si.shutdown()
                .await
                .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
            drop(si);
        }
        let out = child
            .wait_with_output()
            .await
            .map_err(|e| RemoteExecError::Spawn(e.to_string()))?;
        if !out.status.success() {
            return Err(RemoteExecError::NonZero {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(out.stdout)
    }
}

// ── Tail pump helpers ─────────────────────────────────────────────────────────

/// How often the tail pump polls the remote `run.json` for a terminal status.
///
/// The first tick of [`tokio::time::interval`] fires immediately, so the pump
/// can resolve near-instantly when the run is already terminal (e.g. in tests
/// or when the pump attaches after a fast run).
const PUMP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a tail pump waits for ANY sign that its run exists on the host
/// before giving up and finalizing it as failed.
///
/// `tail -n +1 -F` NEVER exits on its own: it retries missing paths forever.
/// The pump's only two exits are (a) the interval arm observing a terminal
/// `run.json` and (b) the tail stream ending. For a run whose remote process
/// died before creating `$HOME/.rupu/runs/<run_id>/` at all, NEITHER can ever
/// happen — `run.json` never appears, and `tail -F` sits on four paths that
/// never appear either. The pump then spins forever, holding an ssh session
/// and a remote `tail` process. Measured in production: two such pumps
/// outlived their runs by 3h40m and 1h39m.
///
/// (A CANCELLED run is NOT this case and never was: `cancel_run` shells
/// `rupu workflow cancel` on the remote, which writes a terminal status into
/// `run.json`, so the interval arm sees it and the pump exits normally.)
///
/// Deliberately the same five minutes as `rupu-cli`'s `POLL_MAX_STARTUP`, and
/// for the same reason: both are waiting on exactly one event — the remote
/// `rupu` starting the run — so they should give up on it together. Same
/// justification too: ~5x the ~60 s measured for staging a ~50 MB workspace
/// over SSH plus process start, which is enough that a genuinely slow launch
/// still gets mirrored, and short enough that a launch that never happened
/// does not strand an ssh session for hours.
///
/// **What "never seen" must mean.** This deadline is only allowed to fire on
/// the absence of a start, and the pump's two passive signals do not prove
/// that absence on their own: a `==>` header depends on the transcript
/// landing at the ONE `$HOME/.rupu/transcripts/` path this pump tails (a
/// staged workspace carrying its own `.rupu/` moves it), and `run.json` for
/// an agent run is not written until the agent FINISHES. So before firing,
/// the pump asks the host directly ([`probe_run_start_evidence`]); only a
/// definite "no trace of this run" lets the deadline through.
const PUMP_STARTUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Absolute cap on how long a tail pump may live without ever observing a
/// terminal `run.json`.
///
/// Once [`PUMP_STARTUP_DEADLINE`] has been cleared by real evidence the run
/// started, the pump is no longer bounded by a STARTUP timer — a four-hour
/// run must not be cut off by one. It still needs a backstop, though: a run
/// that started and was then SIGKILLed never writes a terminal `run.json`,
/// and `tail -F` never ends, so without this the pump would hold its ssh
/// session forever again. Four hours matches `rupu-cli`'s `POLL_MAX_WALL` —
/// the longest a placed run is ever waited on — so the pump outlives every
/// run it could legitimately be mirroring and no run it could not.
const PUMP_MAX_WALL: std::time::Duration = std::time::Duration::from_secs(4 * 60 * 60);

/// Returns `true` when `status` is a terminal [`rupu_orchestrator::RunStatus`]
/// serialized value.  Mirrors [`RunStatus::is_terminal`] using the
/// `#[serde(rename_all = "snake_case")]` wire form.
fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "rejected" | "cancelled")
}

/// One-shot terminal transcript catch-up for the tail pump: `cat` the remote
/// transcript and REPLACE the mirrored copy wholesale. The tail stream may be
/// torn down with lines still buffered, so the tailed copy is never trusted
/// as final; this is. A missing remote file (workflow run, or no transcript
/// written) fails the `cat` → skipped, harmless.
async fn pump_catch_up_transcript(
    exec: &dyn RemoteExec,
    mirror: &NodeMirror,
    run_id: &str,
    host_id: &str,
    cat_transcript_cmd: &str,
) {
    if let Ok(t) = exec.run(cat_transcript_cmd).await {
        if t.success && !t.stdout.is_empty() {
            let _ = mirror.replace_transcript(run_id, host_id, &t.stdout);
        }
    }
}

/// Spec §6.1 step 3: one ssh invocation that prints the pump's own
/// `==> <path> <==` header, the file, then a synthetic newline + `==> end <==`
/// for every path. The synthetic newline guarantees the end marker starts a
/// fresh line even when the file's last line is torn; `split_batched_cat`
/// removes it again.
pub(crate) fn batch_cat_command(paths: &[String]) -> String {
    let mut cmd = String::from("for p in");
    for p in paths {
        cmd.push(' ');
        cmd.push_str(&shell_escape(p));
    }
    cmd.push_str(
        "; do printf '==> %s <==\\n' \"$p\"; cat \"$p\" 2>/dev/null; printf '\\n==> end <==\\n'; done",
    );
    cmd
}

/// Inverse of [`batch_cat_command`]: `path → body` for every file whose end
/// marker arrived. A file cut off mid-stream is absent from the map, so the
/// caller leaves its cache untouched and unmarked (§6.2).
pub(crate) fn split_batched_cat(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in stdout.split('\n') {
        match parse_tail_marker(line) {
            Some("end") => {
                if let Some((path, mut lines)) = current.take() {
                    // Drop the ONE synthetic empty line the command appended.
                    if lines.last() == Some(&"") {
                        lines.pop();
                    }
                    let mut body = lines.join("\n");
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    out.insert(path, body);
                }
            }
            Some(path) => current = Some((path.to_string(), Vec::new())),
            None => {
                if let Some((_, lines)) = current.as_mut() {
                    lines.push(line);
                }
            }
        }
    }
    out
}

/// Spec §6: pull every step transcript the run's artifacts recorded into the
/// host's cache, in one ssh round trip, marking each complete. Best-effort:
/// a failure leaves files unmarked for the on-demand retry (§4.2).
///
/// This pull is AUTHORITATIVE and always wins — including over a viewer's live
/// tail on the same file. What it must not do is win by `rename`: a live feed
/// holds an append handle on the cache's inode, and swapping the dentry would
/// strand it on an unlinked file. `lazy` is consulted per target so those get
/// [`write_cache_file_inplace`] instead, which keeps the feed's fd pointing at
/// the file readers see.
async fn pump_pull_step_transcripts(
    exec: &dyn RemoteExec,
    mirror: &NodeMirror,
    lazy: &crate::host::lazy_tail::LazyTailRegistry,
    run_id: &str,
    host_id: &str,
) {
    let global = mirror.global_dir();
    let agent_mirror = mirror.transcript_mirror_path(run_id);
    let mut targets: Vec<(String, std::path::PathBuf)> = Vec::new();
    for recorded in
        crate::host::transcript_paths::recorded_transcript_paths(mirror.run_store(), run_id)
    {
        if recorded == agent_mirror {
            continue; // handled by pump_catch_up_transcript
        }
        let Some(cache) = crate::host::transcript_paths::cache_path(&global, host_id, &recorded)
        else {
            continue;
        };
        if crate::host::transcript_paths::is_complete(&cache) {
            continue;
        }
        if let Some(remote) = recorded.to_str() {
            targets.push((remote.to_string(), cache));
        }
    }
    if targets.is_empty() {
        return;
    }
    let remotes: Vec<String> = targets.iter().map(|(r, _)| r.clone()).collect();
    let Ok(out) = exec.run(&batch_cat_command(&remotes)).await else {
        return;
    };
    let files = split_batched_cat(&out.stdout);
    for (remote, cache) in &targets {
        if let Some(body) = files.get(remote) {
            let write = if lazy.has_live_feed(cache) {
                write_cache_file_inplace
            } else {
                write_cache_file
            };
            if let Err(e) = write(cache, body, true) {
                tracing::warn!(host_id, run_id, cache = %cache.display(), error = %e, "terminal transcript pull: cache write failed");
            }
        } else {
            tracing::warn!(
                host_id,
                run_id,
                remote,
                "terminal transcript pull did not deliver this file; left for on-demand retry"
            );
        }
    }
}

/// What one [`pump_finalize_if_terminal`] probe learned about the run.
///
/// The pump needs BOTH bits this carries. `Finalized` is its exit condition;
/// `Absent` vs `Alive` is how it tells "the host has no record of this run at
/// all" from "the run exists and is still working", which is what
/// [`PUMP_STARTUP_DEADLINE`] is bounded against. Folding those two into one
/// `false` is what left the pump with no way to distinguish a run that had
/// not started yet from one that never would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PumpProbe {
    /// `run.json` could not be read (missing, unreachable, empty, unparseable).
    Absent,
    /// `run.json` was read and its status is non-terminal — the run exists.
    Alive,
    /// `run.json` was read, its status is terminal, and the run was finished.
    Finalized,
}

/// One terminal-status probe for the tail pump: `cat run.json`; if the status
/// is terminal, mirror the record, catch up the transcript, and `finish` the
/// run. Returns [`PumpProbe::Finalized`] when the run was finalized (the pump
/// should stop). Shared by the pump's interval tick and its dispatcher nudge
/// so both paths finalize identically.
async fn pump_finalize_if_terminal(
    exec: &dyn RemoteExec,
    mirror: &NodeMirror,
    lazy: &crate::host::lazy_tail::LazyTailRegistry,
    run_id: &str,
    host_id: &str,
    cat_cmd: &str,
    cat_transcript_cmd: &str,
) -> PumpProbe {
    let Ok(out) = exec.run(cat_cmd).await else {
        return PumpProbe::Absent;
    };
    if !out.success || out.stdout.trim().is_empty() {
        return PumpProbe::Absent;
    }
    let trimmed = out.stdout.trim().to_string();
    let Ok(rec) = serde_json::from_str::<serde_json::Value>(&trimmed) else {
        return PumpProbe::Absent;
    };
    let Some(status) = rec.get("status").and_then(|v| v.as_str()) else {
        return PumpProbe::Absent;
    };
    if !is_terminal_status(status) {
        // The host HAS a record for this run — it is simply not done. Mirror
        // it (spec §5.2) so the local record carries the live active step,
        // then clear the startup deadline.
        let _ = mirror.append(run_id, host_id, ArtifactFile::RunJson, &trimmed);
        return PumpProbe::Alive;
    }
    let status = status.to_string();
    let _ = mirror.append(run_id, host_id, ArtifactFile::RunJson, &trimmed);
    // Before `finish`, so the synthesized step-result row sees the complete
    // transcript on disk.
    pump_catch_up_transcript(exec, mirror, run_id, host_id, cat_transcript_cmd).await;
    pump_pull_step_transcripts(exec, mirror, lazy, run_id, host_id).await;
    let _ = mirror.finish(run_id, host_id, &status);
    PumpProbe::Finalized
}

/// Map a [`RemoteExecError`] from `run_bytes` to the corresponding
/// [`HostConnectorError`]: a spawn/connection failure (ssh binary missing,
/// no route to host, etc.) is `Unreachable`; a nonzero exit from the remote
/// `rupu __workspace` helper is `Remote(code, stderr)`.
fn map_remote_err(e: RemoteExecError) -> HostConnectorError {
    match e {
        RemoteExecError::Spawn(m) => HostConnectorError::Unreachable(m),
        RemoteExecError::NonZero { code, stderr } => {
            HostConnectorError::Remote(code.unwrap_or(-1) as u16, stderr)
        }
    }
}

/// Classify a nonzero-exit `rupu workflow|session|transcript <verb>` failure
/// from [`SshHostConnector::remote_workflow`] / `remote_session` /
/// `remote_transcript`.
///
/// Those three helpers previously mapped EVERY nonzero exit to
/// `Unreachable`, which is right for an actual SSH/transport failure but
/// wrong for a load-bearing safety refusal the remote CLI printed to
/// stderr and exited nonzero for — e.g. "is managed by session", "is not
/// terminal", or the standalone-transcript liveness guard's "appears to
/// still be running". `Unreachable` lands the caller in the generic
/// `other => 500` arm of `map_host_mutate_err` / `map_host_session_mutate_err`
/// / `map_host_transcript_mutate_err`, silently downgrading an intentional
/// 409 refusal into an opaque server error. Recognize those refusal shapes
/// here and reclassify as `Invalid`, which those three mapping functions
/// already turn into a 409 — the same status code the LOCAL (non-SSH) branch
/// returns for the identical refusal.
fn classify_remote_cli_failure(stderr: &str) -> HostConnectorError {
    let lower = stderr.to_ascii_lowercase();
    const REFUSAL_MARKERS: &[&str] = &[
        "is managed by session",
        "is not terminal",
        "cancel it first",
        "still running",
        "appears to be running",
        "while the worker is still running",
    ];
    if REFUSAL_MARKERS.iter().any(|m| lower.contains(m)) {
        HostConnectorError::Invalid(stderr.trim().to_string())
    } else {
        HostConnectorError::Unreachable(stderr.trim().to_string())
    }
}

// ── SshHostConnector ──────────────────────────────────────────────────────────

/// [`HostConnector`] backed by SSH transport.
///
/// Dispatches workflow/agent runs as detached remote processes (see
/// [`Self::detach_launch`] for the exact wrapper: `cd` into the staged
/// working dir when there is one, `setsid`, stdin/stdout to `/dev/null`,
/// stderr to a per-run `launch.log`), mirrors their artifact files via an
/// `ssh tail -f` pump that routes `==>` file headers to the right
/// [`ArtifactFile`] variant, and issues control operations as one-shot
/// remote `rupu workflow` commands.  Auth is entirely delegated to the
/// system `ssh`; rupu stores no key material.
pub(crate) struct SshHostConnector {
    pub host_id: String,
    pub exec: Arc<dyn RemoteExec>,
    pub mirror: Arc<NodeMirror>,
    pub run_store: Arc<RunStore>,
    /// Live tail pumps by run id — see [`PumpHandle`]. Entries are inserted
    /// by `spawn_tail_pump` and removed by the pump task itself when its
    /// terminal work is done, so an absent entry means "nothing to wait for".
    pumps: Arc<std::sync::Mutex<std::collections::HashMap<String, PumpHandle>>>,
    /// Shared registry of live `tail -F` feeds into the transcript cache
    /// (spec §5.1) — see [`ensure_transcript_feed`].
    lazy: Arc<crate::host::lazy_tail::LazyTailRegistry>,
}

/// The dispatcher-facing side of one tail pump.
///
/// `done` flips to `true` (or its sender drops) once the pump has finished
/// its terminal work — final transcript catch-up, `run.json` mirror, and
/// `NodeMirror::finish`. `nudge` asks the pump to probe for a terminal
/// status NOW instead of at its next [`PUMP_POLL_INTERVAL`] tick; the
/// dispatcher fires it when its own `get_run` poll has already observed
/// terminal, so the join costs two ssh round-trips rather than up to an
/// extra interval.
#[derive(Clone)]
struct PumpHandle {
    done: tokio::sync::watch::Receiver<bool>,
    nudge: Arc<tokio::sync::Notify>,
}

/// Upper bound on how long [`HostConnector::await_run_mirror`] waits for a
/// pump's terminal work. This is a cap on waiting for a REAL completion
/// signal, not a delay: the normal path resolves in ~two ssh round-trips
/// (`cat` transcript + `cat run.json`), and the bound only bites when the
/// host went unresponsive mid-finalization — where hanging the dispatching
/// CLI forever would be worse than a logged, explicitly-incomplete mirror.
const PUMP_FINALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Rename a `session show` report `item`'s human-table field labels to the
/// API's, so a remote session renders through the same client code as a
/// local one.
///
/// Only the labels that actually differ are touched (`agent` → `agent_name`,
/// `provider` → `provider_name`); every other key passes through untouched,
/// so a newer remote reporting extra fields is not silently truncated here.
/// `usage` is deliberately not computed — this connector has no pricing.
fn session_item_to_api_shape(item: &serde_json::Value) -> serde_json::Value {
    let mut out = item.clone();
    let Some(map) = out.as_object_mut() else {
        return out;
    };
    for (from, to) in [("agent", "agent_name"), ("provider", "provider_name")] {
        if let Some(v) = map.remove(from) {
            map.insert(to.to_string(), v);
        }
    }
    // `runs` belongs to the dedicated `session_runs` surface, not the detail
    // body (the API session DTO has no such field).
    map.remove("runs");
    map.entry("scope")
        .or_insert_with(|| serde_json::Value::String("active".into()));
    serde_json::Value::Object(map.clone())
}

impl SshHostConnector {
    /// Construct a new connector.
    ///
    /// No `pricing` parameter: `get_run` shells the remote CLI, which
    /// resolves pricing from the *remote* host's own config — this
    /// connector no longer computes usage/cost locally (that was
    /// `mirror_get_run`'s job; see `get_run`'s doc comment).
    pub fn new(
        host_id: impl Into<String>,
        exec: Arc<dyn RemoteExec>,
        mirror: Arc<NodeMirror>,
        run_store: Arc<RunStore>,
    ) -> Self {
        let lazy = Arc::new(crate::host::lazy_tail::LazyTailRegistry::new(Arc::clone(
            &exec,
        )));
        Self {
            host_id: host_id.into(),
            exec,
            mirror,
            run_store,
            pumps: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            lazy,
        }
    }

    /// Build the remote argv for a workflow run.
    fn workflow_argv(req: &LaunchRequest, run_id: &str) -> Vec<String> {
        let mut a = vec![
            "rupu".into(),
            "workflow".into(),
            "run".into(),
            req.workflow.clone(),
        ];
        if let Some(t) = &req.target {
            a.push(t.clone());
        }
        a.push("--run-id".into());
        a.push(run_id.to_string());
        a.push("--plain".into());
        for (k, v) in &req.inputs {
            a.push("--input".into());
            a.push(format!("{k}={v}"));
        }
        if let Some(m) = &req.mode {
            a.push("--mode".into());
            a.push(m.clone());
        }
        a
    }

    /// Build the remote argv for an agent run.
    fn agent_argv(req: &AgentLaunchRequest, run_id: &str) -> Vec<String> {
        let mut a = vec!["rupu".into(), "run".into(), req.agent.clone()];
        if let Some(t) = &req.target {
            a.push(t.clone());
        }
        a.push("--run-id".into());
        a.push(run_id.to_string());
        if let Some(m) = &req.mode {
            a.push("--mode".into());
            a.push(m.clone());
        }
        if let Some(p) = &req.prompt {
            a.push("--prompt".into());
            a.push(p.clone());
        }
        if req.target.is_some() {
            a.push("--tmp".into());
        }
        a
    }

    /// The CP global dir, derived exactly as `NodeMirror::transcript_mirror_path`
    /// derives it: the run store root is `<global>/runs`.
    fn global_dir(&self) -> PathBuf {
        crate::host::transcript_paths::global_dir_of(&self.run_store)
    }

    /// Spec §3.3: a remote read is scoped to a run. `run_id` must be a run
    /// this host executed (`worker_id`), and `recorded` must be a path that
    /// run's own artifacts claim. Returns the cache path the file serves
    /// from. Never touches the remote.
    pub(crate) fn authorize_remote_transcript(
        &self,
        run_id: &str,
        recorded: &Path,
    ) -> Result<PathBuf, HostConnectorError> {
        let cache =
            crate::host::transcript_paths::cache_path(&self.global_dir(), &self.host_id, recorded)
                .ok_or_else(|| {
                    HostConnectorError::Invalid(format!(
                        "not a transcript path: {}",
                        recorded.display()
                    ))
                })?;
        let rec = self
            .run_store
            .load(run_id)
            .map_err(|_| HostConnectorError::NotFound(run_id.to_string()))?;
        if rec.worker_id.as_deref() != Some(self.host_id.as_str()) {
            return Err(HostConnectorError::Invalid(format!(
                "run {run_id} does not belong to host {}",
                self.host_id
            )));
        }
        let claimed =
            crate::host::transcript_paths::recorded_transcript_paths(&self.run_store, run_id);
        if !claimed.iter().any(|p| p == recorded) {
            return Err(HostConnectorError::Invalid(format!(
                "run {run_id} did not record transcript {}",
                recorded.display()
            )));
        }
        Ok(cache)
    }

    /// Wrap a shell-escaped remote control command (`rupu workflow resume`)
    /// so it is detached and survives the SSH session closing. Plain
    /// wrapper: no cwd, no log — the run it resumes already has its
    /// lifecycle on the host. Launches use [`Self::detach_launch`].
    fn detach(remote_cmd: &str) -> String {
        format!("setsid {remote_cmd} </dev/null >/dev/null 2>&1 &")
    }

    /// Wrap a shell-escaped remote launch (`rupu run …` / `rupu workflow run
    /// …`) so the run is detached, executes in `working_dir` when one was
    /// staged, and leaves its stderr on the host where a dead launch can be
    /// read back ([`HostConnector::launch_diagnostics`]).
    ///
    /// Shape (one line; `<log>` = `$HOME/.rupu/runs/<run_id>/launch.log`):
    ///
    /// ```text
    /// mkdir -p $HOME/.rupu/runs/<run_id> && (umask 077; : > <log>) && \
    ///   cd '<working_dir>' && (setsid <remote_cmd> </dev/null >/dev/null 2>><log> &)
    /// ```
    ///
    /// with the `cd '<working_dir>' &&` segment present only when
    /// `working_dir` is `Some`. Each piece is load-bearing:
    ///
    /// - **`cd` before the launch.** `rupu run` resolves its workspace from
    ///   the process cwd (`std::env::current_dir()` → workspace upsert →
    ///   `workspace_path`). Without the `cd` a `workspace: sync` step stages
    ///   its tree and then runs the agent in the login `$HOME` — the delta is
    ///   collected from a directory the agent never touched. `None` keeps
    ///   the self-contained path's cwd exactly as before.
    /// - **Only the `setsid` is backgrounded** — the `mkdir`/log-create/`cd`
    ///   run in the ssh session's foreground, so a failure there (staged dir
    ///   vanished, unwritable `$HOME`) is a non-zero ssh exit that the
    ///   launch reports, instead of a "successful" detach with no process
    ///   behind it. The subshell exits immediately after backgrounding;
    ///   nothing keeps the ssh channel open.
    /// - **stderr → per-run log, stdout → `/dev/null`.** Everything a dying
    ///   `rupu` prints (`Error: …`, clap usage, `warn`-level tracing) goes
    ///   to stderr; stdout for an agent run is the rendered transcript, which
    ///   the tail pump already mirrors. The log lives inside the run dir so
    ///   it is removed with the run, and it is created at mode 0600 (the
    ///   `umask 077` is scoped to its own subshell, so the launched process
    ///   inherits the login umask — files the agent writes into the staged
    ///   tree keep their normal modes). The run dir is pre-created here;
    ///   `RunStore::create` uses `create_dir_all` and keys existence on
    ///   `run.json`, and `RunStore::list` skips dirs without one, so a
    ///   pre-existing empty dir changes nothing for the remote `rupu`.
    ///
    /// **Trust boundary.** `working_dir` is remote output (`stage_workspace`
    /// echoes the host's stdout) and is `shell_escape`d — any content is
    /// inert inside the single quotes. `run_id` is interpolated RAW into the
    /// `$HOME/...` paths because `$HOME` must expand on the remote shell;
    /// that is only safe under [`is_safe_run_id`], which this checks and
    /// refuses to build the command without. Launch ids are minted locally
    /// (`run_<ULID>`) so the refusal never fires in practice; it is the
    /// invariant made explicit at the point that depends on it.
    fn detach_launch(
        remote_cmd: &str,
        run_id: &str,
        working_dir: Option<&str>,
    ) -> Result<String, HostConnectorError> {
        if !is_safe_run_id(run_id) {
            return Err(HostConnectorError::Invalid(format!(
                "refusing to launch: run id {run_id:?} is not [A-Za-z0-9_]"
            )));
        }
        let dir = remote_run_dir(run_id);
        let log = remote_launch_log(run_id);
        let mut cmd = format!("mkdir -p {dir} && (umask 077; : > {log}) && ");
        if let Some(wd) = working_dir {
            cmd.push_str(&format!("cd {} && ", shell_escape(wd)));
        }
        cmd.push_str(&format!(
            "(setsid {remote_cmd} </dev/null >/dev/null 2>>{log} &)"
        ));
        Ok(cmd)
    }

    /// Spawn a background tokio task that tails the JSONL artifact files
    /// for `run_id` on the remote host — plus the run's agent transcript,
    /// which lives in a different directory (`$HOME/.rupu/transcripts/`) —
    /// and feeds each line to [`NodeMirror::append`].
    ///
    /// `tail -n +1 -F` emits `==> <path> <==` headers when switching files;
    /// [`parse_tail_marker`] extracts the path, which determines which
    /// [`ArtifactFile`] variant subsequent lines belong to.
    ///
    /// # Termination
    ///
    /// `tail -F` **never exits on its own** — when the remote run finishes, the
    /// artifact files stop growing but `tail` keeps watching.  The pump therefore
    /// uses `tokio::select!` over two arms:
    ///
    /// 1. **Line arm** — routes artifact lines as before; on stream-end/error
    ///    (e.g. SSH connection dropped) breaks and falls through to a best-effort
    ///    cat.
    /// 2. **Interval arm** — fires every [`PUMP_POLL_INTERVAL`] (first tick is
    ///    immediate).  Reads the remote `run.json` and calls
    ///    [`NodeMirror::finish`] when a terminal status is detected.  Dropping
    ///    `stream` at that point triggers `kill_on_drop` on the `ssh` child,
    ///    killing the remote `tail` process and freeing all resources.
    ///
    /// If the stream ends before a terminal status is observed (SSH drop, etc.),
    /// a final `cat run.json` is attempted.  If the status is still non-terminal
    /// (or unreadable), the run is finished as `"failed"` so it is never stuck
    /// in `Running` indefinitely.
    fn spawn_tail_pump(&self, run_id: String) {
        let exec = Arc::clone(&self.exec);
        let mirror = Arc::clone(&self.mirror);
        // The pump's terminal pull needs to know which cache files a viewer is
        // currently tailing, so it can rewrite those in place instead of
        // renaming a fresh file over the inode the feed holds open.
        let lazy = Arc::clone(&self.lazy);
        let host_id = self.host_id.clone();

        // $HOME must expand on the remote shell — build as raw command strings
        // rather than through build_remote_command / shell_escape. Single-quoting
        // every token (as build_remote_command does) would prevent $HOME from
        // expanding, producing a literal path that never exists on the remote.
        // run_id contains only [A-Za-z0-9_] (ULID prefix), so unquoted
        // concatenation is safe. That invariant covers ALL FOUR tailed paths
        // and both cat commands below: run_id is their only variable component.
        //
        // The fourth path is the run's agent transcript, which lives OUTSIDE
        // the run directory (`transcripts/<run_id>.jsonl`, not
        // `runs/<run_id>/…`) — a placed agent run's only real content. It
        // doesn't exist for workflow runs (and doesn't exist yet at spawn
        // time even for agent runs); `tail -F` tolerates a missing path and
        // picks the file up from byte zero when it appears, so the other
        // artifacts mirror regardless.
        let tail_cmd = format!(
            "tail -n +1 -F \
             $HOME/.rupu/runs/{run_id}/events.jsonl \
             $HOME/.rupu/runs/{run_id}/step_results.jsonl \
             $HOME/.rupu/runs/{run_id}/unit_checkpoints.jsonl \
             $HOME/.rupu/transcripts/{run_id}.jsonl"
        );
        let cat_cmd = format!("cat $HOME/.rupu/runs/{run_id}/run.json");
        let cat_transcript_cmd = format!("cat $HOME/.rupu/transcripts/{run_id}.jsonl");
        // `==> <path> <==` suffix that identifies the transcript among the
        // tailed files. The directory component makes mis-attribution
        // impossible: none of the `runs/<run_id>/*.jsonl` artifacts can end
        // with `transcripts/<run_id>.jsonl`, and the transcript can't end
        // with `events.jsonl` / `step_results.jsonl` / `unit_checkpoints.jsonl`.
        let transcript_suffix = format!("transcripts/{run_id}.jsonl");

        // Register the dispatcher-facing handle BEFORE spawning, so a caller
        // that observes terminal via its own `get_run` immediately after
        // `launch_*` returns can never miss the pump.
        let (done_tx, done_rx) = tokio::sync::watch::channel(false);
        let nudge = Arc::new(tokio::sync::Notify::new());
        self.pumps.lock().unwrap().insert(
            run_id.clone(),
            PumpHandle {
                done: done_rx,
                nudge: Arc::clone(&nudge),
            },
        );
        let pumps = Arc::clone(&self.pumps);

        tokio::spawn(async move {
            // The pump proper runs in an inner block so EVERY exit path —
            // including the fallback's early `return` — falls through to the
            // completion signal below. That signal is what lets a short-lived
            // dispatching process (`rupu workflow run` exits the moment the
            // workflow completes) join the terminal work instead of killing
            // this task mid-`cat` and leaving a silently truncated transcript.
            let pump = async {
                let mut current: Option<ArtifactFile> = None;
                // Set to true when the interval-poll arm observes a terminal status
                // and calls mirror.finish.  Used below to skip the fallback cat.
                let mut terminal_seen = false;
                // Set to true the first time the host shows ANY sign that this
                // run exists: a `==>` header from `tail` (the file is there) or
                // a readable `run.json`. Until then the pump is bounded by
                // PUMP_STARTUP_DEADLINE — see that constant for why a pump with
                // no such bound spins forever on a launch that died.
                let mut run_seen_on_host = false;
                let pump_started = tokio::time::Instant::now();
                // Set once the transcript's first `==>` header is seen. `tail -n +1`
                // replays the file from byte zero, so the FIRST header (the initial
                // full replay) truncates the mirrored copy — a respawned pump then
                // overwrites instead of appending a duplicate copy. Later headers
                // are ordinary file switches carrying only new lines: no truncate.
                let mut transcript_replayed = false;

                if let Ok(mut stream) = exec.spawn_lines(&tail_cmd) {
                    let mut interval = tokio::time::interval(PUMP_POLL_INTERVAL);
                    // First tick fires immediately per tokio docs; subsequent ticks
                    // fire every PUMP_POLL_INTERVAL.
                    loop {
                        tokio::select! {
                            maybe_line = stream.next() => {
                                match maybe_line {
                                    Some(Ok(line)) => {
                                        if let Some(path) = parse_tail_marker(&line) {
                                            // A `==>` header means `tail` actually
                                            // OPENED one of the run's artifact files,
                                            // so the run exists on the host. (stderr
                                            // is /dev/null, so an unopenable path
                                            // produces no line at all.)
                                            run_seen_on_host = true;
                                            // Route subsequent lines based on filename
                                            // suffix — the expanded absolute path from
                                            // `tail` still ends with the same basename.
                                            current = if path.ends_with("events.jsonl") {
                                                Some(ArtifactFile::Events)
                                            } else if path.ends_with("step_results.jsonl") {
                                                Some(ArtifactFile::StepResults)
                                            } else if path.ends_with("unit_checkpoints.jsonl") {
                                                Some(ArtifactFile::UnitCheckpoints)
                                            } else if path.ends_with(&transcript_suffix) {
                                                if !transcript_replayed {
                                                    transcript_replayed = true;
                                                    // First header = full replay from
                                                    // byte zero — start from a clean
                                                    // mirrored copy (see the flag's
                                                    // declaration).
                                                    let _ = mirror
                                                        .reset_transcript(&run_id, &host_id);
                                                    let _ = mirror
                                                        .note_transcript_started(&run_id, &host_id);
                                                }
                                                Some(ArtifactFile::Transcript)
                                            } else {
                                                None
                                            };
                                            continue;
                                        }
                                        if line.trim().is_empty() {
                                            continue;
                                        }
                                        if let Some(file) = &current {
                                            let _ = mirror.append(
                                                &run_id, &host_id, file.clone(), &line,
                                            );
                                        }
                                    }
                                    // Stream ended or errored (SSH connection dropped,
                                    // remote process exited, etc.).  Break out and do
                                    // a best-effort final cat below.
                                    _ => break,
                                }
                            }
                            _ = interval.tick() => {
                                match pump_finalize_if_terminal(
                                    exec.as_ref(), &mirror, &lazy, &run_id, &host_id,
                                    &cat_cmd, &cat_transcript_cmd,
                                ).await {
                                    PumpProbe::Finalized => {
                                        terminal_seen = true;
                                        break;
                                        // Dropping `stream` below kills the ssh child
                                        // via kill_on_drop.
                                    }
                                    PumpProbe::Alive => run_seen_on_host = true,
                                    PumpProbe::Absent => {}
                                }
                                // Neither passive signal has shown this run and
                                // the startup deadline has passed. That is NOT yet
                                // proof the launch died: for an agent run `run.json`
                                // only appears when the agent finishes, and the
                                // `==>` header only appears if the transcript landed
                                // at the one $HOME path this pump tails. Ask the
                                // host directly before abandoning a run that may be
                                // working perfectly well.
                                if !run_seen_on_host
                                    && pump_started.elapsed() >= PUMP_STARTUP_DEADLINE
                                {
                                    match probe_run_start_evidence(exec.as_ref(), &run_id).await {
                                        RunStartEvidence::Started => {
                                            // It started. From here the pump is
                                            // bounded by PUMP_MAX_WALL, not by a
                                            // startup timer.
                                            tracing::debug!(
                                                host_id = %host_id,
                                                run_id = %run_id,
                                                "tail pump has not seen this run's \
                                                 artifacts, but the host reports it \
                                                 started; keeping the mirror open"
                                            );
                                            run_seen_on_host = true;
                                        }
                                        evidence => {
                                            // NoTrace: the launch died before doing
                                            // anything. Unknown: no probe reached the
                                            // host, so we are no better informed than
                                            // before and keep the old bound rather
                                            // than hold the session open on a guess.
                                            // Either way, break to the fallback below,
                                            // which finishes the run as failed;
                                            // dropping `stream` kills the ssh child
                                            // and the remote `tail` with it.
                                            tracing::warn!(
                                                host_id = %host_id,
                                                run_id = %run_id,
                                                deadline_secs = PUMP_STARTUP_DEADLINE.as_secs(),
                                                ?evidence,
                                                "tail pump never saw this run on the \
                                                 host within the startup deadline and \
                                                 the host shows no sign it started; \
                                                 abandoning the mirror so the ssh \
                                                 session and remote tail are not held \
                                                 open indefinitely"
                                            );
                                            break;
                                        }
                                    }
                                }
                                // Backstop for a run that DID start and was then
                                // killed without writing a terminal run.json: no
                                // arm of this loop can ever fire for it either.
                                if pump_started.elapsed() >= PUMP_MAX_WALL {
                                    tracing::warn!(
                                        host_id = %host_id,
                                        run_id = %run_id,
                                        wall_secs = PUMP_MAX_WALL.as_secs(),
                                        "tail pump reached its wall-clock cap without \
                                         ever observing a terminal run.json; \
                                         abandoning the mirror"
                                    );
                                    break;
                                }
                            }
                            _ = nudge.notified() => {
                                // A dispatcher already observed terminal through
                                // its own `get_run` poll and is waiting on us:
                                // probe now rather than at the next interval tick.
                                match pump_finalize_if_terminal(
                                    exec.as_ref(), &mirror, &lazy, &run_id, &host_id,
                                    &cat_cmd, &cat_transcript_cmd,
                                ).await {
                                    PumpProbe::Finalized => {
                                        terminal_seen = true;
                                        break;
                                    }
                                    PumpProbe::Alive => run_seen_on_host = true,
                                    PumpProbe::Absent => {}
                                }
                            }
                        }
                    }
                    // `stream` drops here → SshLineStream::_child drops →
                    // kill_on_drop kills the remote `tail` process.
                }

                // If the stream ended before we detected a terminal status (SSH
                // drop, spawn failure, etc.), do a best-effort final cat + finish
                // so the run is never stuck in Running.
                if !terminal_seen {
                    // Ordering matches the happy path (`pump_finalize_if_terminal`):
                    // mirror the final `run.json` FIRST, then catch the transcript
                    // up, then pull the step transcripts, then finish. Running the
                    // transcript work first left the mirrored record stale for the
                    // whole (possibly slow) pull.
                    //
                    // Use the observed status only if it is terminal; a
                    // non-terminal status (e.g. "running") would be wrong to
                    // persist as final since the executor may still be alive.
                    // Finish as "failed" in that case — and when the cat fails
                    // outright — so the run is never stuck in Running.
                    let finish_status = match exec.run(&cat_cmd).await {
                        Ok(out) if out.success && !out.stdout.trim().is_empty() => {
                            let trimmed = out.stdout.trim().to_string();
                            let _ =
                                mirror.append(&run_id, &host_id, ArtifactFile::RunJson, &trimmed);
                            serde_json::from_str::<serde_json::Value>(&trimmed)
                                .ok()
                                .and_then(|rec| {
                                    rec.get("status")
                                        .and_then(|v| v.as_str())
                                        .map(str::to_string)
                                })
                                .filter(|s| is_terminal_status(s))
                                .unwrap_or_else(|| "failed".to_string())
                        }
                        _ => "failed".to_string(),
                    };
                    // Same terminal transcript catch-up as the interval arm above:
                    // the stream ended without a clean terminal detection, so any
                    // buffered-but-undelivered transcript lines are gone. Replace
                    // the mirrored copy with the remote's full content while we
                    // can still reach the host. Best-effort, like the run.json cat.
                    pump_catch_up_transcript(
                        exec.as_ref(),
                        &mirror,
                        &run_id,
                        &host_id,
                        &cat_transcript_cmd,
                    )
                    .await;
                    // The host is still reachable here too — pull every
                    // recorded step transcript before finishing the run.
                    pump_pull_step_transcripts(exec.as_ref(), &mirror, &lazy, &run_id, &host_id)
                        .await;
                    let _ = mirror.finish(&run_id, &host_id, &finish_status);
                }
            };
            pump.await;
            // Terminal work is on disk. Deregister first, then signal: an
            // awaiter that cloned the handle earlier gets the flip; one that
            // looks up afterwards finds no entry, which means "already done".
            pumps.lock().unwrap().remove(&run_id);
            let _ = done_tx.send(true);
        });
    }

    /// Issue a one-shot `rupu workflow <tail...>` command on the remote host.
    ///
    /// Used by [`cancel_run`], [`approve_run`], [`reject_run`], and the
    /// run archive/restore/delete overrides.
    async fn remote_workflow(&self, tail: &[&str]) -> Result<(), HostConnectorError> {
        let mut argv: Vec<String> = vec!["rupu".into(), "workflow".into()];
        argv.extend(tail.iter().map(|s| s.to_string()));
        let cmd = build_remote_command(&argv);
        let out = self
            .exec
            .run(&cmd)
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success {
            return Err(classify_remote_cli_failure(&out.stderr));
        }
        Ok(())
    }

    /// Issue a one-shot `rupu session <tail...>` command on the remote host.
    /// The `rupu workflow`-prefixed sibling of [`remote_workflow`]; used by
    /// the session archive/restore/delete overrides.
    async fn remote_session(&self, tail: &[&str]) -> Result<(), HostConnectorError> {
        let mut argv: Vec<String> = vec!["rupu".into(), "session".into()];
        argv.extend(tail.iter().map(|s| s.to_string()));
        let cmd = build_remote_command(&argv);
        let out = self
            .exec
            .run(&cmd)
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success {
            return Err(classify_remote_cli_failure(&out.stderr));
        }
        Ok(())
    }

    /// Issue a one-shot `rupu transcript <tail...>` command on the remote
    /// host. The `rupu session`-prefixed sibling of [`remote_session`]; used
    /// by the transcript archive/delete overrides.
    async fn remote_transcript(&self, tail: &[&str]) -> Result<(), HostConnectorError> {
        let mut argv: Vec<String> = vec!["rupu".into(), "transcript".into()];
        argv.extend(tail.iter().map(|s| s.to_string()));
        let cmd = build_remote_command(&argv);
        let out = self
            .exec
            .run(&cmd)
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success {
            return Err(classify_remote_cli_failure(&out.stderr));
        }
        Ok(())
    }

    /// Run a one-shot `rupu <argv...>` over ssh and return the parsed JSON
    /// value of the CLI's `--format json` report. Shared command-building +
    /// error-mapping for [`remote_json_rows`](Self::remote_json_rows)
    /// (extracts `.rows`) and [`remote_json_item`](Self::remote_json_item)
    /// (extracts `.item`).
    async fn remote_json(&self, argv: &[&str]) -> Result<serde_json::Value, HostConnectorError> {
        let owned: Vec<String> = std::iter::once("rupu".to_string())
            .chain(argv.iter().map(|s| s.to_string()))
            .collect();
        let cmd = build_remote_command(&owned);
        let out = self
            .exec
            .run(&cmd)
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success {
            return Err(HostConnectorError::Unreachable(out.stderr));
        }
        serde_json::from_str(out.stdout.trim()).map_err(|e| {
            HostConnectorError::Remote(0, format!("parse `rupu {}` output: {e}", argv.join(" ")))
        })
    }

    /// Run a one-shot `rupu <argv...>` over ssh and return the `rows` array of
    /// the CLI's `--format json` report. Used by the list-view connectors.
    async fn remote_json_rows(
        &self,
        argv: &[&str],
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let parsed = self.remote_json(argv).await?;
        Ok(parsed
            .get("rows")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Whether a failed remote CLI call means "that session does not exist"
    /// as opposed to "this host could not answer".
    ///
    /// The distinction decides a 404 vs a 500, so it must key on what the
    /// commands actually print. `rupu session show` and `rupu session
    /// usage-timeline` both resolve through `session.rs`'s `read_session`,
    /// which bails with "unknown session: <id>" — NOT "not found", which is
    /// `run show`'s phrasing. Both spellings are matched so either
    /// command's wording works, and the id must appear too: a generic
    /// failure that happens to contain the phrase is not a statement about
    /// THIS session.
    fn says_session_absent(message: &str, id: &str) -> bool {
        let lower = message.to_lowercase();
        (lower.contains("unknown session") || lower.contains("not found")) && message.contains(id)
    }

    /// The useful half of a failed `remote_json` call.
    ///
    /// [`remote_json`](Self::remote_json) maps a NON-ZERO EXIT to
    /// `Unreachable`, whose Display prefixes "host unreachable:". For a host
    /// that answered and simply rejected the command (an out-of-date `rupu`
    /// printing "unrecognized subcommand"), that prefix is actively wrong —
    /// it points an operator at the network instead of at the binary. Unwrap
    /// the inner message so callers can report what the remote actually
    /// said.
    fn remote_failure_detail(e: &HostConnectorError) -> String {
        match e {
            HostConnectorError::Unreachable(m) => m.trim().to_string(),
            other => other.to_string(),
        }
    }

    /// Fetch one session's `session show` report `item` over ssh.
    ///
    /// A remote that explicitly says the session is absent maps to
    /// `NotFound`; anything else (a host predating the command, unreachable,
    /// malformed body) maps to `Unsupported` — "the host cannot report" is
    /// not the same as "the session does not exist", and rendering the
    /// latter would present an empty page as truth. Same rule as
    /// [`get_run`](Self::get_run).
    async fn session_show_item(&self, id: &str) -> Result<serde_json::Value, HostConnectorError> {
        match self
            .remote_json_item(&["--format", "json", "session", "show", id])
            .await
        {
            Ok(v) => Ok(v),
            Err(e) => {
                if Self::says_session_absent(&e.to_string(), id) {
                    return Err(HostConnectorError::NotFound(id.to_string()));
                }
                tracing::warn!(
                    host_id = %self.host_id,
                    session_id = %id,
                    error = %e,
                    "session_show_item: remote `rupu session show` failed; host may predate it"
                );
                Err(HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu session show --format json`: {}",
                    self.host_id,
                    Self::remote_failure_detail(&e)
                )))
            }
        }
    }

    /// Run a one-shot `rupu <argv...>` over ssh and return the `item` object
    /// of the CLI's `--format json` report. Used by [`get_run`](Self::get_run).
    async fn remote_json_item(
        &self,
        argv: &[&str],
    ) -> Result<serde_json::Value, HostConnectorError> {
        let parsed = self.remote_json(argv).await?;
        parsed.get("item").cloned().ok_or_else(|| {
            HostConnectorError::Remote(0, format!("rupu {} output missing `item`", argv.join(" ")))
        })
    }
}

#[async_trait::async_trait]
impl HostConnector for SshHostConnector {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        // Reachability: can we execute anything over ssh? (`true` exits 0; ssh
        // itself exits nonzero on a connection failure.)
        let probe = build_remote_command(&["true".to_string()]);
        let reachable = matches!(self.exec.run(&probe).await, Ok(o) if o.success);
        // Version: best-effort `rupu --version` (prints e.g. "rupu 0.35.2"),
        // taking the trailing version token to match the bare-semver format the
        // local/HTTP connectors report. Only attempted when reachable.
        let version = if reachable {
            let vc = build_remote_command(&["rupu".to_string(), "--version".to_string()]);
            match self.exec.run(&vc).await {
                Ok(o) if o.success => o
                    .stdout
                    .split_whitespace()
                    .last()
                    .map(str::to_string)
                    .filter(|s| !s.is_empty()),
                _ => None,
            }
        } else {
            None
        };
        Ok(HostInfo {
            reachable,
            version,
            capabilities: HostCapabilities::default(),
        })
    }

    async fn launch_run(&self, req: LaunchRequest) -> Result<String, HostConnectorError> {
        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Workflow,
            name: req.workflow.clone(),
            inputs: req.inputs.clone(),
            prompt: None,
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        // Build the remote command BEFORE creating the mirror run: a refused
        // build (see `detach_launch`) must not leave a Running mirror record
        // with no executor behind it.
        let argv = Self::workflow_argv(&req, &run_id);
        let remote_cmd = build_remote_command(&argv);
        let detached = Self::detach_launch(&remote_cmd, &run_id, req.working_dir.as_deref())?;

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let out = match self.exec.run(&detached).await {
            Ok(o) => o,
            Err(e) => {
                // Spawn error (e.g. ssh binary not found): mirror run would
                // be stuck Running with no executor — clean it up now.
                let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
                return Err(HostConnectorError::Unreachable(e.to_string()));
            }
        };

        if !out.success {
            // Best-effort cleanup: mark the mirror run failed so it doesn't
            // stay stuck in Running with no executor attached.
            let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
            return Err(HostConnectorError::Unreachable(out.stderr));
        }

        self.spawn_tail_pump(run_id.clone());
        Ok(run_id)
    }

    async fn launch_agent(&self, req: AgentLaunchRequest) -> Result<String, HostConnectorError> {
        // A coordinator dispatching a placed unit already minted this run's
        // id (see `UnitDispatch::run_id`) so it can know the child run's
        // mirrored transcript path before dispatch — validate it BEFORE
        // touching `build_remote_command`/`detach_launch` (nothing shells
        // for a bad id) and before `create_run` (no mirror record left
        // behind). `None` ⇒ this connector mints one, as before.
        let run_id = match req.run_id.as_deref() {
            Some(id) => {
                let ok = id.starts_with("run_")
                    && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !ok {
                    return Err(HostConnectorError::Invalid(format!(
                        "supplied run id {id:?} is not a valid run id"
                    )));
                }
                id.to_string()
            }
            None => format!("run_{}", Ulid::new()),
        };

        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: req.agent.clone(),
            inputs: std::collections::BTreeMap::new(),
            prompt: req.prompt.clone(),
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        // Build the remote command BEFORE creating the mirror run — see
        // `launch_run`.
        let argv = Self::agent_argv(&req, &run_id);
        let remote_cmd = build_remote_command(&argv);
        let detached = Self::detach_launch(&remote_cmd, &run_id, req.working_dir.as_deref())?;

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let out = match self.exec.run(&detached).await {
            Ok(o) => o,
            Err(e) => {
                // Spawn error: mirror run would be stuck Running — clean up.
                let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
                return Err(HostConnectorError::Unreachable(e.to_string()));
            }
        };

        if !out.success {
            let _ = self.mirror.finish(&run_id, &self.host_id, "failed");
            return Err(HostConnectorError::Unreachable(out.stderr));
        }

        self.spawn_tail_pump(run_id.clone());
        Ok(run_id)
    }

    async fn start_session(&self, _req: SessionStartRequest) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "sessions not supported over ssh (slice 2c)".into(),
        ))
    }

    async fn send_session_turn(
        &self,
        _req: SendMessageRequest,
    ) -> Result<String, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "sessions not supported over ssh (slice 2c)".into(),
        ))
    }

    /// List runs by shelling the remote CLI.
    ///
    /// Was: `mirror_list_runs`. The mirror is populated only by
    /// `spawn_tail_pump`, which runs solely on the launch path — so runs
    /// started directly on the box, or launched by a PREVIOUS `cp serve`
    /// process, were permanently invisible. Enumerating via the CLI is the
    /// same pattern `list_sessions` / `list_autoflow_runs` / `list_agent_runs`
    /// already use.
    ///
    /// Returns `remote_json_rows`' rows **verbatim** — no reshaping mapper.
    /// `rupu run list` (Task 1) emits `rupu_cp::api::runs::RunListRow` JSON
    /// directly, which is exactly the wire shape `/api/runs` needs (`id`,
    /// `usage`, `turns`, `duration_ms`, …). A hand-written mapper here
    /// previously (`run_list_row_to_wire`) dropped `usage`/`turns`/
    /// `duration_ms` — fields the web UI reads unguarded — which crashed the
    /// whole runs list for any host with a visible SSH run. Do not
    /// reintroduce one; see `RunListRow`'s doc comment.
    ///
    /// `stream_run_events` still reads the mirror, deliberately: tailing a
    /// known path on a live run is a different problem from enumerating.
    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let rows = match self
            .remote_json_rows(&["--format", "json", "run", "list", "--limit", "10000"])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // An old remote rupu has no `run list`; it parses as "launch an
                // agent named list" and errors. Surface it as Unsupported so the
                // freshness strip renders "needs a newer rupu" rather than
                // silently reporting zero runs.
                tracing::warn!(
                    host_id = %self.host_id,
                    error = %e,
                    "list_runs: remote `rupu run list` failed; host may predate the command"
                );
                return Err(HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu run list`: {e}",
                    self.host_id
                )));
            }
        };

        let mut out: Vec<serde_json::Value> = rows
            .into_iter()
            .filter(|r| match params.kind {
                RunKind::All => true,
                // Workflow-only means manual-triggered only, mirroring
                // query_run_rows' `event.is_none() && source_wake_id.is_none()`.
                RunKind::Workflow => r["trigger"] == "manual",
            })
            .filter(|r| match params.lifecycle.as_deref() {
                None => true,
                Some("active") => !matches!(
                    r["status"].as_str().unwrap_or(""),
                    "completed" | "failed" | "rejected" | "cancelled"
                ),
                Some("completed") => r["status"] == "completed",
                Some("failed") => r["status"] == "failed",
                Some(_) => true,
            })
            .collect();

        // The CLI already sorts newest-first, but re-sort so this is correct
        // regardless of remote CLI version.
        out.sort_by(|a, b| {
            let ta = a["started_at"].as_str().unwrap_or("");
            let tb = b["started_at"].as_str().unwrap_or("");
            tb.cmp(ta)
        });

        Ok(out
            .into_iter()
            .skip(params.offset)
            .take(params.limit)
            .collect())
    }

    /// Fetch one run by shelling the remote CLI.
    ///
    /// Was: `mirror_get_run`, which only saw runs THIS process launched — so
    /// after the `list_runs` fix (above) the list would show runs whose
    /// detail 404'd against the (still-empty, for a directly-started run)
    /// mirror. The list and the detail must agree.
    ///
    /// Error mapping is a two-way rule, and BOTH directions matter: *a thing
    /// that cannot report is not a thing that is absent.*
    ///
    /// - A remote that cannot even parse `run show` (old rupu, no such
    ///   subcommand) must never be reported as `NotFound` — that would
    ///   silently hide a run that genuinely exists on that host behind a
    ///   "no such run" message. This is the same failure mode `list_runs`
    ///   guards against. Old-host stderr looks like our own format-gate
    ///   rejecting the flag before the subcommand even runs, e.g.:
    ///     `run does not support `--format json` (supported: `table`)`
    ///   (from `output::formats::ensure_supported`, not clap — a message we
    ///   control) — or, on hosts old enough to lack `run show` entirely, a
    ///   "launch an agent named show" parse failure. Either way: `Unsupported`.
    /// - Conversely, a remote that DID run `run show` and explicitly told us
    ///   the run doesn't exist must not be flattened into "this host cannot
    ///   report runs" — that hides a real 404 behind a capability complaint.
    ///   Current-rupu not-found stderr looks like:
    ///     `[error] run run_DOESNOTEXIST: run `run_DOESNOTEXIST` not found`
    ///   Map that to `NotFound`.
    ///
    /// The two are told apart by sniffing the failure's message for
    /// `"not found"` (case-insensitive) *and* the run id itself — both must
    /// appear, so a message about some unrelated thing being not found (e.g.
    /// an old host's classifier failing with "agent 'show' not found",
    /// because it read `show` as an agent name) is not mistaken for "this
    /// run does not exist" just because it happens to contain the words
    /// "not found". Absence of that marker defaults to `Unsupported` — the
    /// safe default, since a false `NotFound` (hiding a real run) is worse
    /// than an occasional over-cautious `Unsupported`.
    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        // NOTE the flag order: `--format json` must precede `run`, because
        // `Cmd::Run` is trailing_var_arg and swallows everything after it —
        // see `remote_json_rows`'s callers / cmd::run's module doc.
        match self
            .remote_json_item(&["--format", "json", "run", "show", run_id])
            .await
        {
            Ok(v) => Ok(v),
            Err(e) => {
                let message = e.to_string();
                let message_lower = message.to_lowercase();
                if message_lower.contains("not found") && message.contains(run_id) {
                    // The remote ran `run show` and explicitly said the run
                    // is absent — believe it.
                    tracing::warn!(
                        host_id = %self.host_id,
                        run_id = %run_id,
                        error = %e,
                        "get_run: remote reported run not found"
                    );
                    return Err(HostConnectorError::NotFound(run_id.to_string()));
                }
                // Anything else (old host that can't parse `run show`,
                // unreachable, malformed body): the host cannot report, not
                // "the run is absent". Map to Unsupported, never NotFound.
                tracing::warn!(
                    host_id = %self.host_id,
                    run_id = %run_id,
                    error = %e,
                    "get_run: remote `rupu run show` failed; host may predate the command"
                );
                Err(HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu run show`: {e}",
                    self.host_id
                )))
            }
        }
    }

    async fn approve_run(&self, run_id: &str, mode: &str) -> Result<(), HostConnectorError> {
        if mode.is_empty() {
            self.remote_workflow(&["approve", run_id]).await
        } else {
            self.remote_workflow(&["approve", run_id, "--mode", mode])
                .await
        }
    }

    async fn reject_run(
        &self,
        run_id: &str,
        reason: Option<&str>,
    ) -> Result<(), HostConnectorError> {
        match reason {
            Some(r) => {
                self.remote_workflow(&["reject", run_id, "--reason", r])
                    .await
            }
            None => self.remote_workflow(&["reject", run_id]).await,
        }
    }

    async fn cancel_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.remote_workflow(&["cancel", run_id]).await
    }

    /// Join the tail pump's terminal work for `run_id`. See the trait doc
    /// for why this exists; see [`PumpHandle`] / [`PUMP_FINALIZE_TIMEOUT`]
    /// for the mechanism and the bound. No pump registered ⇒ nothing to
    /// wait for (already finished, or never launched by this process).
    async fn await_run_mirror(&self, run_id: &str) {
        let handle = self.pumps.lock().unwrap().get(run_id).cloned();
        let Some(PumpHandle { mut done, nudge }) = handle else {
            return;
        };
        nudge.notify_one();
        let wait = async {
            loop {
                if *done.borrow_and_update() {
                    break;
                }
                // Err ⇒ the sender dropped (pump task gone) — nothing more
                // will ever arrive; treat as done.
                if done.changed().await.is_err() {
                    break;
                }
            }
        };
        if tokio::time::timeout(PUMP_FINALIZE_TIMEOUT, wait)
            .await
            .is_err()
        {
            tracing::warn!(
                host_id = %self.host_id,
                run_id,
                bound_secs = PUMP_FINALIZE_TIMEOUT.as_secs(),
                "tail pump did not finish its terminal work within the bound; \
                 the mirrored transcript for this run may be incomplete"
            );
        }
    }

    /// Cooperatively pause a remote in-flight run.
    ///
    /// Same mechanism as [`cancel_run`](Self::cancel_run): a one-shot,
    /// blocking `rupu workflow pause <run_id>` on the remote host. That
    /// command (the exact primitive `LocalHostConnector::pause_run` uses
    /// in-process) flips the remote's own `RunStore` record to `Paused` and
    /// writes the pause marker the *already-running* detached
    /// `rupu workflow run`/`rupu run` process polls (~every 250ms) — so the
    /// remote's own in-process executor genuinely honors the pause at its
    /// next safe boundary. Quick, like `cancel`/`approve`/`reject` — no
    /// detach needed.
    /// `cat` the per-run `launch.log` that [`Self::detach_launch`] pointed the
    /// detached process's stderr at. `None` when the log is absent or empty
    /// (the launch is fine, or hasn't written anything yet), when the host
    /// can't be reached, or when `run_id` fails [`is_safe_run_id`] — this id
    /// arrives from callers, not from our own minting, and is interpolated
    /// unquoted into a `$HOME/...` path, so an id outside the alphabet is
    /// refused without touching the host. The excerpt is bounded to the last
    /// [`LAUNCH_DIAGNOSTICS_MAX_CHARS`].
    async fn launch_diagnostics(&self, run_id: &str) -> Option<String> {
        if !is_safe_run_id(run_id) {
            return None;
        }
        let cmd = format!("cat {}", remote_launch_log(run_id));
        let out = self.exec.run(&cmd).await.ok()?;
        if !out.success {
            return None;
        }
        let text = out.stdout.trim();
        if text.is_empty() {
            return None;
        }
        Some(tail_chars(text, LAUNCH_DIAGNOSTICS_MAX_CHARS))
    }

    /// Ask the host directly whether this run's process ever started, without
    /// going through `run.json` — which for an agent run is written only when
    /// the agent finishes. One ssh round trip; see
    /// [`remote_start_evidence_cmd`] for what counts as evidence and why the
    /// run directory and `launch.log` deliberately do not.
    async fn run_start_evidence(&self, run_id: &str) -> RunStartEvidence {
        probe_run_start_evidence(self.exec.as_ref(), run_id).await
    }

    async fn pause_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.remote_workflow(&["pause", run_id]).await
    }

    /// Resume a `Paused` remote run.
    ///
    /// Unlike `cancel`/`pause` (a quick status flip on an already-live or
    /// already-stopped process), resuming re-enters `run_workflow` from the
    /// persisted checkpoint — the same shape as [`launch_run`](Self::launch_run),
    /// not a fast operation. So this dispatches the existing
    /// `rupu workflow resume <run_id>` command (which already accepts a
    /// `Paused` run — see the T4 commit) as a **detached** remote process
    /// (`Self::detach`, the same wrapping `launch_run` uses) rather than
    /// through `remote_workflow`'s blocking exec, which would otherwise tie
    /// up this call until the entire resumed workflow finished.
    async fn resume_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        let argv = vec![
            "rupu".to_string(),
            "workflow".to_string(),
            "resume".to_string(),
            run_id.to_string(),
        ];
        let remote_cmd = build_remote_command(&argv);
        let detached = Self::detach(&remote_cmd);
        let out = self
            .exec
            .run(&detached)
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success {
            return Err(HostConnectorError::Unreachable(out.stderr));
        }
        Ok(())
    }

    /// Archive a terminal remote run via `rupu workflow archive-run <run_id>`.
    async fn archive_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.remote_workflow(&["archive-run", run_id]).await
    }

    /// Restore an archived remote run via `rupu workflow restore-run <run_id>`.
    async fn restore_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.remote_workflow(&["restore-run", run_id]).await
    }

    /// Permanently delete a remote run via
    /// `rupu workflow delete-run <run_id> --force` (the CLI requires
    /// `--force` to confirm; this connector always passes it since this IS
    /// the confirmed delete path, one hop removed). The remote CLI's own
    /// `delete-run` now re-checks terminal status itself
    /// (`delete_run_with_store` in `cmd/workflow.rs`, mirroring the LOCAL
    /// branch's `delete_run_checked` guard) and refuses — even under
    /// `--force` — to remove a non-terminal run's directory out from under a
    /// process still writing it. `remote_workflow` classifies that refusal's
    /// stderr as `HostConnectorError::Invalid` (409), not `Unreachable`.
    async fn delete_run(&self, run_id: &str) -> Result<(), HostConnectorError> {
        self.remote_workflow(&["delete-run", run_id, "--force"])
            .await
    }

    async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError> {
        mirror_stream_run_events(&self.run_store, &self.host_id, run_id).await
    }

    async fn get_transcript(&self, path: &str) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
    }

    fn local_transcript_path(&self, recorded: &Path) -> PathBuf {
        crate::host::transcript_paths::cache_path(&self.global_dir(), &self.host_id, recorded)
            .unwrap_or_else(|| recorded.to_path_buf())
    }

    async fn pull_transcript(
        &self,
        run_id: &str,
        recorded: &Path,
        terminal: bool,
    ) -> Result<(), HostConnectorError> {
        let cache = self.authorize_remote_transcript(run_id, recorded)?;
        // A live tail is already filling this cache from byte zero, and holds
        // an append handle on its inode. Pulling would `rename` a fresh file
        // over that dentry, stranding the feed on an unlinked inode: the SSE
        // stream goes permanently quiet, and because `alive()` stays true,
        // every later subscriber joins the same zombie. The web panel mounts a
        // GET and a stream for the same path, so this is the common case, not
        // an edge one. Step aside — the handler serves the file the feed fills.
        if self.lazy.has_live_feed(&cache) {
            return Ok(());
        }
        let remote = recorded
            .to_str()
            .ok_or_else(|| HostConnectorError::Invalid("non-UTF-8 transcript path".into()))?;
        if remote.contains('\0') {
            return Err(HostConnectorError::Invalid(
                "transcript path contains NUL".into(),
            ));
        }
        let out = self
            .exec
            .run(&single_cat_command(remote))
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success && out.stdout.is_empty() {
            return Err(HostConnectorError::Unreachable(format!(
                "host {} did not answer: {}",
                self.host_id,
                out.stderr.trim()
            )));
        }
        let body = if out.stdout.trim_end() == NO_FILE_SENTINEL {
            String::new()
        } else {
            out.stdout
        };
        write_cache_file(&cache, &body, terminal)
    }

    async fn ensure_transcript_feed(
        &self,
        run_id: &str,
        recorded: &Path,
    ) -> Result<FeedGuard, HostConnectorError> {
        let cache = self.authorize_remote_transcript(run_id, recorded)?;
        let remote = recorded
            .to_str()
            .ok_or_else(|| HostConnectorError::Invalid("non-UTF-8 transcript path".into()))?;
        // Same guard `pull_transcript` applies: a NUL truncates the path in
        // the shell command we are about to build.
        if remote.contains('\0') {
            return Err(HostConnectorError::Invalid(
                "transcript path contains NUL".into(),
            ));
        }
        match self.lazy.subscribe(remote, &cache).await {
            Ok(Some(handle)) => Ok(FeedGuard::holding(Box::new(handle))),
            Ok(None) => Ok(FeedGuard::noop()),
            Err(e) => Err(HostConnectorError::Unreachable(e.to_string())),
        }
    }

    /// SSH/Tunnel/Bucket runs are created in, and tailed into, the
    /// coordinator's own `RunStore` by `NodeMirror`, so run-scoped detail
    /// endpoints read that mirror instead of the wire.
    fn serves_runs_from_local_mirror(&self) -> bool {
        true
    }

    /// SSH's `launch_agent` passes `AgentLaunchRequest.run_id` through to the
    /// remote as `rupu run --run-id <id>`, so a coordinator-minted id is the
    /// id the run actually executes under. The tunnel and bucket connectors
    /// mint their own, which is why this is separate from
    /// `serves_runs_from_local_mirror`.
    fn honours_supplied_run_id(&self) -> bool {
        true
    }

    async fn proxy_get_json(
        &self,
        _path_and_query: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        Err(HostConnectorError::Invalid(
            "proxy_get_json is not supported for ssh hosts".into(),
        ))
    }

    /// Enumerate remote sessions by shelling `rupu session list --format json`
    /// over `ssh` (sessions aren't mirrored to a local store the way runs are).
    /// Returns the `rows` array from the CLI report.
    async fn list_sessions(
        &self,
        scope: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let mut argv: Vec<String> = vec![
            "rupu".into(),
            "session".into(),
            "list".into(),
            "--format".into(),
            "json".into(),
        ];
        // The CLI lists active sessions by default; `--archived` restricts
        // to the archived scope. "active"/None → default (no flag).
        if let Some("archived") = scope {
            argv.push("--archived".into());
        }
        let cmd = build_remote_command(&argv);
        let out = self
            .exec
            .run(&cmd)
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        if !out.success {
            return Err(HostConnectorError::Unreachable(out.stderr));
        }
        let parsed: serde_json::Value = serde_json::from_str(out.stdout.trim()).map_err(|e| {
            HostConnectorError::Remote(0, format!("parse `rupu session list` output: {e}"))
        })?;
        Ok(parsed
            .get("rows")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Archive an active remote session via `rupu session archive <id>`.
    /// Fetch one session's detail by shelling `rupu session show <id>
    /// --format json`, renaming the report's human-table field labels to the
    /// API's. Sessions aren't mirrored to a local store the way runs are, so
    /// this is a real remote read.
    ///
    /// No `usage` block: this connector carries no pricing config (see
    /// [`SshHostConnector::new`]), so the caller prices the session from the
    /// token counts reported here.
    async fn get_session(&self, id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let item = self.session_show_item(id).await?;
        Ok(session_item_to_api_shape(&item))
    }

    /// The session's runs, read out of the same `session show` report
    /// [`get_session`](Self::get_session) uses — one ssh round trip. The two
    /// routes are separate HTTP requests, so a detail page that loads both
    /// costs two; sessions are not mirrored locally, so there is nothing
    /// cheaper to read.
    async fn session_runs(&self, id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let item = self.session_show_item(id).await?;
        Ok(item
            .get("runs")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new())))
    }

    /// Read one run's flow records by shelling `rupu netflow show
    /// <run_id> --format json`. The remote performs the same
    /// ledger+transcript merge the CP does locally (both call
    /// `rupu_cp::api::netflow::run_scoped_flows_and_dropped`); enrichment
    /// and filtering happen on our side.
    async fn run_netflow(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        let report = self
            .remote_json(&["--format", "json", "netflow", "show", run_id])
            .await
            .map_err(|e| {
                tracing::warn!(
                    host_id = %self.host_id,
                    run_id = %run_id,
                    error = %e,
                    "run_netflow: remote command failed; host may predate it"
                );
                HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu netflow show`: {}",
                    self.host_id,
                    Self::remote_failure_detail(&e)
                ))
            })?;
        Ok(serde_json::json!({
            "flows": report.get("rows").cloned().unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
            // A dropped record is data the sink could not write; carrying the
            // count is the only trace it existed. Never fold it into zero.
            "dropped_total": report.get("dropped_total").and_then(|v| v.as_u64()).unwrap_or(0),
        }))
    }

    /// Roll up this host's usage by shelling `rupu usage --format json`.
    ///
    /// The remote prices with ITS own config — the same property the HTTP
    /// path has, since that proxies the remote's `/api/usage`.
    async fn usage_rollup(
        &self,
        since: &str,
        until: &str,
        group_by: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        self.remote_json(&[
            "--format",
            "json",
            "usage",
            "--since",
            since,
            "--until",
            until,
            "--group-by",
            group_by,
        ])
        .await
    }

    /// One ssh round trip: the remote CLI walks the session's transcripts
    /// and returns the finished series, rather than this connector fetching
    /// each run's transcript separately.
    async fn session_usage_timeline(
        &self,
        id: &str,
    ) -> Result<serde_json::Value, HostConnectorError> {
        let rows = self
            .remote_json_rows(&["--format", "json", "session", "usage-timeline", id])
            .await
            .map_err(|e| {
                // The remote RAN the command and said the session is absent:
                // that is a 404, not "this host cannot answer". Without this
                // arm a plain missing session was reported as an
                // out-of-date remote — false, and it sends an operator to
                // upgrade a host that is already current.
                if Self::says_session_absent(&e.to_string(), id) {
                    return HostConnectorError::NotFound(id.to_string());
                }
                tracing::warn!(
                    host_id = %self.host_id,
                    session_id = %id,
                    error = %e,
                    "session_usage_timeline: remote command failed; host may predate it"
                );
                HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu session usage-timeline`: {}",
                    self.host_id,
                    Self::remote_failure_detail(&e)
                ))
            })?;
        Ok(serde_json::Value::Array(rows))
    }

    async fn archive_session(&self, id: &str) -> Result<(), HostConnectorError> {
        self.remote_session(&["archive", id]).await
    }

    /// Restore an archived remote session via `rupu session restore <id>`.
    async fn restore_session(&self, id: &str) -> Result<(), HostConnectorError> {
        self.remote_session(&["restore", id]).await
    }

    /// Permanently delete a remote session via
    /// `rupu session delete <id> --force`.
    async fn delete_session(&self, id: &str) -> Result<(), HostConnectorError> {
        self.remote_session(&["delete", id, "--force"]).await
    }

    /// Archive a standalone remote transcript via `rupu transcript archive
    /// <id> [--ignore-liveness]`.
    async fn archive_transcript(
        &self,
        id: &str,
        ignore_liveness: bool,
    ) -> Result<(), HostConnectorError> {
        if ignore_liveness {
            self.remote_transcript(&["archive", id, "--ignore-liveness"])
                .await
        } else {
            self.remote_transcript(&["archive", id]).await
        }
    }

    /// Permanently delete a remote transcript via
    /// `rupu transcript delete <id> --force [--ignore-liveness]`.
    async fn delete_transcript(
        &self,
        id: &str,
        ignore_liveness: bool,
    ) -> Result<(), HostConnectorError> {
        if ignore_liveness {
            self.remote_transcript(&["delete", id, "--force", "--ignore-liveness"])
                .await
        } else {
            self.remote_transcript(&["delete", id, "--force"]).await
        }
    }

    /// Standalone agent runs via `rupu transcript list --format json`, reshaped
    /// to the `AgentRunRow` wire shape. Covers standalone `rupu run` runs; it
    /// does not include session-owned runs (which the local view merges in).
    async fn list_agent_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let rows = self
            .remote_json_rows(&["transcript", "list", "--format", "json"])
            .await?;
        Ok(rows.iter().map(transcript_row_to_agent_run).collect())
    }

    /// Autoflow cycle summaries aggregated from `rupu autoflow history`.
    async fn list_autoflow_runs(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let rows = self
            .remote_json_rows(&["autoflow", "history", "--format", "json"])
            .await?;
        Ok(history_rows_to_autoflow_cycles(&rows))
    }

    /// Autoflow events via `rupu autoflow history --format json`, reshaped to
    /// the `AutoflowEventRow` wire shape.
    async fn list_autoflow_events(&self) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        let rows = self
            .remote_json_rows(&["autoflow", "history", "--format", "json"])
            .await?;
        Ok(rows.iter().map(history_row_to_autoflow_event).collect())
    }

    /// Build this host's dashboard contribution by shelling the remote CLI
    /// exactly twice: `rupu run list` and `rupu autoflow history`. Every
    /// `RemoteExec::run` spawns a fresh ssh process with a full handshake (no
    /// ControlMaster multiplexing), so this deliberately stays coarse — no
    /// per-panel round-trips.
    ///
    /// An old remote rupu without `run list` yields
    /// [`HostConnectorError::Unsupported`], never zeroed data: a host that
    /// cannot report is not a host with no runs.
    async fn dashboard_summary(
        &self,
        range: crate::host::dashboard_summary::DashboardRange,
    ) -> Result<crate::host::dashboard_summary::DashboardSummary, HostConnectorError> {
        use crate::host::dashboard_summary::*;

        let run_rows = self
            .remote_json_rows(&["--format", "json", "run", "list", "--limit", "10000"])
            .await
            .map_err(|e| {
                tracing::warn!(host_id = %self.host_id, error = %e, "dashboard_summary: run list failed");
                HostConnectorError::Unsupported(format!(
                    "remote host {} does not support `rupu run list`: {e}",
                    self.host_id
                ))
            })?;
        let cycle_rows = self.list_autoflow_runs().await.unwrap_or_else(|e| {
            // Degrade to empty, but never silently: an IO/remote failure must
            // not be indistinguishable from "this host has no cycles" (spec
            // §4.1). Matches `LocalHostConnector::dashboard_summary`'s
            // `collect_cycle_rollups` failure handling.
            tracing::warn!(host_id = %self.host_id, error = %e, "dashboard_summary: list_autoflow_runs failed; reporting no cycles");
            Vec::new()
        });
        // NOTE: `run_rows` are `RunListRow`-shaped (id / workflow_name / status /
        // started_at / finished_at / trigger / usage / turns / duration_ms).
        // `rupu run list` emits that type verbatim so remote == local by
        // construction; there is deliberately NO mapper. The id field is `id`.

        let now = chrono::Utc::now();
        let since = range.since(now);
        let in_range = |t: chrono::DateTime<chrono::Utc>| since.map(|s| t >= s).unwrap_or(true);

        // CycleCounts: `total` is the count of history-derived cycles in
        // range. `clean`/`with_failures` stay `None` — never read from
        // `ran_cycles`/`skipped_cycles`/`failed_cycles` on these rows: those
        // keys come from `history_rows_to_autoflow_cycles`, which hardcodes
        // them to the JSON literal `0` because `rupu autoflow history`'s
        // per-event stream carries no ran/skipped/failed rollup (confirmed by
        // inspecting `--format json` output — event rows have no such fields
        // at all). Reading them here would parse a fabricated zero, not a
        // reported one; this host genuinely does not know the breakdown, so
        // it must say so via `None`, not fabricate it (established during the
        // final-review I4 fix).
        let cycles_total = cycle_rows
            .iter()
            .filter(|c| {
                c.get("started_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| in_range(t.with_timezone(&chrono::Utc)))
                    .unwrap_or(false)
            })
            .count() as u64;

        let mut active = ActiveCounts::default();
        let mut terminal_buckets: std::collections::BTreeMap<
            chrono::DateTime<chrono::Utc>,
            TerminalBucket,
        > = Default::default();
        let mut throughput_buckets: std::collections::BTreeMap<
            chrono::DateTime<chrono::Utc>,
            ThroughputBucket,
        > = Default::default();
        // The RUNNING run with the OLDEST started_at, tracked as (run_id,
        // workflow_name, started_at) so `age_ms` can be computed once at the
        // end against a single `now`. Deliberately narrower than "every
        // non-terminal row" — see the matching comment in
        // `summary_build::build_summary`: this field pairs with
        // `active.running` on the "Active now" tile, so an older
        // awaiting/paused/pending row must never win it over a running one.
        let mut longest: Option<(String, String, chrono::DateTime<chrono::Utc>)> = None;

        for row in &run_rows {
            let (Some(id), Some(status), Some(started)) = (
                // `id`, NOT `run_id` — see note above.
                row.get("id").and_then(|v| v.as_str()),
                row.get("status").and_then(|v| v.as_str()),
                row.get("started_at").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(started) else {
                continue;
            };
            let started_at = started_at.with_timezone(&chrono::Utc);
            if !in_range(started_at) {
                continue;
            }
            let trigger = row
                .get("trigger")
                .and_then(|v| v.as_str())
                .unwrap_or("manual");
            let workflow_name = row
                .get("workflow_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            match status {
                "running" => active.running += 1,
                "awaiting_approval" => active.awaiting_approval += 1,
                "paused" => active.paused += 1,
                "pending" => active.pending += 1,
                _ => {}
            }

            let terminal = matches!(status, "completed" | "failed" | "rejected" | "cancelled");
            if status == "running" {
                longest = Some(match longest {
                    // The current candidate started at or before this row —
                    // it is the same age or older, so it stays the
                    // longest-running candidate.
                    Some((lid, lname, lstarted)) if lstarted <= started_at => {
                        (lid, lname, lstarted)
                    }
                    _ => (id.to_string(), workflow_name.clone(), started_at),
                });
            }
            if terminal {
                // Truncate to midnight-UTC through the SAME `day_key` the
                // local connector and `fill_bucket_grid` use. Keying on a
                // `String` day but stamping `ts` with the raw `started_at`
                // (the pre-fix behavior) meant `ts` never equalled a
                // midnight fill-grid cursor, so this host's buckets were
                // silently dropped after merging with any other host's —
                // see the regression test in `api::dashboard`.
                let key = crate::host::summary_build::day_key(started_at);
                let b = terminal_buckets.entry(key).or_insert(TerminalBucket {
                    ts: key,
                    completed: 0,
                    failed: 0,
                    rejected: 0,
                    cancelled: 0,
                });
                match status {
                    "completed" => b.completed += 1,
                    "failed" => b.failed += 1,
                    "rejected" => b.rejected += 1,
                    "cancelled" => b.cancelled += 1,
                    _ => {}
                }
            }

            // Throughput: every run in range counts once, keyed by the day it
            // STARTED and by trigger — same day-key alignment as
            // `terminal_buckets`, and matching the local connector: a
            // still-running run counts here even though it never reaches
            // `terminal_buckets`.
            let tkey = crate::host::summary_build::day_key(started_at);
            let tb = throughput_buckets.entry(tkey).or_insert(ThroughputBucket {
                ts: tkey,
                manual: 0,
                cron: 0,
                event: 0,
            });
            match trigger {
                "cron" => tb.cron += 1,
                "event" => tb.event += 1,
                _ => tb.manual += 1,
            }
        }

        let active_longest = longest.map(|(run_id, workflow_name, started_at)| ActiveLongest {
            run_id,
            workflow_name,
            age_ms: (now - started_at).num_milliseconds().max(0) as u64,
        });

        Ok(DashboardSummary {
            active,
            active_longest,
            // Deliberately NOT zero-filled here (unlike the local connector):
            // this host emits only the days it actually saw activity for, and
            // the fleet-wide zero-fill happens once, after the merge, in
            // `api::dashboard::merge_dashboard_summaries` — the only place
            // that has visibility into every host's range.
            terminal_buckets: terminal_buckets.into_values().collect(),
            throughput_buckets: throughput_buckets.into_values().collect(),
            cycles: CycleCounts {
                total: cycles_total,
                clean: None,
                with_failures: None,
            },
            // Findings are not exposed by the CLI. `None`, NOT `Some(0)` —
            // this host does not report findings at all, and `Some(0)` would
            // be indistinguishable from a genuine zero-findings host once
            // summed at the aggregation layer. `api::dashboard` sums only
            // `Some` values and flags the aggregate `findings_partial` when
            // any reporting host contributed `None`.
            findings_open: None,
            // SSH shells to the remote `rupu` CLI, which has no fleet-inventory
            // surface. Report nothing rather than zeros — the merge raises
            // `fleet_partial` so the strip says so.
            fleet: crate::host::dashboard_summary::FleetCounts::default(),
            captured_at: now,
        })
    }

    // ── Workspace sync ─────────────────────────────────────────────────────────
    //
    // The wire-encoded payload/delta are shipped as raw stdin/stdout bytes to
    // the remote `rupu __workspace` helper via `RemoteExec::run_bytes`, which
    // runs the codec via the *remote* `rupu` binary — the host needs no
    // git/tar of its own, and the bytes never pass through a lossy UTF-8
    // decode. Only the single trailing "working dir" line printed by `stage`
    // is text, so it alone goes through `from_utf8_lossy`.
    async fn stage_workspace(&self, payload: Vec<u8>) -> Result<String, HostConnectorError> {
        if payload.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                payload.len()
            )));
        }
        let cmd = build_remote_command(&["rupu".into(), "__workspace".into(), "stage".into()]);
        let out = self
            .exec
            .run_bytes(&cmd, Some(payload))
            .await
            .map_err(map_remote_err)?;
        let line = String::from_utf8_lossy(&out);
        let dir = line.trim();
        if dir.is_empty() {
            return Err(HostConnectorError::Invalid(
                "remote stage returned no working dir".into(),
            ));
        }
        // Remote output enters the process here; check it once, at the
        // boundary, rather than trusting it at each later point of use.
        validate_staged_working_dir(dir)?;
        Ok(dir.to_string())
    }

    async fn collect_workspace_delta(
        &self,
        working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        let cmd = build_remote_command(&[
            "rupu".into(),
            "__workspace".into(),
            "collect".into(),
            working_dir.to_string(),
        ]);
        let bytes = self
            .exec
            .run_bytes(&cmd, None)
            .await
            .map_err(map_remote_err)?;
        if bytes.len() > MAX_WORKSPACE_BYTES {
            return Err(HostConnectorError::Invalid(format!(
                "workspace delta {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    async fn discard_workspace(&self, working_dir: &str) -> Result<(), HostConnectorError> {
        let cmd = build_remote_command(&[
            "rupu".into(),
            "__workspace".into(),
            "discard".into(),
            working_dir.to_string(),
        ]);
        self.exec
            .run_bytes(&cmd, None)
            .await
            .map_err(map_remote_err)?;
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_wraps_and_escapes_quotes() {
        assert_eq!(shell_escape("plain"), "'plain'");
        assert_eq!(shell_escape("a b"), "'a b'");
        assert_eq!(shell_escape("it's"), r#"'it'\''s'"#);
        assert_eq!(shell_escape("a;rm -rf /"), "'a;rm -rf /'");
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
    }

    // Minor finding: a load-bearing safety refusal shelled over SSH must
    // classify as `Invalid` (→ 409 downstream), not `Unreachable` (→ 500).
    #[test]
    fn classify_remote_cli_failure_recognizes_session_ownership_refusal() {
        let err = classify_remote_cli_failure(
            "Error: transcript run_x is managed by session ses_1; use `rupu session archive|delete` instead",
        );
        assert!(
            matches!(err, HostConnectorError::Invalid(m) if m.contains("is managed by session"))
        );
    }

    #[test]
    fn classify_remote_cli_failure_recognizes_non_terminal_refusal() {
        let err = classify_remote_cli_failure(
            "Error: run run_x is not terminal (running) — cancel it first",
        );
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }

    #[test]
    fn classify_remote_cli_failure_recognizes_liveness_refusal() {
        let err = classify_remote_cli_failure("Error: transcript run_x appears to be running");
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }

    #[test]
    fn classify_remote_cli_failure_falls_back_to_unreachable() {
        let err =
            classify_remote_cli_failure("ssh: connect to host edge port 22: Connection refused");
        assert!(matches!(err, HostConnectorError::Unreachable(_)));
    }

    #[test]
    fn build_remote_command_escapes_each_token() {
        let argv = vec![
            "rupu".to_string(),
            "workflow".to_string(),
            "run".to_string(),
            "my workflow".to_string(),
            "--run-id".to_string(),
            "run_1".to_string(),
        ];
        assert_eq!(
            build_remote_command(&argv),
            "'rupu' 'workflow' 'run' 'my workflow' '--run-id' 'run_1'"
        );
    }

    #[test]
    fn ssh_argv_includes_flags_in_order() {
        let argv = ssh_argv(
            "deploy@edge",
            Some(2222),
            Some(std::path::Path::new("/k/id")),
            "'true'",
            SHORT_CALL_CONNECT_TIMEOUT_SECS,
        );
        // BatchMode present as two args: -o BatchMode=yes
        assert!(argv.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(argv.iter().any(|a| a == "-i") && argv.iter().any(|a| a == "/k/id"));
        assert!(argv.iter().any(|a| a == "-p") && argv.iter().any(|a| a == "2222"));
        assert_eq!(argv.last().unwrap(), "'true'");
        let pos_host = argv.iter().position(|a| a == "deploy@edge").unwrap();
        let pos_cmd = argv.len() - 1;
        assert!(pos_host < pos_cmd, "host must precede the remote command");
    }

    #[test]
    fn ssh_argv_omits_optional_flags() {
        let argv = ssh_argv(
            "edge",
            None,
            None,
            "'true'",
            SHORT_CALL_CONNECT_TIMEOUT_SECS,
        );
        assert!(!argv.iter().any(|a| a == "-i"));
        assert!(!argv.iter().any(|a| a == "-p"));
        assert!(argv.iter().any(|a| a == "edge"));
    }

    /// R5: the short request/response calls (`run` / `run_bytes`, backing
    /// `remote_json_rows` / `remote_json_item` / `remote_workflow` / `info`)
    /// must carry a bounded `ConnectTimeout` so a dead host resolves in ~3s
    /// instead of stalling the dashboard fan-out — the SSH analogue of the
    /// HTTP connector's 5s/30s bound.
    #[test]
    fn ssh_argv_short_call_uses_tight_connect_timeout() {
        let argv = ssh_argv(
            "edge",
            None,
            None,
            "'true'",
            SHORT_CALL_CONNECT_TIMEOUT_SECS,
        );
        assert!(argv.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "ConnectTimeout=3"),
            "short-call argv must set ConnectTimeout=3, got: {argv:?}"
        );
    }

    /// R5: the launch-path pump's long-lived `tail -F` ssh (`spawn_lines`,
    /// called only from `spawn_tail_pump`) is a streaming connection meant to
    /// stay open, not a probe — it must NOT be tightened to the short-call
    /// bound. It keeps its own (more generous) connect timeout.
    #[test]
    fn ssh_argv_pump_call_does_not_use_short_call_timeout() {
        let argv = ssh_argv("edge", None, None, "'tail -F x'", PUMP_CONNECT_TIMEOUT_SECS);
        assert!(
            !argv
                .windows(2)
                .any(|w| w[0] == "-o" && w[1] == "ConnectTimeout=3"),
            "pump argv must not carry the tightened short-call ConnectTimeout, got: {argv:?}"
        );
        assert_eq!(PUMP_CONNECT_TIMEOUT_SECS, 10);
    }

    #[test]
    fn parse_tail_marker_extracts_path() {
        assert_eq!(
            parse_tail_marker("==> /r/run_1/events.jsonl <=="),
            Some("/r/run_1/events.jsonl")
        );
        assert_eq!(parse_tail_marker(r#"{"some":"json"}"#), None);
        assert_eq!(parse_tail_marker(""), None);
    }

    #[tokio::test]
    async fn run_bytes_pipes_stdin_and_returns_stdout_bytes() {
        let exec = FakeExec::with_bytes_ok(b"DELTA".to_vec());
        let out = exec
            .run_bytes("rupu __workspace stage", Some(b"PAYLOAD".to_vec()))
            .await
            .expect("ok");
        assert_eq!(out, b"DELTA");
        let (cmd, stdin) = exec.last_bytes_call.lock().unwrap().clone().unwrap();
        assert_eq!(cmd, "rupu __workspace stage");
        assert_eq!(stdin.as_deref(), Some(&b"PAYLOAD"[..]));
    }

    #[tokio::test]
    async fn run_bytes_nonzero_exit_is_error() {
        let exec = FakeExec::with_bytes_err(RemoteExecError::NonZero {
            code: Some(2),
            stderr: "boom".into(),
        });
        let err = exec
            .run_bytes("rupu __workspace collect /x", None)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RemoteExecError::NonZero { code: Some(2), .. }
        ));
    }

    // ── SshHostConnector tests ────────────────────────────────────────────────

    use crate::host::connector::HostConnectorError;

    struct FakeExec {
        commands: std::sync::Mutex<Vec<String>>,
        tail_lines: Vec<String>,
        fail: bool,
        fail_stderr: String,
        /// If set, returned as stdout when `run()` is called for a `cat …`
        /// command on a `runs/<id>/…` path (i.e. `run.json`).
        cat_stdout: Option<String>,
        /// If set, returned as stdout for a `cat …/transcripts/<id>.jsonl`
        /// command (the pump's terminal transcript catch-up). `None` → empty
        /// stdout, which the pump treats as "no transcript".
        cat_transcript_stdout: Option<String>,
        /// If set, the transcript `cat` sleeps this long before answering —
        /// a slow remote, so a teardown test can catch the pump mid-`cat`.
        cat_transcript_delay: Option<std::time::Duration>,
        /// If set, returned as stdout for the pump's batched terminal pull
        /// (`for p in …; do printf '==> %s <==' …; cat …; done`, Task 6).
        batch_cat_stdout: Option<String>,
        /// If set, returned as stdout for a `rupu … run show <id>` command
        /// (`get_run`), so a test can drive the dispatcher's poll sequence.
        show_stdout: Option<String>,
        /// If set, returned as stdout for a `cat …/runs/<id>/launch.log`
        /// command (`launch_diagnostics`) — the remote process's captured
        /// stderr. `None` → empty stdout, i.e. a launch that wrote nothing.
        launch_log_stdout: Option<String>,
        /// If set, returned as stdout for the run-start evidence probe
        /// (`remote_start_evidence_cmd`). `None` → empty stdout, which the
        /// probe classifies as `Unknown` — a host that did not answer.
        evidence_stdout: Option<String>,
        /// Scripted result for `run_bytes`, taken on first call.
        run_bytes_out: std::sync::Mutex<Option<Result<Vec<u8>, RemoteExecError>>>,
        /// Records the `(remote_command, stdin)` of the last `run_bytes` call.
        last_bytes_call: std::sync::Mutex<Option<(String, Option<Vec<u8>>)>>,
    }

    impl FakeExec {
        fn ok(tail_lines: Vec<String>) -> Self {
            Self {
                commands: Default::default(),
                tail_lines,
                fail: false,
                fail_stderr: String::new(),
                cat_stdout: None,
                cat_transcript_stdout: None,
                cat_transcript_delay: None,
                batch_cat_stdout: None,
                show_stdout: None,
                launch_log_stdout: None,
                evidence_stdout: None,
                run_bytes_out: Default::default(),
                last_bytes_call: Default::default(),
            }
        }

        fn offline(stderr: impl Into<String>) -> Self {
            Self {
                commands: Default::default(),
                tail_lines: vec![],
                fail: true,
                fail_stderr: stderr.into(),
                cat_stdout: None,
                cat_transcript_stdout: None,
                cat_transcript_delay: None,
                batch_cat_stdout: None,
                show_stdout: None,
                launch_log_stdout: None,
                evidence_stdout: None,
                run_bytes_out: Default::default(),
                last_bytes_call: Default::default(),
            }
        }

        /// Variant for tail-pump tests: success dispatch, canned tail stream,
        /// and a canned `cat run.json` response.
        fn with_cat_stdout(tail_lines: Vec<String>, cat_stdout: impl Into<String>) -> Self {
            Self {
                commands: Default::default(),
                tail_lines,
                fail: false,
                fail_stderr: String::new(),
                cat_stdout: Some(cat_stdout.into()),
                cat_transcript_stdout: None,
                cat_transcript_delay: None,
                batch_cat_stdout: None,
                show_stdout: None,
                launch_log_stdout: None,
                evidence_stdout: None,
                run_bytes_out: Default::default(),
                last_bytes_call: Default::default(),
            }
        }

        /// Variant for `run_bytes` tests: scripts a successful stdout-bytes
        /// response.
        fn with_bytes_ok(bytes: Vec<u8>) -> Self {
            Self {
                commands: Default::default(),
                tail_lines: vec![],
                fail: false,
                fail_stderr: String::new(),
                cat_stdout: None,
                cat_transcript_stdout: None,
                cat_transcript_delay: None,
                batch_cat_stdout: None,
                show_stdout: None,
                launch_log_stdout: None,
                evidence_stdout: None,
                run_bytes_out: std::sync::Mutex::new(Some(Ok(bytes))),
                last_bytes_call: Default::default(),
            }
        }

        /// Variant for `run_bytes` tests: scripts a failing response.
        fn with_bytes_err(err: RemoteExecError) -> Self {
            Self {
                commands: Default::default(),
                tail_lines: vec![],
                fail: false,
                fail_stderr: String::new(),
                cat_stdout: None,
                cat_transcript_stdout: None,
                cat_transcript_delay: None,
                batch_cat_stdout: None,
                show_stdout: None,
                launch_log_stdout: None,
                evidence_stdout: None,
                run_bytes_out: std::sync::Mutex::new(Some(Err(err))),
                last_bytes_call: Default::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl RemoteExec for FakeExec {
        async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
            self.commands.lock().unwrap().push(remote.to_string());
            if self.fail {
                Ok(RemoteOutput {
                    stdout: String::new(),
                    stderr: self.fail_stderr.clone(),
                    success: false,
                })
            } else {
                // Return the canned stdout for the two `cat` shapes the pump
                // issues: `cat …/transcripts/<id>.jsonl` (terminal transcript
                // catch-up) vs `cat …/runs/<id>/run.json` (status poll).
                let stdout = if remote.starts_with("if [ -s ") {
                    self.evidence_stdout.clone().unwrap_or_default()
                } else if remote.contains("'show'") {
                    self.show_stdout.clone().unwrap_or_default()
                } else if remote.starts_with("for p in ") {
                    self.batch_cat_stdout.clone().unwrap_or_default()
                } else if remote.starts_with("cat ") {
                    if remote.contains("/launch.log") {
                        self.launch_log_stdout.clone().unwrap_or_default()
                    } else if remote.contains("/transcripts/") {
                        if let Some(d) = self.cat_transcript_delay {
                            tokio::time::sleep(d).await;
                        }
                        self.cat_transcript_stdout.clone().unwrap_or_default()
                    } else {
                        self.cat_stdout.clone().unwrap_or_default()
                    }
                } else {
                    String::new()
                };
                Ok(RemoteOutput {
                    stdout,
                    stderr: String::new(),
                    success: true,
                })
            }
        }

        fn spawn_lines(&self, remote: &str) -> Result<LineStream, RemoteExecError> {
            self.commands.lock().unwrap().push(remote.to_string());
            let lines: Vec<std::io::Result<String>> =
                self.tail_lines.iter().cloned().map(Ok).collect();
            // Chain a forever-pending tail to simulate real `tail -F`, which
            // never exits on its own.  The pump must terminate via the
            // cat-poll interval, not stream-end.
            let stream = futures_util::stream::iter(lines)
                .chain(futures_util::stream::pending::<std::io::Result<String>>());
            Ok(Box::pin(stream))
        }

        async fn run_bytes(
            &self,
            remote_command: &str,
            stdin: Option<Vec<u8>>,
        ) -> Result<Vec<u8>, RemoteExecError> {
            *self.last_bytes_call.lock().unwrap() = Some((remote_command.to_string(), stdin));
            self.run_bytes_out
                .lock()
                .unwrap()
                .take()
                .expect("run_bytes_out not scripted")
        }
    }

    fn make_conn<E: RemoteExec + 'static>(
        fake: std::sync::Arc<E>,
    ) -> (
        SshHostConnector,
        std::sync::Arc<rupu_orchestrator::RunStore>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let run_store =
            std::sync::Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let mirror = std::sync::Arc::new(crate::node::NodeMirror::new(std::sync::Arc::clone(
            &run_store,
        )));
        let exec: std::sync::Arc<dyn RemoteExec> = fake;
        let conn =
            SshHostConnector::new("host_abc", exec, mirror, std::sync::Arc::clone(&run_store));
        (conn, run_store, tmp)
    }

    #[tokio::test]
    async fn list_sessions_shells_rupu_session_list_and_parses_rows() {
        struct StubExec {
            json: String,
            last_cmd: std::sync::Mutex<String>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                *self.last_cmd.lock().unwrap() = remote.to_string();
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by list_sessions")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by list_sessions")
            }
        }

        let json = r#"{"kind":"session_list","version":1,"rows":[
            {"session_id":"ses_1","agent":"oracle-assessor","scope":"active","status":"idle"},
            {"session_id":"ses_2","agent":"rupuso","scope":"active","status":"failed"}
        ]}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            last_cmd: std::sync::Mutex::new(String::new()),
        });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        // Active scope → shells `rupu session list --format json` (no --archived).
        let rows = conn.list_sessions(Some("active")).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["session_id"], "ses_1");
        let cmd = stub.last_cmd.lock().unwrap().clone();
        assert!(
            cmd.contains("session") && cmd.contains("list") && cmd.contains("json"),
            "cmd: {cmd}"
        );
        assert!(
            !cmd.contains("--archived"),
            "active scope must not pass --archived: {cmd}"
        );

        // Archived scope → adds --archived.
        conn.list_sessions(Some("archived")).await.unwrap();
        assert!(stub.last_cmd.lock().unwrap().contains("--archived"));
    }

    /// `rupu session show` reports an absent session as "unknown session:
    /// <id>", not "not found". Matching only the latter would report a
    /// genuinely-missing session as "this host cannot answer" (500) instead
    /// of a 404.
    #[tokio::test]
    async fn get_session_maps_the_cli_unknown_session_message_to_not_found() {
        let fake = std::sync::Arc::new(FakeExec::offline("unknown session: ses_gone"));
        let (conn, _store, _tmp) = make_conn(fake);

        let err = conn.get_session("ses_gone").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn run_netflow_shells_the_remote_command_and_carries_dropped() {
        struct StubExec {
            json: String,
            last_cmd: std::sync::Mutex<String>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                *self.last_cmd.lock().unwrap() = remote.to_string();
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by run_netflow")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by run_netflow")
            }
        }

        let json = r#"{"kind":"netflow_show","version":1,"dropped_total":7,"rows":[
            {"host":"api.anthropic.com"},{"host":"api.github.com"}
        ]}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            last_cmd: std::sync::Mutex::new(String::new()),
        });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let v = conn.run_netflow("run_a").await.unwrap();

        assert_eq!(v["flows"].as_array().unwrap().len(), 2);
        assert_eq!(
            v["dropped_total"], 7,
            "a dropped record is data the sink lost; the count must survive"
        );
        let cmd = stub.last_cmd.lock().unwrap().clone();
        assert!(
            cmd.contains("netflow") && cmd.contains("show") && cmd.contains("run_a"),
            "cmd: {cmd}"
        );
    }

    /// A run with no ledger reports an empty set, and a host that cannot
    /// answer at all reports Unsupported — the two must not look alike.
    #[tokio::test]
    async fn run_netflow_maps_an_old_host_to_unsupported() {
        let fake = std::sync::Arc::new(FakeExec::offline("unrecognized subcommand 'show'"));
        let (conn, _store, _tmp) = make_conn(fake);

        let err = conn.run_netflow("run_a").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[tokio::test]
    async fn session_usage_timeline_shells_the_remote_command_once() {
        struct StubExec {
            json: String,
            calls: std::sync::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                self.calls.lock().unwrap().push(remote.to_string());
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by session_usage_timeline")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by session_usage_timeline")
            }
        }

        let json = r#"{"kind":"session_usage_timeline","version":1,"rows":[
            {"turn":1,"label":"run_a","tokens_in":10,"tokens_out":20,"tokens_cached":0},
            {"turn":2,"label":"run_a","tokens_in":5,"tokens_out":7,"tokens_cached":0}
        ]}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let series = conn.session_usage_timeline("ses_1").await.unwrap();

        assert_eq!(series.as_array().unwrap().len(), 2);
        assert_eq!(series[1]["label"], "run_a");
        let calls = stub.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "the series is computed remotely in ONE round trip, not one per run"
        );
        assert!(
            calls[0].contains("usage-timeline") && calls[0].contains("ses_1"),
            "cmd: {}",
            calls[0]
        );
    }

    /// Caught live against a real host running the CURRENT binary: the
    /// remote ran `session usage-timeline`, said "unknown session: <id>",
    /// and the CP reported "remote host does not support `rupu session
    /// usage-timeline`" — false, and it sends an operator to upgrade a host
    /// that is already up to date. An absent session is a 404.
    #[tokio::test]
    async fn session_usage_timeline_maps_an_absent_session_to_not_found() {
        let fake = std::sync::Arc::new(FakeExec::offline("[error] unknown session: ses_gone"));
        let (conn, _store, _tmp) = make_conn(fake);

        let err = conn.session_usage_timeline("ses_gone").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::NotFound(_)),
            "expected NotFound, got {err:?}"
        );
    }

    /// The two conditions are ANDed deliberately: a generic failure that
    /// happens to contain the phrase is not a statement about THIS session,
    /// and must not be laundered into a 404.
    #[test]
    fn says_session_absent_requires_both_the_phrase_and_the_id() {
        assert!(SshHostConnector::says_session_absent(
            "[error] unknown session: ses_1",
            "ses_1"
        ));
        assert!(SshHostConnector::says_session_absent(
            "not found: ses_1",
            "ses_1"
        ));
        assert!(
            !SshHostConnector::says_session_absent("unknown session: ses_other", "ses_1"),
            "a different session's absence says nothing about this one"
        );
        assert!(
            !SshHostConnector::says_session_absent("permission denied for ses_1", "ses_1"),
            "an unrelated failure must not become a 404"
        );
    }

    /// A host whose `rupu` predates `session usage-timeline` must say so,
    /// not report an empty chart as if the session had no turns.
    /// A host that ANSWERED and rejected the command is not an unreachable
    /// host. `remote_json` maps a non-zero exit to `Unreachable`, so the
    /// reported reason must not carry that prefix onward — it would point an
    /// operator at the network instead of at the out-of-date binary.
    #[tokio::test]
    async fn an_old_hosts_reason_does_not_claim_the_host_is_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline(
            "error: unrecognized subcommand 'usage-timeline'",
        ));
        let (conn, _store, _tmp) = make_conn(fake);

        let err = conn.session_usage_timeline("ses_1").await.unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains("unrecognized subcommand"),
            "the remote's own words must survive: {message}"
        );
        assert!(
            !message.contains("host unreachable"),
            "the host answered; do not call it unreachable: {message}"
        );
    }

    #[tokio::test]
    async fn session_usage_timeline_maps_an_old_host_to_unsupported() {
        let fake = std::sync::Arc::new(FakeExec::offline("unrecognized subcommand"));
        let (conn, _store, _tmp) = make_conn(fake);

        let err = conn.session_usage_timeline("ses_1").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[test]
    fn session_item_to_api_shape_renames_cli_labels_and_lifts_runs_out() {
        // The CLI report labels for a human table (`agent`, `provider`); the
        // API labels for the web client. A silent drift here renders a
        // remote session's detail page with blank fields and no error.
        let item = serde_json::json!({
            "session_id": "ses_1",
            "agent": "scout",
            "provider": "anthropic",
            "model": "opus",
            "scope": "archived",
            "runs": [{"run_id": "run_a"}],
        });

        let v = session_item_to_api_shape(&item);

        assert_eq!(v["agent_name"], "scout", "agent -> agent_name");
        assert_eq!(v["provider_name"], "anthropic", "provider -> provider_name");
        assert!(v.get("agent").is_none(), "old label must not linger");
        assert_eq!(v["model"], "opus", "untouched keys pass through");
        assert_eq!(v["scope"], "archived", "scope is carried, not defaulted");
        assert!(
            v.get("runs").is_none(),
            "runs belong to the session_runs surface, not the detail body"
        );
    }

    #[test]
    fn session_item_to_api_shape_defaults_a_missing_scope_to_active() {
        let v = session_item_to_api_shape(&serde_json::json!({ "session_id": "ses_2" }));
        assert_eq!(v["scope"], "active");
    }

    #[tokio::test]
    async fn get_session_shells_rupu_session_show_and_returns_the_item() {
        struct StubExec {
            json: String,
            last_cmd: std::sync::Mutex<String>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                *self.last_cmd.lock().unwrap() = remote.to_string();
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by get_session")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by get_session")
            }
        }

        let json = r#"{"kind":"session_show","version":1,"item":{
            "session_id":"ses_1","agent":"scout","scope":"active","status":"idle",
            "provider":"anthropic","model":"opus","total_turns":3,
            "total_tokens_in":10,"total_tokens_out":20,
            "created_at":"2026-09-01T00:00:00Z","updated_at":"2026-09-01T01:00:00Z",
            "runs":[{"run_id":"run_a","transcript_path":"/t/run_a.jsonl"}]
        }}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            last_cmd: std::sync::Mutex::new(String::new()),
        });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let detail = conn.get_session("ses_1").await.unwrap();
        assert_eq!(detail["session_id"], "ses_1");
        assert_eq!(detail["agent_name"], "scout", "returned in API shape");
        assert!(
            detail.get("usage").is_none(),
            "this connector carries no pricing; the caller prices the session"
        );

        // The same report also answers `session_runs` — no extra round trip.
        let runs = conn.session_runs("ses_1").await.unwrap();
        assert_eq!(runs[0]["run_id"], "run_a");

        let cmd = stub.last_cmd.lock().unwrap().clone();
        assert!(
            cmd.contains("session") && cmd.contains("show") && cmd.contains("ses_1"),
            "cmd: {cmd}"
        );
    }

    /// A remote whose `rupu` predates `session show --format json` must be
    /// reported as unable to answer — never as "the session does not exist",
    /// which would render an empty page as if it were the truth.
    #[tokio::test]
    async fn get_session_maps_an_old_host_to_unsupported_not_not_found() {
        let fake = std::sync::Arc::new(FakeExec::offline("unrecognized subcommand 'show'"));
        let (conn, _store, _tmp) = make_conn(fake);

        let err = conn.get_session("ses_missing").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[tokio::test]
    async fn list_runs_shells_rupu_run_list_not_the_mirror() {
        struct StubExec {
            json: String,
            last_cmd: std::sync::Mutex<String>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                *self.last_cmd.lock().unwrap() = remote.to_string();
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by list_runs")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by list_runs")
            }
        }

        // RunListRow-shaped stub (the CLI's real `run list --format json`
        // contract, since Task 5) — `id`, `usage`, `turns`, `duration_ms`,
        // not the old lossy mapper's shape.
        let json = r#"{"kind":"run_list","version":1,"rows":[
            {"id":"run_a","workflow_name":"nightly","status":"completed",
             "started_at":"2026-07-16T14:02:11Z","finished_at":"2026-07-16T14:09:02Z",
             "trigger":"cron",
             "usage":{"input_tokens":100,"output_tokens":50,"cached_tokens":0,
                      "total_tokens":150,"cost_usd":0.01,"priced":true,"runs":1},
             "turns":3,"duration_ms":410000}
        ],"summary":{"count":1,"limit":10000,"status_filter":null}}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            last_cmd: std::sync::Mutex::new(String::new()),
        });
        // The mirror is EMPTY — this is the point. Before the fix, list_runs
        // read the mirror and would return zero rows here.
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let rows = conn
            .list_runs(RunListQuery {
                kind: RunKind::All,
                offset: 0,
                limit: 100,
                lifecycle: None,
            })
            .await
            .unwrap();

        assert_eq!(
            rows.len(),
            1,
            "must return the CLI's row, not the empty mirror"
        );
        assert_eq!(rows[0]["id"], "run_a");
        assert_eq!(
            rows[0]["trigger"], "cron",
            "trigger must survive — cycle grouping depends on it"
        );

        let cmd = stub.last_cmd.lock().unwrap().clone();
        assert!(
            cmd.contains("run") && cmd.contains("list") && cmd.contains("json"),
            "must shell `rupu run list --format json`: {cmd}"
        );
    }

    #[tokio::test]
    async fn list_runs_rows_carry_usage_and_turns() {
        // The web UI reads r.usage.input_tokens UNGUARDED and App.tsx has a
        // single top-level ErrorBoundary — a row without `usage` blanks the
        // whole app. These fields are not optional. Regression test for the
        // deleted `run_list_row_to_wire` mapper, which omitted them.
        struct StubExec {
            json: String,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, _remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by list_runs")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by list_runs")
            }
        }

        let json = r#"{"kind":"run_list","version":1,"rows":[
            {"id":"run_b","workflow_name":"deploy","status":"completed",
             "started_at":"2026-07-16T09:00:00Z","finished_at":"2026-07-16T09:05:00Z",
             "trigger":"manual",
             "usage":{"input_tokens":1200,"output_tokens":800,"cached_tokens":100,
                      "total_tokens":2000,"cost_usd":5.25,"priced":true,"runs":1},
             "turns":7,"duration_ms":300000}
        ],"summary":{"count":1,"limit":10000,"status_filter":null}}"#;
        let stub = std::sync::Arc::new(StubExec { json: json.into() });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let rows = conn
            .list_runs(RunListQuery {
                kind: RunKind::All,
                offset: 0,
                limit: 100,
                lifecycle: None,
            })
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert!(
            !rows[0]["usage"].is_null(),
            "usage must be present, not null/missing: {:?}",
            rows[0]
        );
        assert_eq!(rows[0]["usage"]["input_tokens"], 1200);
        assert_eq!(
            rows[0]["turns"], 7,
            "turns must be present and non-zero: {:?}",
            rows[0]
        );
        assert_eq!(rows[0]["duration_ms"], 300000);
    }

    #[tokio::test]
    async fn get_run_shells_rupu_run_show_not_the_mirror() {
        struct StubExec {
            json: String,
            last_cmd: std::sync::Mutex<String>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                *self.last_cmd.lock().unwrap() = remote.to_string();
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
        }

        let json = r#"{"kind":"run_show","version":1,"item":{"id":"run_a","status":"completed"}}"#;
        let stub = std::sync::Arc::new(StubExec {
            json: json.into(),
            last_cmd: std::sync::Mutex::new(String::new()),
        });
        // Mirror is EMPTY — before the fix this returned NotFound, because
        // get_run read the mirror (populated only by `spawn_tail_pump`, which
        // never saw this run).
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let item = conn.get_run("run_a").await.unwrap();

        assert_eq!(
            item["id"], "run_a",
            "must return the CLI's `item` payload, not the empty mirror"
        );
        assert_eq!(item["status"], "completed");

        let cmd = stub.last_cmd.lock().unwrap().clone();
        assert!(
            cmd.contains("run") && cmd.contains("show") && cmd.contains("json"),
            "must shell `rupu run show --format json`: {cmd}"
        );
    }

    #[tokio::test]
    async fn get_run_maps_old_host_failure_to_unsupported_not_not_found() {
        struct FailExec;
        #[async_trait::async_trait]
        impl RemoteExec for FailExec {
            async fn run(&self, _remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                // An old remote rupu has no `run show`; `classify` treats it as
                // "launch an agent named show", which fails to load and exits
                // nonzero.
                Ok(RemoteOutput {
                    stdout: String::new(),
                    stderr: "error: agent 'show' not found".into(),
                    success: false,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
        }
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(FailExec));

        let err = conn.get_run("run_a").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unsupported(_)),
            "an old-host failure must map to Unsupported, never NotFound — NotFound \
             would be indistinguishable from \"this run does not exist\": {err:?}"
        );
    }

    #[tokio::test]
    async fn get_run_maps_a_remote_not_found_to_not_found() {
        // The remote explicitly said the run is absent — believe it. Reporting
        // Unsupported here would claim the HOST is broken when it answered
        // correctly.
        struct StubExec;
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, _remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                Ok(RemoteOutput {
                    stdout: String::new(),
                    stderr: "[error] run run_x: run `run_x` not found".into(),
                    success: false,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
        }
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(StubExec));

        let err = conn.get_run("run_x").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::NotFound(_)),
            "the remote explicitly reported the run as not found — this must \
             surface as NotFound, not Unsupported: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_run_maps_an_old_host_to_unsupported_not_not_found() {
        // An old rupu lacking `run show` must NOT look like "the run does not
        // exist" — that would silently hide a run that is really there.
        struct StubExec;
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, _remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                Ok(RemoteOutput {
                    stdout: String::new(),
                    stderr: "run does not support `--format json` (supported: `table`)".into(),
                    success: false,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!("not used by get_run")
            }
        }
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(StubExec));

        let err = conn.get_run("run_x").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unsupported(_)),
            "an old host's format-gate rejection must map to Unsupported, not \
             NotFound — NotFound here would hide a run that really exists: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_runs_preserves_rfc3339_for_merge_sort() {
        // rupu-cp's fan_out merge does a LEXICOGRAPHIC string compare on
        // started_at. A space-separated timestamp (' ' = 0x20 < 'T' = 0x54)
        // would sort every remote row after every local row at the same
        // instant. Guard the format.
        struct StubExec {
            json: String,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, _remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                Ok(RemoteOutput {
                    stdout: self.json.clone(),
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }
        let json = r#"{"kind":"run_list","version":1,"rows":[
            {"id":"run_a","workflow_name":"w","status":"completed",
             "started_at":"2026-07-16T14:02:11Z","finished_at":null,"trigger":"manual",
             "usage":{"input_tokens":0,"output_tokens":0,"cached_tokens":0,
                      "total_tokens":0,"cost_usd":null,"priced":false,"runs":1},
             "turns":0,"duration_ms":null}
        ],"summary":{"count":1,"limit":1,"status_filter":null}}"#;
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(StubExec { json: json.into() }));
        let rows = conn
            .list_runs(RunListQuery {
                kind: RunKind::All,
                offset: 0,
                limit: 100,
                lifecycle: None,
            })
            .await
            .unwrap();
        let started = rows[0]["started_at"].as_str().unwrap();
        assert!(
            started.contains('T'),
            "started_at must stay RFC-3339: {started}"
        );
    }

    #[tokio::test]
    async fn ssh_dashboard_summary_sets_captured_at_and_tallies_active() {
        struct StubExec {
            runs_json: String,
            cycles_json: String,
            cmds: std::sync::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                self.cmds.lock().unwrap().push(remote.to_string());
                let stdout = if remote.contains("autoflow") {
                    self.cycles_json.clone()
                } else {
                    self.runs_json.clone()
                };
                Ok(RemoteOutput {
                    stdout,
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }

        // `run_id` here would silently zero out every row: `rupu run list`
        // emits `RunListRow` verbatim, whose id field is `id`, not `run_id`.
        // r2 (awaiting_approval) is deliberately the OLDER of the two rows —
        // this is the real-data regression: an older awaiting-approval run
        // must never win `active_longest` over a younger running run, since
        // this field pairs with `active.running` on the "Active now" tile.
        let runs_json = r#"{"kind":"run_list","version":1,"rows":[
            {"id":"r1","workflow_name":"w","status":"running",
             "started_at":"__R1_STARTED__","finished_at":null,"trigger":"manual",
             "usage":{"input_tokens":0,"output_tokens":0,"cached_tokens":0,
                      "total_tokens":0,"cost_usd":null,"priced":false,"runs":1},
             "turns":0,"duration_ms":null},
            {"id":"r2","workflow_name":"w","status":"awaiting_approval",
             "started_at":"__R2_STARTED__","finished_at":null,"trigger":"cron",
             "usage":{"input_tokens":0,"output_tokens":0,"cached_tokens":0,
                      "total_tokens":0,"cost_usd":null,"priced":false,"runs":1},
             "turns":0,"duration_ms":null}
        ],"summary":{"count":2,"limit":10000,"status_filter":null}}"#;
        // Anchored to `now`: the query below asks for `Days30`, so hard-coded
        // fixture dates silently age out of the window and the test starts
        // failing on a calendar date rather than on a code change. `r1` is the
        // older of the two so it stays the `active_longest` pick.
        let r1_started = chrono::Utc::now() - chrono::Duration::hours(2);
        let r2_started = chrono::Utc::now() - chrono::Duration::hours(1);
        let runs_json = runs_json
            .replace(
                "__R1_STARTED__",
                &r1_started.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            )
            .replace(
                "__R2_STARTED__",
                &r2_started.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            );
        let cycles_json = r#"{"kind":"autoflow_history","version":1,"rows":[]}"#;

        let stub = std::sync::Arc::new(StubExec {
            runs_json,
            cycles_json: cycles_json.into(),
            cmds: std::sync::Mutex::new(Vec::new()),
        });
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&stub));

        let before = chrono::Utc::now();
        let s = conn
            .dashboard_summary(crate::host::dashboard_summary::DashboardRange::Days30)
            .await
            .unwrap();

        assert_eq!(s.active.running, 1);
        assert_eq!(s.active.awaiting_approval, 1);
        let al = s
            .active_longest
            .expect("one running run in hand — expected an active_longest");
        assert_eq!(
            al.run_id, "r1",
            "r1 is the only RUNNING row; r2 (awaiting_approval) is older but must \
             never win active_longest — that field pairs with active.running"
        );
        let total_manual: u64 = s.throughput_buckets.iter().map(|b| b.manual).sum();
        let total_cron: u64 = s.throughput_buckets.iter().map(|b| b.cron).sum();
        assert_eq!(total_manual, 1, "r1 is manual-triggered");
        assert_eq!(total_cron, 1, "r2 is cron-triggered");
        assert_eq!(
            s.cycles.clean, None,
            "SSH cannot report the clean/with-failures breakdown — must stay None, never a fabricated 0"
        );
        assert_eq!(s.cycles.with_failures, None);
        assert!(
            s.captured_at >= before,
            "captured_at must be stamped when the host was actually read"
        );
        assert_eq!(
            s.findings_open, None,
            "SSH has no findings surface — this must be None, never a fabricated Some(0)"
        );
    }

    #[tokio::test]
    async fn ssh_dashboard_summary_truncates_terminal_bucket_ts_to_midnight() {
        // Regression test for the SSH-bucket-drop bug (C1): the connector used
        // to key its bucket map by day-string but stamp `ts` with the RAW
        // `started_at` of the first terminal run seen that day. `ts` then
        // never equalled a midnight fill-grid cursor
        // (`summary_build::fill_bucket_grid`), so this host's terminal counts
        // were silently dropped after merging with any other host's
        // (`api::dashboard::merge_dashboard_summaries`).
        struct StubExec {
            runs_json: String,
        }
        #[async_trait::async_trait]
        impl RemoteExec for StubExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                let stdout = if remote.contains("autoflow") {
                    r#"{"kind":"autoflow_history","version":1,"rows":[]}"#.to_string()
                } else {
                    self.runs_json.clone()
                };
                Ok(RemoteOutput {
                    stdout,
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }

        // A completed run at a raw, decidedly non-midnight timestamp.
        //
        // Anchored to `now` rather than a frozen date: the query below asks for
        // `Days30`, so a hard-coded fixture silently ages out of the window and
        // the test starts failing on a calendar date rather than on a code
        // change. Two days back is safely inside the window while still being a
        // different day from "today" in every timezone.
        let started = (chrono::Utc::now() - chrono::Duration::days(2))
            .date_naive()
            .and_hms_opt(13, 47, 22)
            .expect("13:47:22 is a valid time")
            .and_utc();
        let finished = started + chrono::Duration::minutes(3);
        let runs_json = format!(
            r#"{{"kind":"run_list","version":1,"rows":[
            {{"id":"r1","workflow_name":"w","status":"completed",
             "started_at":"{}","finished_at":"{}","trigger":"manual",
             "usage":{{"input_tokens":0,"output_tokens":0,"cached_tokens":0,
                      "total_tokens":0,"cost_usd":null,"priced":false,"runs":1}},
             "turns":0,"duration_ms":null}}
        ],"summary":{{"count":1,"limit":10000,"status_filter":null}}}}"#,
            started.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            finished.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(StubExec { runs_json }));

        let s = conn
            .dashboard_summary(crate::host::dashboard_summary::DashboardRange::Days30)
            .await
            .unwrap();

        assert_eq!(
            s.terminal_buckets.len(),
            1,
            "expected exactly one terminal bucket, got {:?}",
            s.terminal_buckets
        );
        let expected_midnight = started
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight is a valid time")
            .and_utc();
        assert_eq!(
            s.terminal_buckets[0].ts, expected_midnight,
            "bucket ts must be midnight-truncated, not the raw started_at \
             ({started}) — a non-midnight ts never matches a \
             fill-grid cursor and is silently dropped"
        );
        assert_eq!(s.terminal_buckets[0].completed, 1);

        // The throughput bucket for the same run must be midnight-truncated
        // too — the same C1-class bug (a non-midnight ts silently dropped at
        // merge) applies equally to `throughput_buckets`.
        assert_eq!(s.throughput_buckets.len(), 1);
        assert_eq!(s.throughput_buckets[0].ts, expected_midnight);
        assert_eq!(s.throughput_buckets[0].manual, 1);
    }

    #[test]
    fn transcript_row_maps_to_agent_run_shape() {
        let row = serde_json::json!({
            "run_id": "run_1", "scope": "active", "title": null,
            "agent": "oracle-enumerator-glm", "status": "rejected",
            "total_tokens": 42, "started_at": "2026-07-02 00:15:04"
        });
        let m = transcript_row_to_agent_run(&row);
        assert_eq!(m["run_id"], "run_1");
        assert_eq!(m["source"], "standalone");
        assert_eq!(m["agent"], "oracle-enumerator-glm");
        assert_eq!(m["status"], "rejected");
        assert_eq!(m["usage"]["total_tokens"], 42);
        assert_eq!(m["turns"], 0);
        assert!(m["session_id"].is_null());
        assert!(m["duration_ms"].is_null());
    }

    #[test]
    fn history_row_maps_to_autoflow_event_shape() {
        let row = serde_json::json!({
            "at": "2026-05-14T22:58:15Z", "cycle_id": "afc_1", "mode": "serve",
            "worker": "matt@host", "event": "wake_consumed",
            "issue": "github:o/r/issues/20", "source": "-", "workflow": "-",
            "repo": "github:o/r", "run": "-", "wake": "wake_9", "detail": "cronpoll"
        });
        let m = history_row_to_autoflow_event(&row);
        assert_eq!(m["event_id"], "wake_9");
        assert_eq!(m["cycle_id"], "afc_1");
        assert_eq!(m["kind"], "wake_consumed");
        assert_eq!(m["issue_display_ref"], "github:o/r/issues/20");
        assert!(m["workflow"].is_null(), "dash → null");
        assert!(m["run_id"].is_null(), "dash → null");
        assert_eq!(m["worker_name"], "matt@host");

        // No wake → synthesized stable event_id from cycle_id:at.
        let row2 = serde_json::json!({
            "at": "2026-05-14T22:58:15Z", "cycle_id": "afc_2", "event": "cycle_started", "wake": "-"
        });
        assert_eq!(
            history_row_to_autoflow_event(&row2)["event_id"],
            "afc_2:2026-05-14T22:58:15Z"
        );
    }

    #[test]
    fn history_rows_aggregate_into_cycles() {
        let rows = vec![
            serde_json::json!({"at":"2026-05-14T10:00:00Z","cycle_id":"afc_1","mode":"serve","worker":"w","event":"cycle_started","workflow":"wf-a","run":"run_1"}),
            serde_json::json!({"at":"2026-05-14T10:05:00Z","cycle_id":"afc_1","mode":"serve","worker":"w","event":"run_finished","workflow":"wf-b","run":"run_2"}),
            serde_json::json!({"at":"2026-05-14T09:00:00Z","cycle_id":"afc_2","mode":"serve","worker":"w","event":"cycle_started","workflow":"-","run":"-"}),
        ];
        let cycles = history_rows_to_autoflow_cycles(&rows);
        assert_eq!(cycles.len(), 2);
        let c1 = cycles.iter().find(|c| c["cycle_id"] == "afc_1").unwrap();
        assert_eq!(c1["started_at"], "2026-05-14T10:00:00Z");
        assert_eq!(c1["finished_at"], "2026-05-14T10:05:00Z");
        assert_eq!(c1["workflow_count"], 2);
        assert_eq!(c1["run_ids"].as_array().unwrap().len(), 2);
        let c2 = cycles.iter().find(|c| c["cycle_id"] == "afc_2").unwrap();
        assert_eq!(c2["workflow_count"], 0);
        assert_eq!(c2["run_ids"].as_array().unwrap().len(), 0);
        assert_eq!(c2["ran_cycles"], 0);
    }

    #[tokio::test]
    async fn info_reports_remote_rupu_version() {
        struct VerExec;
        #[async_trait::async_trait]
        impl RemoteExec for VerExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                let stdout = if remote.contains("--version") {
                    "rupu 0.35.2\n".to_string()
                } else {
                    String::new()
                };
                Ok(RemoteOutput {
                    stdout,
                    stderr: String::new(),
                    success: true,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(VerExec));
        let info = conn.info().await.unwrap();
        assert!(info.reachable);
        assert_eq!(info.version.as_deref(), Some("0.35.2"));
    }

    #[tokio::test]
    async fn info_reachable_but_version_none_when_rupu_missing() {
        // `true` succeeds (ssh works) but `rupu --version` exits nonzero.
        struct NoRupuExec;
        #[async_trait::async_trait]
        impl RemoteExec for NoRupuExec {
            async fn run(&self, remote: &str) -> Result<RemoteOutput, RemoteExecError> {
                let success = !remote.contains("--version");
                Ok(RemoteOutput {
                    stdout: String::new(),
                    stderr: if success {
                        String::new()
                    } else {
                        "rupu: command not found".into()
                    },
                    success,
                })
            }
            fn spawn_lines(&self, _r: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!()
            }
            async fn run_bytes(
                &self,
                _c: &str,
                _s: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                unimplemented!()
            }
        }
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(NoRupuExec));
        let info = conn.info().await.unwrap();
        assert!(info.reachable, "ssh works even if rupu is missing");
        assert!(info.version.is_none());
    }

    #[tokio::test]
    async fn launch_run_mints_creates_mirror_and_dispatches() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let run_id = conn
            .launch_run(crate::launcher::LaunchRequest {
                workflow: "deploy".into(),
                inputs: Default::default(),
                mode: Some("bypass".into()),
                target: None,
                working_dir: None,
            })
            .await
            .unwrap();

        assert!(run_id.starts_with("run_"), "run_id must start with run_");

        // Mirror run exists, attributed to host_abc.
        let rec = run_store.load(&run_id).unwrap();
        assert_eq!(rec.worker_id.as_deref(), Some("host_abc"));

        // Dispatched a detached remote `rupu workflow run … --run-id <id> --plain`.
        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'run'")
                && c.contains(&format!("'{run_id}'"))
                && c.contains("'--plain'")),
            "dispatch command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter()
                .any(|c| c.contains("setsid") || c.contains("nohup")),
            "command must be wrapped for detachment: {cmds:?}"
        );
    }

    /// Seed a mirrored run owned by `host_abc` whose step_results claim
    /// `remote` — the shape every allowlist test starts from.
    fn seed_claimed_run(
        conn: &SshHostConnector,
        run_store: &rupu_orchestrator::RunStore,
        run_id: &str,
        remote: &str,
        terminal: bool,
    ) {
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "wf".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();
        run_store
            .append_step_result(
                run_id,
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "build".into(),
                    run_id: "run_01STEPX".into(),
                    transcript_path: std::path::PathBuf::from(remote),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: Default::default(),
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                    host: None,
                },
            )
            .unwrap();
        if terminal {
            conn.mirror
                .finish(run_id, &conn.host_id, "completed")
                .unwrap();
        }
    }

    #[test]
    fn ssh_local_transcript_path_maps_into_the_host_cache() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let recorded = std::path::Path::new("/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl");
        assert_eq!(
            conn.local_transcript_path(recorded),
            tmp.path()
                .join("mirror/host_abc/transcripts/run_01STEP.jsonl")
        );
        // A non-transcript path maps to itself, so the handler's "is it a
        // local file" check fails and the request falls to the run-scoped
        // branch, which rejects it.
        let odd = std::path::Path::new("/etc/passwd");
        assert_eq!(conn.local_transcript_path(odd), odd.to_path_buf());
    }

    #[tokio::test]
    async fn ssh_pull_transcript_requires_the_run_to_claim_the_path() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01OWNED", claimed, false);

        let other = std::path::Path::new("/home/ci/proj/.rupu/transcripts/run_01OTHER.jsonl");
        let err = conn
            .pull_transcript("run_01OWNED", other, false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Invalid(ref m) if m.contains("did not record")),
            "{err}"
        );

        let err = conn
            .pull_transcript("run_01MISSING", std::path::Path::new(claimed), false)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::NotFound(_)), "{err}");

        // Nothing was shelled for a refused pull.
        assert!(fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .all(|c| !c.starts_with("cat ")));
    }

    #[tokio::test]
    async fn ssh_pull_transcript_rejects_a_run_owned_by_another_host() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01FOREIGN", claimed, false);
        let mut rec = run_store.load("run_01FOREIGN").unwrap();
        rec.worker_id = Some("host_other".into());
        run_store.update(&rec).unwrap();

        let err = conn
            .pull_transcript("run_01FOREIGN", std::path::Path::new(claimed), false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Invalid(ref m) if m.contains("does not belong")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn ssh_pull_transcript_writes_cache_and_marks_complete_only_when_terminal() {
        let mut fake = FakeExec::ok(vec![]);
        fake.cat_transcript_stdout =
            Some("{\"type\":\"run_start\"}\n{\"type\":\"turn_start\"}\n".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01STEP.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01PULL", claimed, false);
        let cache = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01STEP.jsonl");

        conn.pull_transcript("run_01PULL", std::path::Path::new(claimed), false)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n{\"type\":\"turn_start\"}\n"
        );
        assert!(
            !crate::host::transcript_paths::is_complete(&cache),
            "non-terminal pull is a snapshot"
        );

        conn.pull_transcript("run_01PULL", std::path::Path::new(claimed), true)
            .await
            .unwrap();
        assert!(crate::host::transcript_paths::is_complete(&cache));
        // Two sequential pulls, two distinct (ULID-named) tmp files, both
        // renamed away — a fixed `{cache}.tmp` would be shared by concurrent
        // pulls and could splice two bodies into one renamed-in file.
        let strays: Vec<_> = std::fs::read_dir(cache.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "stray tmp files left behind: {strays:?}");

        let cmds = fake.commands.lock().unwrap();
        let cat = cmds
            .iter()
            .find(|c| c.starts_with("cat "))
            .expect("a cat was issued");
        assert!(
            cat.contains(&format!("'{claimed}'")),
            "path must be single-quoted: {cat}"
        );
        assert!(
            cat.contains("__RUPU_NO_FILE__"),
            "absent-file sentinel present: {cat}"
        );
    }

    #[tokio::test]
    async fn ssh_pull_transcript_absent_remote_file_is_an_empty_complete_answer() {
        let mut fake = FakeExec::ok(vec![]);
        fake.cat_transcript_stdout = Some("__RUPU_NO_FILE__\n".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01NONE.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01ABSENT", claimed, false);
        let cache = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01NONE.jsonl");

        conn.pull_transcript("run_01ABSENT", std::path::Path::new(claimed), true)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "");
        assert!(crate::host::transcript_paths::is_complete(&cache));
    }

    #[tokio::test]
    async fn ssh_pull_transcript_offline_is_unreachable_and_leaves_no_cache() {
        let fake = std::sync::Arc::new(FakeExec::offline(
            "ssh: connect to host mini port 22: No route",
        ));
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01OFF.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01OFFLINE", claimed, false);

        let err = conn
            .pull_transcript("run_01OFFLINE", std::path::Path::new(claimed), true)
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Unreachable(_)), "{err}");
        assert!(!tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01OFF.jsonl")
            .exists());
    }

    /// Poll a cache file until `needle` shows up (or give up after ~1s).
    async fn wait_for_cache(cache: &std::path::Path, needle: &str) {
        for _ in 0..100 {
            if std::fs::read_to_string(cache)
                .map(|s| s.contains(needle))
                .unwrap_or(false)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{needle:?} never appeared in {}", cache.display());
    }

    #[tokio::test]
    async fn ssh_ensure_transcript_feed_tails_the_claimed_path_into_the_cache() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![r#"{"type":"run_start"}"#.into()]));
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01FEED.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01FEEDRUN", claimed, false);
        let cache = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01FEED.jsonl");

        let guard = conn
            .ensure_transcript_feed("run_01FEEDRUN", std::path::Path::new(claimed))
            .await
            .unwrap();
        wait_for_cache(&cache, "run_start").await;
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n"
        );
        {
            let cmds = fake.commands.lock().unwrap();
            assert!(
                cmds.iter()
                    .any(|c| c == &format!("tail -n +1 -F '{claimed}'")),
                "{cmds:?}"
            );
        }
        drop(guard);

        let err = match conn
            .ensure_transcript_feed(
                "run_01FEEDRUN",
                std::path::Path::new("/home/ci/proj/.rupu/transcripts/run_01X.jsonl"),
            )
            .await
        {
            Ok(_) => panic!("expected an Invalid error for an unclaimed path"),
            Err(e) => e,
        };
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }

    /// C1: the web transcript panel mounts a GET and an SSE stream for the
    /// same path. The stream's feed holds an append handle on the cache's
    /// inode; the GET's `pull_transcript` used to `rename` a freshly written
    /// file over that dentry, stranding the feed on an unlinked inode — the
    /// stream went permanently quiet and, because `alive()` stayed true, every
    /// later subscriber joined the same zombie. A pull now steps aside.
    #[tokio::test]
    async fn ssh_pull_transcript_steps_aside_for_a_live_feed() {
        let mut fake = FakeExec::ok(vec![r#"{"type":"run_start"}"#.into()]);
        fake.cat_transcript_stdout = Some("THIS MUST NEVER REACH THE CACHE\n".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        let claimed = "/home/ci/proj/.rupu/transcripts/run_01LIVE.jsonl";
        seed_claimed_run(&conn, &run_store, "run_01LIVERUN", claimed, false);
        let cache = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01LIVE.jsonl");

        let _guard = conn
            .ensure_transcript_feed("run_01LIVERUN", std::path::Path::new(claimed))
            .await
            .unwrap();
        wait_for_cache(&cache, "run_start").await;
        let before = std::fs::read_to_string(&cache).unwrap();
        let cats_before = fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("cat "))
            .count();

        // Ok(()) — the handler then serves the file the feed is filling.
        conn.pull_transcript("run_01LIVERUN", std::path::Path::new(claimed), false)
            .await
            .unwrap();

        let cats_after = fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("cat "))
            .count();
        assert_eq!(cats_before, cats_after, "no new `cat` was shelled");
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            before,
            "the feed's cache was left untouched"
        );
        assert!(
            !crate::host::transcript_paths::is_complete(&cache),
            "stepping aside never marks the cache authoritative"
        );
    }

    /// C1, other half: the TERMINAL pull is authoritative and must still win
    /// over a live feed — but by rewriting the SAME inode rather than renaming
    /// a new file over it, so the feed's append handle stays valid.
    #[tokio::test]
    async fn terminal_pull_rewrites_a_live_feeds_cache_in_place() {
        use std::os::unix::fs::MetadataExt as _;

        let claimed = "/home/ci/proj/.rupu/transcripts/run_01INPLACE.jsonl";
        let mut fake = FakeExec::ok(vec![r#"{"type":"run_start"}"#.into()]);
        fake.batch_cat_stdout = Some(format!(
            "==> {claimed} <==\n{{\"type\":\"run_start\"}}\n{{\"type\":\"run_complete\"}}\n\n==> end <==\n"
        ));
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));
        seed_claimed_run(&conn, &run_store, "run_01INPLACERUN", claimed, false);
        let cache = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01INPLACE.jsonl");

        let _guard = conn
            .ensure_transcript_feed("run_01INPLACERUN", std::path::Path::new(claimed))
            .await
            .unwrap();
        wait_for_cache(&cache, "run_start").await;
        let inode_before = std::fs::metadata(&cache).unwrap().ino();

        pump_pull_step_transcripts(
            fake.as_ref(),
            &conn.mirror,
            &conn.lazy,
            "run_01INPLACERUN",
            "host_abc",
        )
        .await;

        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "{\"type\":\"run_start\"}\n{\"type\":\"run_complete\"}\n",
            "the authoritative body wins"
        );
        assert!(
            crate::host::transcript_paths::is_complete(&cache),
            "and is marked complete"
        );
        assert_eq!(
            std::fs::metadata(&cache).unwrap().ino(),
            inode_before,
            "same inode: the live feed's append fd still points at the file \
             readers open"
        );
    }

    #[tokio::test]
    async fn cancel_approve_reject_issue_remote_commands() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        // run_id only needs to be a valid shell token for the assertion;
        // cancel/approve/reject never touch the local store.
        let run_id = "run_01TESTCONTROLOK";

        conn.cancel_run(run_id).await.unwrap();
        conn.approve_run(run_id, "bypass").await.unwrap();
        conn.reject_run(run_id, Some("nope")).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'cancel'")
                && c.contains(&format!("'{run_id}'"))),
            "cancel command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'approve'")
                && c.contains(&format!("'{run_id}'"))
                && c.contains("'--mode'")
                && c.contains("'bypass'")),
            "approve command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'reject'")
                && c.contains(&format!("'{run_id}'"))
                && c.contains("'--reason'")
                && c.contains("'nope'")),
            "reject command not found in: {cmds:?}"
        );
    }

    #[test]
    fn ssh_serves_runs_from_local_mirror() {
        // SSH runs are created in, and tailed into, the coordinator's own
        // RunStore by NodeMirror, so run-scoped detail endpoints must read
        // that mirror rather than attempt a generic GET this transport
        // structurally cannot serve (see `proxy_get_json`).
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(FakeExec::ok(vec![])));
        assert!(conn.serves_runs_from_local_mirror());
    }

    #[tokio::test]
    async fn ssh_pause_run_invokes_remote() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let run_id = "run_01TESTPAUSEOK";

        conn.pause_run(run_id).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'pause'")
                && c.contains(&format!("'{run_id}'"))),
            "pause command not found in: {cmds:?}"
        );
    }

    #[tokio::test]
    async fn ssh_pause_run_offline_surfaces_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline("connection refused"));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn.pause_run("run_01TESTPAUSEOFFLINE").await.unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn ssh_resume_run_dispatches_detached_remote_resume() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let run_id = "run_01TESTRESUMEOK";

        conn.resume_run(run_id).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'resume'")
                && c.contains(&format!("'{run_id}'"))
                && (c.contains("setsid") || c.contains("nohup"))),
            "detached resume command not found in: {cmds:?}"
        );
    }

    #[tokio::test]
    async fn ssh_resume_run_offline_surfaces_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline("connection refused"));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn
            .resume_run("run_01TESTRESUMEOFFLINE")
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn ssh_archive_restore_delete_run_issue_remote_commands() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let run_id = "run_01TESTARCHIVEOK";

        conn.archive_run(run_id).await.unwrap();
        conn.restore_run(run_id).await.unwrap();
        conn.delete_run(run_id).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'archive-run'")
                && c.contains(&format!("'{run_id}'"))),
            "archive-run command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'restore-run'")
                && c.contains(&format!("'{run_id}'"))),
            "restore-run command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'workflow'")
                && c.contains("'delete-run'")
                && c.contains(&format!("'{run_id}'"))
                && c.contains("'--force'")),
            "delete-run command not found in: {cmds:?}"
        );
    }

    #[tokio::test]
    async fn ssh_archive_run_offline_surfaces_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline("connection refused"));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn
            .archive_run("run_01TESTARCHIVEOFFLINE")
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn ssh_archive_restore_delete_session_issue_remote_commands() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let id = "ses_01TESTARCHIVEOK";

        conn.archive_session(id).await.unwrap();
        conn.restore_session(id).await.unwrap();
        conn.delete_session(id).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'session'")
                && c.contains("'archive'")
                && c.contains(&format!("'{id}'"))),
            "session archive command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'session'")
                && c.contains("'restore'")
                && c.contains(&format!("'{id}'"))),
            "session restore command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'session'")
                && c.contains("'delete'")
                && c.contains(&format!("'{id}'"))
                && c.contains("'--force'")),
            "session delete command not found in: {cmds:?}"
        );
    }

    #[tokio::test]
    async fn ssh_archive_session_offline_surfaces_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline("connection refused"));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn
            .archive_session("ses_01TESTARCHIVEOFFLINE")
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn ssh_archive_delete_transcript_issue_remote_commands() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let id = "run_01TESTTRANSCRIPTOK";

        conn.archive_transcript(id, false).await.unwrap();
        conn.delete_transcript(id, false).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'transcript'")
                && c.contains("'archive'")
                && c.contains(&format!("'{id}'"))),
            "transcript archive command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'transcript'")
                && c.contains("'delete'")
                && c.contains(&format!("'{id}'"))
                && c.contains("'--force'")),
            "transcript delete command not found in: {cmds:?}"
        );
    }

    // PID-reuse escape hatch: `ignore_liveness: true` must append
    // `--ignore-liveness` to BOTH the archive and delete remote commands.
    #[tokio::test]
    async fn ssh_archive_delete_transcript_ignore_liveness_appends_flag() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let id = "run_01TESTTRANSCRIPTIGNORE";

        conn.archive_transcript(id, true).await.unwrap();
        conn.delete_transcript(id, true).await.unwrap();

        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter().any(|c| c.contains("'transcript'")
                && c.contains("'archive'")
                && c.contains(&format!("'{id}'"))
                && c.contains("'--ignore-liveness'")),
            "transcript archive --ignore-liveness command not found in: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| c.contains("'transcript'")
                && c.contains("'delete'")
                && c.contains(&format!("'{id}'"))
                && c.contains("'--force'")
                && c.contains("'--ignore-liveness'")),
            "transcript delete --ignore-liveness command not found in: {cmds:?}"
        );
    }

    #[tokio::test]
    async fn ssh_archive_transcript_offline_surfaces_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline("connection refused"));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn
            .archive_transcript("run_01TESTTRANSCRIPTOFFLINE", false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );
    }

    #[tokio::test]
    async fn offline_host_run_failure_surfaces_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::offline("connection refused"));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        // info() reports unreachable but does not error.
        let info = conn.info().await.unwrap();
        assert!(
            !info.reachable,
            "offline host should report reachable: false"
        );

        // launch_run maps a failed ssh dispatch to Unreachable.
        let err = conn
            .launch_run(crate::launcher::LaunchRequest {
                workflow: "deploy".into(),
                inputs: Default::default(),
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );
    }

    /// Verifies the tail pump:
    ///  1. Routes lines after a `==> …/events.jsonl <==` marker to events.jsonl.
    ///  2. Terminates via the cat-poll interval (NOT stream-end): the stream
    ///     pends forever after the finite lines, just like real `tail -F`.
    ///  3. Calls `mirror.finish` with the terminal status from `cat run.json`.
    ///
    /// `FakeExec::spawn_lines` returns `iter(lines).chain(pending())` — the
    /// stream never ends on its own.  The pump must detect termination through
    /// the `tokio::time::interval` arm that polls `cat run.json`.  The first
    /// interval tick fires immediately, so the pump completes near-instantly.
    /// The bounded poll (50 ms checks, 2 s ceiling) absorbs scheduler jitter.
    #[tokio::test]
    async fn tail_pump_routes_events_and_finishes_run() {
        let event_json = r#"{"type":"step_started","step":"s1"}"#;
        // Expanded absolute path (as the remote `tail` would emit after $HOME
        // expansion) — still ends with `events.jsonl`, so routing matches.
        let tail_lines = vec![
            "==> /home/ci/.rupu/runs/run_01TESTPUMP01/events.jsonl <==".to_string(),
            event_json.to_string(),
        ];
        let run_json = r#"{"run_id":"run_01TESTPUMP01","status":"completed"}"#;

        let fake = std::sync::Arc::new(FakeExec::with_cat_stdout(tail_lines, run_json.to_string()));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let run_id = "run_01TESTPUMP01";
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "test-wf".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        // Bounded poll: wait up to 2 s for the spawned pump task to finish
        // and flip the run status to Completed.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.status == rupu_orchestrator::RunStatus::Completed {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "timed out waiting for pump to finish; status={:?}",
                    rec.status
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // The event line must have been appended to the run's events.jsonl.
        let events_path = run_store.events_path(run_id);
        let contents = std::fs::read_to_string(&events_path).unwrap_or_default();
        assert!(
            contents.contains(event_json),
            "expected event line in events.jsonl, got: {contents:?}"
        );
    }

    /// Wait, on the PAUSED virtual clock, for the pump to move `run_id` out of
    /// `Running`. Returns the virtual time it took, or `None` if it never did
    /// within `bound`. The sleep is what lets tokio auto-advance virtual time
    /// (it jumps to the earliest pending timer, which is the pump's own
    /// `PUMP_POLL_INTERVAL` tick), so `PUMP_STARTUP_DEADLINE` resolves in
    /// milliseconds of real time.
    async fn wait_for_pump_to_finish(
        run_store: &rupu_orchestrator::RunStore,
        run_id: &str,
        bound: std::time::Duration,
    ) -> Option<std::time::Duration> {
        let t0 = tokio::time::Instant::now();
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.status != rupu_orchestrator::RunStatus::Running {
                return Some(t0.elapsed());
            }
            if t0.elapsed() >= bound {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    /// A run whose remote process died before creating ANY of its artifacts
    /// must not leave a tail pump behind.
    ///
    /// `tail -n +1 -F` never exits on its own — it retries missing paths
    /// forever — and `cat run.json` never succeeds for a run directory that
    /// was never created, so before `PUMP_STARTUP_DEADLINE` the pump had NO
    /// reachable exit: it spun indefinitely holding an ssh session and a
    /// remote `tail`. Measured in production: two such pumps outlived their
    /// runs by 3h40m and 1h39m.
    ///
    /// `FakeExec::ok(vec![])` is exactly that host: the tail stream yields no
    /// lines and then pends forever (no `==>` header, because `tail` could
    /// not open any of the four paths and its stderr goes to /dev/null), and
    /// `cat` answers empty stdout. It also leaves `evidence_stdout` unset, so
    /// the run-start probe answers nothing either — a host that cannot say
    /// whether the run started leaves the pump exactly as informed as it was
    /// before the probe existed, and the old bound stands. The pump must give
    /// up at the deadline and finish the run as failed rather than leave it
    /// `Running` forever.
    #[tokio::test(start_paused = true)]
    async fn tail_pump_abandons_a_run_that_never_appears_on_the_host() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let run_id = "run_01TESTPUMPDEAD";
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "dead-launch".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let took = wait_for_pump_to_finish(
            &run_store,
            run_id,
            PUMP_STARTUP_DEADLINE + std::time::Duration::from_secs(60),
        )
        .await
        .expect(
            "the pump must give up on a run that never appears on the host, \
             not hold its ssh session open forever",
        );

        assert!(
            took >= PUMP_STARTUP_DEADLINE && took < PUMP_STARTUP_DEADLINE + PUMP_POLL_INTERVAL * 5,
            "the pump must give up at the {}s startup deadline; took {}s",
            PUMP_STARTUP_DEADLINE.as_secs(),
            took.as_secs()
        );

        // A run that never started is a failed run, not a running one — the
        // same "never stuck in Running" contract the stream-end fallback has.
        assert_eq!(
            run_store.load(run_id).unwrap().status,
            rupu_orchestrator::RunStatus::Failed
        );

        // And the pump must have deregistered itself, so a later
        // `await_run_mirror` finds nothing to wait for.
        assert!(
            conn.pumps.lock().unwrap().get(run_id).is_none(),
            "the pump must deregister when it gives up"
        );
    }

    /// The startup deadline must bound only the NEVER-SEEN case. A run whose
    /// artifacts `tail` actually opened has demonstrably started, so the pump
    /// keeps mirroring it past the deadline — cutting it off there would be
    /// the same class of bug as the 60 s poll budget #649 removed, one layer
    /// down.
    ///
    /// Here `tail` emits a `==>` header (so the run is seen) but `run.json` is
    /// never readable, so the deadline is the ONLY thing that could end this
    /// pump. It must not.
    #[tokio::test(start_paused = true)]
    async fn tail_pump_keeps_mirroring_a_run_it_has_seen_past_the_startup_deadline() {
        let run_id = "run_01TESTPUMPSEEN";
        let fake = std::sync::Arc::new(FakeExec::ok(vec![format!(
            "==> /home/ci/.rupu/transcripts/{run_id}.jsonl <=="
        )]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "slow-but-alive".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let gave_up = wait_for_pump_to_finish(
            &run_store,
            run_id,
            PUMP_STARTUP_DEADLINE + std::time::Duration::from_secs(120),
        )
        .await;

        assert!(
            gave_up.is_none(),
            "a run the pump has already seen on the host must not be \
             abandoned at the startup deadline (gave up after {:?})",
            gave_up
        );
    }

    /// The pump's two PASSIVE signals are not proof a run started, and the
    /// startup deadline must not fire on their absence alone.
    ///
    /// `run.json` for an agent run is written only when the agent FINISHES,
    /// and the `==>` header only appears if the transcript landed at the one
    /// `$HOME/.rupu/transcripts/` path this pump tails — a staged workspace
    /// carrying its own `.rupu/` puts it somewhere else entirely. So a
    /// healthy long placed run can show NEITHER for hours. Here the host,
    /// asked directly, says the run started; the pump must keep mirroring.
    ///
    /// Without the probe this test is `tail_pump_abandons_a_run_that_never_
    /// appears_on_the_host` with a live run underneath it — which is exactly
    /// what shipped.
    #[tokio::test(start_paused = true)]
    async fn tail_pump_keeps_mirroring_a_run_the_host_says_started() {
        let mut fake = FakeExec::ok(vec![]);
        fake.evidence_stdout = Some(RUN_START_EVIDENCE_YES.into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let run_id = "run_01TESTPUMPALIVE";
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "alive-but-quiet".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let gave_up = wait_for_pump_to_finish(
            &run_store,
            run_id,
            PUMP_STARTUP_DEADLINE + std::time::Duration::from_secs(600),
        )
        .await;

        assert!(
            gave_up.is_none(),
            "a run the host reports as started must not be abandoned at the \
             startup deadline (gave up after {gave_up:?})"
        );
        assert!(
            fake.commands
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("if [ -s ")),
            "the pump must ask the host whether the run started before \
             abandoning it"
        );
    }

    /// …but "started" is not a licence to run forever. A run that started and
    /// was then killed without writing a terminal `run.json` can still end no
    /// arm of this loop, so `PUMP_MAX_WALL` is the backstop that keeps the
    /// #655 guarantee — no pump outlives every possible run — while the
    /// startup deadline stops firing on healthy work.
    #[tokio::test(start_paused = true)]
    async fn tail_pump_gives_up_at_the_wall_cap_even_after_evidence_of_a_start() {
        let mut fake = FakeExec::ok(vec![]);
        fake.evidence_stdout = Some(RUN_START_EVIDENCE_YES.into());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let run_id = "run_01TESTPUMPWALL";
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "started-then-killed".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let took = wait_for_pump_to_finish(
            &run_store,
            run_id,
            PUMP_MAX_WALL + std::time::Duration::from_secs(300),
        )
        .await
        .expect("the wall cap must eventually end a pump that never sees terminal");

        assert!(
            took >= PUMP_STARTUP_DEADLINE * 2,
            "the wall cap, not the startup deadline, must be what fired; \
             took {}s",
            took.as_secs()
        );
        assert!(
            took >= PUMP_MAX_WALL,
            "and it must be the full wall budget; took {}s",
            took.as_secs()
        );
    }

    /// The evidence probe must not be able to see its own shadow.
    ///
    /// The shell running the probe carries the pattern in its own command
    /// line, so a literal `--run-id <id>` in the `pgrep` pattern would match
    /// the probe itself and report every dead run as alive — turning the
    /// whole signal into a constant `true`. The `[-]` character class is what
    /// prevents that, and it is invisible enough to delete by accident.
    ///
    /// Equally load-bearing in the other direction: the run DIRECTORY and
    /// `launch.log` are created by `detach_launch` BEFORE the remote `rupu`
    /// runs, so neither may be evidence of anything.
    #[test]
    fn start_evidence_cmd_cannot_match_itself_or_the_launch_wrapper() {
        let cmd = remote_start_evidence_cmd("run_01ABC");

        // The self-match guard.
        assert!(
            cmd.contains("[-]-run-id run_01ABC"),
            "the pgrep pattern must use the [-] guard: {cmd}"
        );
        assert!(
            !cmd.contains("--run-id run_01ABC"),
            "a literal --run-id would make the probe match its own shell: {cmd}"
        );

        // The real evidence.
        assert!(cmd.contains("$HOME/.rupu/transcripts/run_01ABC.jsonl"));
        assert!(cmd.contains("$HOME/.rupu/runs/run_01ABC/run.json"));
        assert!(cmd.contains("$HOME/.rupu/runs/run_01ABC/events.jsonl"));
        assert!(cmd.contains("$HOME/.rupu/runs/run_01ABC/step_results.jsonl"));

        // Not evidence: the launch wrapper creates both of these itself.
        assert!(
            !cmd.contains("launch.log"),
            "launch.log is created by the launch wrapper, not by a started \
             run: {cmd}"
        );
        assert!(
            !cmd.contains("-d $HOME/.rupu/runs/run_01ABC"),
            "the run directory is created by the launch wrapper: {cmd}"
        );

        // Answers on stdout, always exit 0 — so a dead ssh is not mistaken
        // for a dead launch.
        assert!(cmd.contains(RUN_START_EVIDENCE_YES));
        assert!(cmd.contains(RUN_START_EVIDENCE_NO));
    }

    /// Only the two expected tokens are answers. Everything else — an empty
    /// reply, a shell that printed something unexpected, an ssh that never
    /// connected — is `Unknown`, never `NoTrace`: only `NoTrace` licenses a
    /// caller to blame the launch.
    #[tokio::test]
    async fn run_start_evidence_never_reads_silence_as_a_dead_launch() {
        for (answer, want) in [
            (Some(RUN_START_EVIDENCE_YES), RunStartEvidence::Started),
            (Some(RUN_START_EVIDENCE_NO), RunStartEvidence::NoTrace),
            (Some("something else entirely"), RunStartEvidence::Unknown),
            (None, RunStartEvidence::Unknown),
        ] {
            let mut fake = FakeExec::ok(vec![]);
            fake.evidence_stdout = answer.map(str::to_string);
            let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(fake));
            assert_eq!(
                conn.run_start_evidence("run_01ABC").await,
                want,
                "answer {answer:?}"
            );
        }

        // An unreachable host is not a dead launch either.
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(FakeExec::offline("no route")));
        assert_eq!(
            conn.run_start_evidence("run_01ABC").await,
            RunStartEvidence::Unknown
        );

        // And a run id that could break out of the unquoted $HOME paths is
        // never interpolated at all.
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::new(FakeExec::ok(vec![])));
        assert_eq!(
            conn.run_start_evidence("run_01ABC; rm -rf /").await,
            RunStartEvidence::Unknown
        );
    }

    /// A placed agent run's only content is its transcript, which lives
    /// OUTSIDE the run directory (`$HOME/.rupu/transcripts/<run_id>.jsonl`).
    /// The pump must (1) tail it, (2) route its lines to the mirrored
    /// transcript file — NOT misfile them into events.jsonl — and (3) leave
    /// event lines out of the transcript. On finish, the mirror synthesizes
    /// a single `"agent"` step-result row pointing at the mirrored copy so
    /// `/api/runs/:id` (steps) and the run graph can actually reach it.
    ///
    /// `cat_transcript_stdout` is deliberately unset: the mirrored content
    /// must arrive via the tail path alone, so this test isolates routing
    /// from the terminal catch-up (covered separately below).
    #[tokio::test]
    async fn tail_pump_mirrors_transcript_lines_as_transcript_not_events() {
        let run_id = "run_01TESTPUMP02";
        let event_json = r#"{"type":"step_started","step":"s1"}"#;
        let t1 = r#"{"type":"run_start","agent":"reviewer"}"#;
        let t2 = r#"{"type":"assistant_message","content":"hi"}"#;
        let tail_lines = vec![
            format!("==> /home/ci/.rupu/runs/{run_id}/events.jsonl <=="),
            event_json.to_string(),
            format!("==> /home/ci/.rupu/transcripts/{run_id}.jsonl <=="),
            t1.to_string(),
            t2.to_string(),
        ];
        let run_json =
            format!(r#"{{"run_id":"{run_id}","status":"completed","final_output":"done"}}"#);

        let fake = std::sync::Arc::new(FakeExec::with_cat_stdout(tail_lines, run_json));
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));

        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "reviewer".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.status != rupu_orchestrator::RunStatus::Running {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for pump; status={:?}", rec.status);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Transcript lines land in the mirrored transcript, next to the runs
        // dir the way the coordinator's own layout puts them.
        let transcript_path = tmp
            .path()
            .join("transcripts")
            .join(format!("{run_id}.jsonl"));
        let transcript = std::fs::read_to_string(&transcript_path).unwrap_or_default();
        assert!(
            transcript.contains(t1) && transcript.contains(t2),
            "expected transcript lines in {transcript_path:?}, got: {transcript:?}"
        );
        assert!(
            !transcript.contains(event_json),
            "event line must not be misfiled into the transcript: {transcript:?}"
        );

        // Event line lands in events.jsonl; transcript lines must not.
        let events = std::fs::read_to_string(run_store.events_path(run_id)).unwrap_or_default();
        assert!(
            events.contains(event_json),
            "expected event line in events.jsonl, got: {events:?}"
        );
        assert!(
            !events.contains(t1) && !events.contains(t2),
            "transcript lines must not be misfiled into events.jsonl: {events:?}"
        );

        // Reachability: finish synthesized exactly one "agent" step-result row
        // pointing at the mirrored copy — this is what /api/runs/:id `steps`
        // and the run graph's transcript panel read.
        let steps = run_store.read_step_results(run_id).unwrap();
        assert_eq!(steps.len(), 1, "expected one synthesized step result");
        assert_eq!(steps[0].step_id, "agent");
        assert_eq!(steps[0].transcript_path, transcript_path);
        assert!(steps[0].success);
    }

    /// A run with no transcript (the workflow case, or an agent that died
    /// before writing one) must still mirror its other artifacts — `tail -F`
    /// on a not-yet-existing path is expected, not fatal. The pump must also
    /// actually ASK for the transcript path (it's in the tail command), or
    /// the whole feature is silently absent.
    #[tokio::test]
    async fn tail_pump_without_transcript_still_mirrors_other_artifacts() {
        let run_id = "run_01TESTPUMP03";
        let event_json = r#"{"type":"step_started","step":"s1"}"#;
        let tail_lines = vec![
            format!("==> /home/ci/.rupu/runs/{run_id}/events.jsonl <=="),
            event_json.to_string(),
        ];
        let run_json = format!(r#"{{"run_id":"{run_id}","status":"completed"}}"#);

        let fake = std::sync::Arc::new(FakeExec::with_cat_stdout(tail_lines, run_json));
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));

        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "deploy".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.status == rupu_orchestrator::RunStatus::Completed {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for pump; status={:?}", rec.status);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // The other artifacts still mirror.
        let events = std::fs::read_to_string(run_store.events_path(run_id)).unwrap_or_default();
        assert!(
            events.contains(event_json),
            "events must mirror: {events:?}"
        );

        // The tail command must include the transcript path (proves the pump
        // asks for it at all), with the run_id concatenated bare — the
        // [A-Za-z0-9_]-only invariant that keeps the raw command injection-safe.
        let commands = fake.commands.lock().unwrap().clone();
        let tail = commands
            .iter()
            .find(|c| c.starts_with("tail "))
            .expect("tail command recorded");
        assert!(
            tail.contains(&format!("$HOME/.rupu/transcripts/{run_id}.jsonl")),
            "tail must include the transcript path, got: {tail}"
        );

        // No transcript existed → nothing mirrored, nothing synthesized.
        let transcript_path = tmp
            .path()
            .join("transcripts")
            .join(format!("{run_id}.jsonl"));
        assert!(
            !transcript_path.exists(),
            "no transcript must be created when the remote never had one"
        );
        let steps = run_store.read_step_results(run_id).unwrap();
        assert!(
            steps.is_empty(),
            "no step result must be synthesized without a transcript: {steps:?}"
        );
    }

    /// Terminal catch-up: the pump's select loop can be torn down with
    /// transcript lines still buffered in the tail stream. On terminal
    /// detection, a one-shot `cat` of the remote transcript must REPLACE the
    /// (possibly partial) tailed copy with the complete content — never leave
    /// a truncated transcript that looks complete, and never duplicate the
    /// lines the tail already delivered.
    #[tokio::test]
    async fn tail_pump_terminal_cat_replaces_partial_transcript() {
        let run_id = "run_01TESTPUMP04";
        let l1 = r#"{"type":"run_start","agent":"reviewer"}"#;
        let l2 = r#"{"type":"assistant_message","content":"mid"}"#;
        let l3 = r#"{"type":"run_complete","status":"ok"}"#;
        // The tail only ever delivers l1 — l2/l3 are "still buffered".
        let tail_lines = vec![
            format!("==> /home/ci/.rupu/transcripts/{run_id}.jsonl <=="),
            l1.to_string(),
        ];
        let run_json = format!(r#"{{"run_id":"{run_id}","status":"completed"}}"#);
        let full_transcript = format!("{l1}\n{l2}\n{l3}\n");

        let mut fake = FakeExec::with_cat_stdout(tail_lines, run_json);
        fake.cat_transcript_stdout = Some(full_transcript.clone());
        let fake = std::sync::Arc::new(fake);
        let (conn, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));

        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "reviewer".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        conn.mirror
            .create_run(run_id, &conn.host_id, &spec)
            .unwrap();

        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.status == rupu_orchestrator::RunStatus::Completed {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for pump; status={:?}", rec.status);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let transcript_path = tmp
            .path()
            .join("transcripts")
            .join(format!("{run_id}.jsonl"));
        let transcript = std::fs::read_to_string(&transcript_path).unwrap_or_default();
        assert_eq!(
            transcript, full_transcript,
            "terminal cat must replace the partial tailed copy with the \
             complete remote content, without duplicating l1"
        );
    }

    /// Reproduces the live-host truncation: the dispatching process is the
    /// `rupu workflow run` CLI, which exits the moment its `get_run` poll
    /// sees terminal — and the tail pump is a `tokio::spawn`ed task in that
    /// process, killed mid-flight (mid-`cat`, on the real host). The earlier
    /// pump tests cannot see this because nothing there tears the runtime
    /// down.
    ///
    /// So this test owns its runtime and drops it. Inside `block_on` it runs
    /// exactly the dispatcher's sequence (`launch_agent` → poll `get_run`
    /// until terminal → `await_run_mirror`), then drops the runtime — the
    /// process exit — and asserts ON DISK, after teardown, that the mirrored
    /// transcript ends with the terminal event.
    ///
    /// Two details make the reproduction honest rather than lucky:
    ///  - the tail delivers NOTHING (`tail_lines` empty): every transcript
    ///    byte must come from the pump's terminal `cat`, which is exactly the
    ///    work that was being killed;
    ///  - that `cat` is slow (150 ms), so without the join the runtime drops
    ///    while the pump is inside it. On a current-thread runtime the pump
    ///    only progresses while the driving future is suspended — without
    ///    `await_run_mirror` yielding, it never even starts.
    ///
    /// Without the join this fails on the file being absent; with it, the
    /// join is what keeps the "process" alive until the copy is complete.
    #[test]
    fn dispatching_process_teardown_keeps_mirrored_transcript_complete() {
        let l1 = r#"{"type":"run_start","agent":"reviewer"}"#;
        let l2 = r#"{"type":"assistant_message","content":"done."}"#;
        let l3 = r#"{"type":"run_complete","status":"ok","total_tokens":5039}"#;
        let full_transcript = format!("{l1}\n{l2}\n{l3}\n");

        let mut fake =
            FakeExec::with_cat_stdout(vec![], r#"{"status":"completed","final_output":"done."}"#);
        fake.cat_transcript_stdout = Some(full_transcript.clone());
        fake.cat_transcript_delay = Some(std::time::Duration::from_millis(150));
        fake.show_stdout = Some(
            r#"{"item":{"run":{"status":"completed","final_output":"done."},"steps":[],"usage":{}}}"#
                .to_string(),
        );
        let fake = std::sync::Arc::new(fake);
        let (conn, _run_store, tmp) = make_conn(std::sync::Arc::clone(&fake));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // The dispatcher's sequence, verbatim in shape.
        let run_id = rt.block_on(async {
            let run_id = conn
                .launch_agent(crate::agent_launcher::AgentLaunchRequest {
                    agent: "reviewer".into(),
                    prompt: Some("go".into()),
                    mode: None,
                    target: None,
                    working_dir: None,
                    run_id: None,
                })
                .await
                .expect("launch_agent");
            loop {
                let rec = conn.get_run(&run_id).await.expect("get_run");
                if is_terminal_status(rec["run"]["status"].as_str().unwrap_or("")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            conn.await_run_mirror(&run_id).await;
            run_id
        });

        // The CLI process exits here: every spawned task dies with the runtime.
        drop(rt);

        let transcript_path = tmp
            .path()
            .join("transcripts")
            .join(format!("{run_id}.jsonl"));
        let transcript = std::fs::read_to_string(&transcript_path).unwrap_or_default();
        assert_eq!(
            transcript, full_transcript,
            "mirrored transcript must be complete on disk after the dispatching \
             process tears down; got {transcript:?}"
        );
        assert!(
            transcript.trim_end().ends_with(l3),
            "mirrored transcript must end with the terminal run_complete event"
        );
    }

    // ── Launch cwd + launch-log tests ────────────────────────────────────────
    //
    // Two defects in one family — work dispatched correctly, then the remote
    // process launched without the context it was prepared for:
    //  (1) `working_dir` was threaded through the launch request and never
    //      read, so the remote `rupu run` executed in the login `$HOME` and a
    //      `workspace: sync` step's delta came from a dir the agent never
    //      touched;
    //  (2) the detached command sent stderr to `/dev/null`, so a launch that
    //      died instantly left no trace anywhere.

    const STAGED_WD: &str = "/cache/workspace-sync/01J/work";

    #[test]
    fn is_safe_run_id_accepts_minted_ids_and_rejects_metacharacters() {
        assert!(is_safe_run_id(&format!("run_{}", Ulid::new())));
        assert!(is_safe_run_id("run_01HXYZ_ok"));
        for bad in [
            "",
            "run_01X;x",
            "run 01X",
            "run_01X/..",
            "run_$HOME",
            "run_01X\n",
            "run_`id`",
        ] {
            assert!(!is_safe_run_id(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn detach_launch_cds_into_working_dir_before_setsid() {
        let cmd = SshHostConnector::detach_launch("'rupu' 'run' 'a'", "run_01X", Some(STAGED_WD))
            .unwrap();
        let cd = cmd
            .find(&format!("cd '{STAGED_WD}' && "))
            .unwrap_or_else(|| panic!("launch must cd into the staged dir: {cmd}"));
        let setsid = cmd
            .find("setsid 'rupu' 'run' 'a'")
            .unwrap_or_else(|| panic!("launch must still be detached via setsid: {cmd}"));
        assert!(cd < setsid, "cd must precede the launch: {cmd}");
    }

    #[test]
    fn detach_launch_without_working_dir_emits_no_cd() {
        let cmd = SshHostConnector::detach_launch("'rupu' 'run' 'a'", "run_01X", None).unwrap();
        assert!(
            !cmd.contains("cd "),
            "self-contained launch must keep the login cwd: {cmd}"
        );
        assert!(
            cmd.contains("setsid 'rupu' 'run' 'a' </dev/null >/dev/null"),
            "detach shape must be unchanged: {cmd}"
        );
    }

    #[test]
    fn detach_launch_quotes_hostile_working_dir_inert() {
        // Every metacharacter class in one value: single quote + terminator,
        // comment, both substitution forms, double quotes, newline, a
        // redirection, a glob, spaces.
        let hostile = "/tmp/x'; rm -rf / #$(id)`id` \"q\"\nnext >out *";
        let cmd =
            SshHostConnector::detach_launch("'rupu' 'run' 'a'", "run_01X", Some(hostile)).unwrap();
        let quoted = shell_escape(hostile);
        assert!(
            cmd.contains(&format!("cd {quoted} && ")),
            "working_dir must appear as ONE shell-escaped literal: {cmd}"
        );
        // The segment between `cd ` and the launch is exactly that literal —
        // nothing of the value leaks past the closing quote.
        let after_cd = &cmd[cmd.find("cd ").unwrap() + 3..];
        let end = after_cd
            .find(" && (setsid")
            .unwrap_or_else(|| panic!("launch must follow the cd: {cmd}"));
        assert_eq!(&after_cd[..end], quoted);
        // Remove the one quoted literal; no hostile fragment may survive
        // outside it.
        let rest = cmd.replacen(&quoted, "<WD>", 1);
        for needle in ["rm -rf", "$(", "`", "\n", "\"", ">out", " *", "#"] {
            assert!(
                !rest.contains(needle),
                "{needle:?} escaped the quotes: {rest}"
            );
        }
    }

    #[test]
    fn detach_launch_refuses_unsafe_run_id() {
        for bad in [
            "",
            "run_01X; rm -rf /",
            "run_$(id)",
            "run 01X",
            "run_01X/../x",
        ] {
            let err = SshHostConnector::detach_launch("'rupu'", bad, None).unwrap_err();
            assert!(
                matches!(err, HostConnectorError::Invalid(_)),
                "{bad:?} → {err:?}"
            );
        }
        assert!(SshHostConnector::detach_launch("'rupu'", "run_01HXYZ_ok", None).is_ok());
    }

    #[test]
    fn detach_launch_captures_stderr_in_per_run_log_not_dev_null() {
        let cmd = SshHostConnector::detach_launch("'rupu' 'run' 'a'", "run_01X", None).unwrap();
        let log = "$HOME/.rupu/runs/run_01X/launch.log";
        assert!(
            cmd.contains(&format!("2>>{log} &)")),
            "stderr must append to the per-run log: {cmd}"
        );
        assert!(
            !cmd.contains("2>&1") && !cmd.contains("2>/dev/null"),
            "stderr must not be discarded: {cmd}"
        );
        assert!(
            cmd.starts_with("mkdir -p $HOME/.rupu/runs/run_01X && "),
            "run dir must exist before the redirect opens the log: {cmd}"
        );
        assert!(
            cmd.contains(&format!("(umask 077; : > {log}) && ")),
            "log must be created 0600 inside its own subshell: {cmd}"
        );
        // stdin/stdout exactly as before.
        assert!(cmd.contains("</dev/null >/dev/null"), "{cmd}");
        // Only the setsid is backgrounded: the `&` closes the subshell and the
        // mkdir / log-create run in the ssh session's foreground, so their
        // failure is a non-zero ssh exit rather than a silent "success".
        assert!(cmd.ends_with(" &)"), "{cmd}");
        let single_amps = cmd.matches('&').count() - 2 * cmd.matches("&&").count();
        assert_eq!(single_amps, 1, "exactly one backgrounding `&`: {cmd}");
    }

    #[tokio::test]
    async fn launch_agent_dispatches_in_staged_working_dir() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let run_id = conn
            .launch_agent(crate::agent_launcher::AgentLaunchRequest {
                agent: "reviewer".into(),
                prompt: Some("go".into()),
                mode: None,
                target: None,
                working_dir: Some(STAGED_WD.into()),
                run_id: None,
            })
            .await
            .unwrap();

        let cmds = fake.commands.lock().unwrap();
        let launch = cmds
            .iter()
            .find(|c| c.contains("setsid"))
            .unwrap_or_else(|| panic!("no detached launch in {cmds:?}"));
        assert!(
            launch.contains(&format!(
                "cd '{STAGED_WD}' && (setsid 'rupu' 'run' 'reviewer'"
            )),
            "agent launch must run in the staged dir: {launch}"
        );
        assert!(
            launch.contains(&format!("2>>$HOME/.rupu/runs/{run_id}/launch.log")),
            "agent launch must keep its stderr: {launch}"
        );
    }

    #[tokio::test]
    async fn launch_agent_honours_a_coordinator_minted_run_id() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let id = conn
            .launch_agent(crate::agent_launcher::AgentLaunchRequest {
                agent: "reviewer".into(),
                prompt: Some("go".into()),
                mode: None,
                target: None,
                working_dir: None,
                run_id: Some("run_01MINTEDBYCOORD".into()),
            })
            .await
            .unwrap();
        assert_eq!(id, "run_01MINTEDBYCOORD");
        assert_eq!(
            run_store.load(&id).unwrap().worker_id.as_deref(),
            Some("host_abc")
        );
        let cmds = fake.commands.lock().unwrap();
        assert!(
            cmds.iter()
                .any(|c| c.contains("'--run-id' 'run_01MINTEDBYCOORD'")),
            "{cmds:?}"
        );
    }

    #[tokio::test]
    async fn launch_agent_rejects_a_malformed_supplied_run_id_without_dispatching() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let err = conn
            .launch_agent(crate::agent_launcher::AgentLaunchRequest {
                agent: "reviewer".into(),
                prompt: None,
                mode: None,
                target: None,
                working_dir: None,
                run_id: Some("../evil".into()),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)), "{err}");
        assert!(fake.commands.lock().unwrap().is_empty(), "nothing shelled");
    }

    #[tokio::test]
    async fn launch_run_dispatches_in_working_dir() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        conn.launch_run(crate::launcher::LaunchRequest {
            workflow: "deploy".into(),
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: Some(STAGED_WD.into()),
        })
        .await
        .unwrap();

        let cmds = fake.commands.lock().unwrap();
        let launch = cmds
            .iter()
            .find(|c| c.contains("setsid"))
            .unwrap_or_else(|| panic!("no detached launch in {cmds:?}"));
        assert!(
            launch.contains(&format!(
                "cd '{STAGED_WD}' && (setsid 'rupu' 'workflow' 'run' 'deploy'"
            )),
            "workflow launch must run in the working dir: {launch}"
        );
    }

    #[tokio::test]
    async fn launch_diagnostics_returns_remote_launch_log_tail() {
        let mut fake = FakeExec::ok(vec![]);
        fake.launch_log_stdout =
            Some("warning: something benign\nError: agent `reviewer` not found\n".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let diag = conn
            .launch_diagnostics("run_01X")
            .await
            .expect("captured stderr must surface");
        assert!(diag.contains("Error: agent `reviewer` not found"), "{diag}");
        let cmds = fake.commands.lock().unwrap();
        assert_eq!(
            cmds.as_slice(),
            ["cat $HOME/.rupu/runs/run_01X/launch.log"],
            "must read exactly the per-run log detach_launch writes"
        );
    }

    #[tokio::test]
    async fn launch_diagnostics_reads_the_log_and_is_none_when_empty() {
        // `launch_log_stdout: None` → the fake answers an empty stdout: a
        // healthy launch that wrote nothing to stderr.
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        assert_eq!(conn.launch_diagnostics("run_01X").await, None);
        // Silence must mean "looked and found nothing", not "didn't look".
        assert!(
            fake.commands
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "cat $HOME/.rupu/runs/run_01X/launch.log"),
            "the log must actually be read"
        );
    }

    #[tokio::test]
    async fn launch_diagnostics_refuses_unsafe_run_id_without_shelling_out() {
        let mut fake = FakeExec::ok(vec![]);
        fake.launch_log_stdout = Some("would leak".into());
        let fake = std::sync::Arc::new(fake);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        for bad in ["run_01X; rm -rf /", "run_$(id)", "../etc/passwd", ""] {
            assert_eq!(conn.launch_diagnostics(bad).await, None, "{bad:?}");
        }
        assert!(
            fake.commands.lock().unwrap().is_empty(),
            "an id outside [A-Za-z0-9_] must never reach the host: {:?}",
            fake.commands.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn launch_diagnostics_bounds_excerpt_to_the_tail() {
        let mut fake = FakeExec::ok(vec![]);
        fake.launch_log_stdout = Some(format!(
            "{}\nError: the actual reason",
            "x".repeat(LAUNCH_DIAGNOSTICS_MAX_CHARS * 2)
        ));
        let fake = std::sync::Arc::new(fake);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let diag = conn.launch_diagnostics("run_01X").await.unwrap();
        assert!(
            diag.chars().count() <= LAUNCH_DIAGNOSTICS_MAX_CHARS + 1,
            "excerpt must be bounded (got {} chars)",
            diag.chars().count()
        );
        assert!(
            diag.starts_with('…'),
            "truncation must be marked: {diag:.20}"
        );
        assert!(
            diag.ends_with("Error: the actual reason"),
            "the TAIL carries the diagnosis and must survive"
        );
    }

    #[test]
    fn tail_chars_keeps_the_tail_on_char_boundaries() {
        assert_eq!(tail_chars("abc", 5), "abc");
        assert_eq!(tail_chars("abc", 3), "abc");
        assert_eq!(tail_chars("abcdef", 3), "…def");
        assert_eq!(tail_chars("ééé", 2), "…éé");
    }

    // ── Workspace sync (stage/collect/discard) tests ─────────────────────────

    #[tokio::test]
    async fn ssh_stage_rejects_multiline_working_dir() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(
            b"/cache/workspace-sync/x/work\nrm -rf /\n".to_vec(),
        ));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn.stage_workspace(b"PAYLOAD".to_vec()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn ssh_stage_rejects_relative_working_dir() {
        let fake =
            std::sync::Arc::new(FakeExec::with_bytes_ok(b"workspace-sync/x/work\n".to_vec()));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn.stage_workspace(b"PAYLOAD".to_vec()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn ssh_stage_returns_working_dir_line() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(
            b"/cache/workspace-sync/x/work\n".to_vec(),
        ));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let dir = conn.stage_workspace(b"PAYLOAD".to_vec()).await.unwrap();
        assert_eq!(dir, "/cache/workspace-sync/x/work");

        let (cmd, stdin) = fake.last_bytes_call.lock().unwrap().clone().unwrap();
        assert!(cmd.contains("__workspace") && cmd.contains("stage"));
        assert_eq!(stdin.as_deref(), Some(&b"PAYLOAD"[..]));
    }

    #[tokio::test]
    async fn ssh_stage_nonzero_maps_to_remote_error() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_err(RemoteExecError::NonZero {
            code: Some(1),
            stderr: "helper failed".into(),
        }));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn.stage_workspace(b"x".to_vec()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Remote(1, _)), "{err:?}");
    }

    #[tokio::test]
    async fn ssh_stage_spawn_failure_maps_to_unreachable() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_err(RemoteExecError::Spawn(
            "no route".into(),
        )));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn.stage_workspace(b"x".to_vec()).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Unreachable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn ssh_stage_oversize_payload_rejected() {
        // No run_bytes call is expected to reach the exec — the size guard
        // must reject before spawning ssh — but script an Ok anyway so a
        // regression that skips the guard fails loudly on the assertion
        // below rather than panicking on an un-scripted FakeExec.
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(Vec::new()));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let huge = vec![0u8; MAX_WORKSPACE_BYTES + 1];
        let err = conn.stage_workspace(huge).await.unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)), "{err:?}");
        assert!(
            fake.last_bytes_call.lock().unwrap().is_none(),
            "oversize payload must be rejected before touching run_bytes"
        );
    }

    #[tokio::test]
    async fn ssh_collect_returns_delta_bytes() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(b"DELTA-BYTES".to_vec()));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let bytes = conn
            .collect_workspace_delta("/cache/workspace-sync/x/work")
            .await
            .unwrap();
        assert_eq!(bytes, b"DELTA-BYTES");

        let (cmd, stdin) = fake.last_bytes_call.lock().unwrap().clone().unwrap();
        assert!(cmd.contains("__workspace") && cmd.contains("collect"));
        assert!(stdin.is_none());
    }

    #[tokio::test]
    async fn ssh_collect_oversize_rejected() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(vec![0u8; MAX_WORKSPACE_BYTES + 1]));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        let err = conn
            .collect_workspace_delta("/cache/workspace-sync/x/work")
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Invalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn ssh_discard_issues_remote_discard_command() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_ok(Vec::new()));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        conn.discard_workspace("/cache/workspace-sync/x/work")
            .await
            .unwrap();

        let (cmd, stdin) = fake.last_bytes_call.lock().unwrap().clone().unwrap();
        assert!(cmd.contains("__workspace") && cmd.contains("discard"));
        assert!(stdin.is_none());
    }

    #[tokio::test]
    async fn ssh_discard_maps_remote_failure() {
        let fake = std::sync::Arc::new(FakeExec::with_bytes_err(RemoteExecError::NonZero {
            code: Some(3),
            stderr: "already gone".into(),
        }));
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

        // discard_workspace itself still surfaces a mapped error — the
        // dispatcher (not this connector) is what treats it as best-effort by
        // ignoring the `Result`.
        let err = conn
            .discard_workspace("/cache/workspace-sync/x/work")
            .await
            .unwrap_err();
        assert!(matches!(err, HostConnectorError::Remote(3, _)), "{err:?}");
    }

    // ── Task 6: run.json-while-alive, live active step, terminal batch pull ──

    fn agent_spec() -> crate::node::protocol::RunSpec {
        crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "reviewer".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        }
    }

    fn workflow_spec() -> crate::node::protocol::RunSpec {
        crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "wf".into(),
            inputs: std::collections::BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        }
    }

    #[test]
    fn note_transcript_started_sets_the_live_active_step_and_finish_clears_it() {
        let fake = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        conn.mirror
            .create_run("run_01LIVE", "host_abc", &agent_spec())
            .unwrap();

        conn.mirror
            .note_transcript_started("run_01LIVE", "host_abc")
            .unwrap();
        let rec = run_store.load("run_01LIVE").unwrap();
        assert_eq!(rec.active_step_id.as_deref(), Some("agent"));
        assert_eq!(
            rec.active_step_transcript_path,
            Some(conn.mirror.transcript_mirror_path("run_01LIVE"))
        );
        assert!(matches!(
            conn.mirror
                .note_transcript_started("run_01LIVE", "host_other"),
            Err(crate::node::MirrorError::WrongNode(_))
        ));

        conn.mirror
            .finish("run_01LIVE", "host_abc", "completed")
            .unwrap();
        let rec = run_store.load("run_01LIVE").unwrap();
        assert_eq!(rec.active_step_id, None);
        assert_eq!(rec.active_step_transcript_path, None);
    }

    #[test]
    fn batch_cat_command_quotes_each_path_and_brackets_each_file() {
        let cmd = batch_cat_command(&["/a/run_01A.jsonl".into(), "/b/it's.jsonl".into()]);
        assert_eq!(
            cmd,
            "for p in '/a/run_01A.jsonl' '/b/it'\\''s.jsonl'; do printf '==> %s <==\\n' \"$p\"; cat \"$p\" 2>/dev/null; printf '\\n==> end <==\\n'; done"
        );
    }

    #[test]
    fn split_batched_cat_separates_files_and_drops_the_synthetic_trailing_newline() {
        let stdout = "==> /a/one.jsonl <==\n{\"a\":1}\n{\"a\":2}\n\n==> end <==\n\
                      ==> /a/absent.jsonl <==\n\n==> end <==\n\
                      ==> /a/torn.jsonl <==\n{\"t\":1}\n{\"t\":\n==> end <==\n";
        let files = split_batched_cat(stdout);
        assert_eq!(files["/a/one.jsonl"], "{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(files["/a/absent.jsonl"], "");
        assert_eq!(
            files["/a/torn.jsonl"], "{\"t\":1}\n{\"t\":\n",
            "a torn last line is kept"
        );
        assert_eq!(files.len(), 3);
        // A stream cut mid-file never yields that file at all.
        let cut = "==> /a/one.jsonl <==\n{\"a\":1}\n";
        assert!(split_batched_cat(cut).is_empty());
    }

    /// Build the remote's `run.json` body from a REAL `RunRecord` so the
    /// mirror's `RunJson` parse cannot silently fail on a hand-written shape.
    fn remote_run_json(
        run_store: &rupu_orchestrator::RunStore,
        run_id: &str,
        status: rupu_orchestrator::RunStatus,
        active: Option<(&str, &str)>,
    ) -> String {
        let mut rec = run_store.load(run_id).unwrap();
        rec.status = status;
        rec.worker_id = None; // the remote does not know it is a mirror
        rec.active_step_id = active.map(|(id, _)| id.to_string());
        rec.active_step_transcript_path = active.map(|(_, p)| std::path::PathBuf::from(p));
        serde_json::to_string(&rec).unwrap()
    }

    #[tokio::test]
    async fn tail_pump_mirrors_run_json_while_the_run_is_still_alive() {
        let run_id = "run_01ALIVE";
        // Two-phase construction: the mirror record must exist before the
        // remote run.json body can be derived from it.
        let fake_probe = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn0, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake_probe));
        conn0
            .mirror
            .create_run(run_id, &conn0.host_id, &workflow_spec())
            .unwrap();
        let alive_json = remote_run_json(
            &run_store,
            run_id,
            rupu_orchestrator::RunStatus::Running,
            Some(("build", "/remote/proj/.rupu/transcripts/run_01BUILD.jsonl")),
        );
        let fake = std::sync::Arc::new(FakeExec::with_cat_stdout(vec![], alive_json));
        let mirror = std::sync::Arc::clone(&conn0.mirror);
        let exec: std::sync::Arc<dyn RemoteExec> = fake;
        let conn =
            SshHostConnector::new("host_abc", exec, mirror, std::sync::Arc::clone(&run_store));
        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.active_step_id.as_deref() == Some("build") {
                assert_eq!(
                    rec.active_step_transcript_path,
                    Some(std::path::PathBuf::from(
                        "/remote/proj/.rupu/transcripts/run_01BUILD.jsonl"
                    ))
                );
                assert_eq!(rec.status, rupu_orchestrator::RunStatus::Running);
                assert_eq!(
                    rec.worker_id.as_deref(),
                    Some("host_abc"),
                    "identity re-pinned"
                );
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("non-terminal run.json was never mirrored");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn tail_pump_first_transcript_header_marks_the_agent_step_live() {
        let run_id = "run_01LIVEHDR";
        let tail_lines = vec![
            format!("==> /home/ci/.rupu/transcripts/{run_id}.jsonl <=="),
            r#"{"type":"run_start","agent":"reviewer"}"#.to_string(),
        ];
        // No run.json yet (an agent run writes it only at the end) → the
        // interval probe answers Absent and the pump stays open.
        let fake = std::sync::Arc::new(FakeExec::ok(tail_lines));
        let (conn, run_store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        conn.mirror
            .create_run(run_id, &conn.host_id, &agent_spec())
            .unwrap();
        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let rec = run_store.load(run_id).unwrap();
            if rec.active_step_id.as_deref() == Some("agent") {
                assert_eq!(
                    rec.active_step_transcript_path,
                    Some(conn.mirror.transcript_mirror_path(run_id))
                );
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("active step never set from the transcript header");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn tail_pump_terminal_pulls_every_recorded_step_transcript_into_the_cache() {
        let run_id = "run_01PULLALL";
        let step_a = "/home/ci/proj/.rupu/transcripts/run_01STEPA.jsonl";
        let step_b = "/home/ci/proj/.rupu/transcripts/run_01STEPB.jsonl";
        // The pump's `select!` is unbiased, so the terminal probe can win
        // before any tailed line is consumed. Pre-seed the LOCAL mirror with
        // the artifacts the tail would have delivered (that is what
        // `recorded_transcript_paths` reads), and feed no tail lines.
        let fake_probe = std::sync::Arc::new(FakeExec::ok(vec![]));
        let (conn0, run_store, tmp) = make_conn(std::sync::Arc::clone(&fake_probe));
        conn0
            .mirror
            .create_run(run_id, &conn0.host_id, &workflow_spec())
            .unwrap();
        conn0
            .mirror
            .append(
                run_id,
                "host_abc",
                ArtifactFile::StepResults,
                &format!(r#"{{"step_id":"a","run_id":"run_01STEPA","transcript_path":"{step_a}","output":"","success":true,"skipped":false,"rendered_prompt":"","finished_at":"2026-09-04T00:00:00Z"}}"#),
            )
            .unwrap();
        conn0
            .mirror
            .append(
                run_id,
                "host_abc",
                ArtifactFile::Events,
                &format!(r#"{{"type":"step_working","run_id":"{run_id}","step_id":"b","transcript_path":"{step_b}"}}"#),
            )
            .unwrap();
        let run_json = remote_run_json(
            &run_store,
            run_id,
            rupu_orchestrator::RunStatus::Completed,
            None,
        );
        let mut fake = FakeExec::with_cat_stdout(vec![], run_json);
        fake.batch_cat_stdout = Some(format!(
            "==> {step_a} <==\n{{\"type\":\"run_start\"}}\n\n==> end <==\n==> {step_b} <==\n\n==> end <==\n"
        ));
        let fake = std::sync::Arc::new(fake);
        let mirror = std::sync::Arc::clone(&conn0.mirror);
        let exec: std::sync::Arc<dyn RemoteExec> = std::sync::Arc::clone(&fake) as _;
        let conn =
            SshHostConnector::new("host_abc", exec, mirror, std::sync::Arc::clone(&run_store));
        conn.spawn_tail_pump(run_id.to_string());

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if run_store.load(run_id).unwrap().status != rupu_orchestrator::RunStatus::Running {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("pump never finalized");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Join the pump's terminal work the way the dispatcher does.
        conn.await_run_mirror(run_id).await;

        let cache_a = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01STEPA.jsonl");
        let cache_b = tmp
            .path()
            .join("mirror/host_abc/transcripts/run_01STEPB.jsonl");
        assert_eq!(
            std::fs::read_to_string(&cache_a).unwrap(),
            "{\"type\":\"run_start\"}\n"
        );
        assert!(crate::host::transcript_paths::is_complete(&cache_a));
        assert_eq!(
            std::fs::read_to_string(&cache_b).unwrap(),
            "",
            "absent remote file → empty complete answer"
        );
        assert!(crate::host::transcript_paths::is_complete(&cache_b));

        let cmds = fake.commands.lock().unwrap();
        let batch: Vec<_> = cmds.iter().filter(|c| c.starts_with("for p in ")).collect();
        assert_eq!(
            batch.len(),
            1,
            "exactly one ssh invocation for all files: {cmds:?}"
        );
        assert!(
            batch[0].contains(&format!("'{step_a}'")) && batch[0].contains(&format!("'{step_b}'"))
        );
    }

    // ── End-to-end SSH workspace-sync parity (ssh-ws T5) ─────────────────────
    //
    // `RemoteExec` and `SshHostConnector` are `pub(crate)`, so this e2e cannot
    // live in `crates/rupu-cp/tests/` as an integration test — it is a unit
    // test here instead, mirroring where the existing `FakeExec` SSH tests
    // live. `HelperExec` is a `RemoteExec` double that, unlike `FakeExec`
    // above (which returns scripted bytes), actually calls the *real* shared
    // staging core (`workspace_stage::stage_to_dir` / `collect_from_dir`)
    // against a tempdir standing in for the remote cache root. This proves
    // the SSH connector's command-building + byte-piping wiring end-to-end
    // against the same core the Local/HttpCp transports use (3c's
    // `workspace_sync_e2e.rs`), for both a git and a non-git (tar) workspace.
    mod e2e_workspace_sync {
        use super::*;
        use crate::host::workspace_stage::{collect_from_dir, stage_to_dir};

        /// A `RemoteExec` double that dispatches on the remote command string
        /// and runs the *real* shared staging core against `self.cache`
        /// (standing in for the remote host's cache root). Stateful — unlike
        /// `FakeExec`'s single-shot scripted `run_bytes`, this must serve both
        /// a stage call and a collect call for the same test.
        struct HelperExec {
            cache: tempfile::TempDir,
        }

        impl HelperExec {
            fn new() -> Self {
                Self {
                    cache: tempfile::tempdir().unwrap(),
                }
            }
        }

        /// Extract the last single-quoted token from a `build_remote_command`
        /// output, e.g. `'rupu' '__workspace' 'collect' '/cache/.../work'` ->
        /// `/cache/.../work`. Good enough for test-generated paths, which
        /// never contain an embedded `'`.
        fn last_quoted_arg(cmd: &str) -> String {
            let trimmed = cmd.trim_end();
            let body = trimmed
                .strip_suffix('\'')
                .expect("remote command must end with a quoted arg");
            let start = body.rfind('\'').expect("expected an opening quote") + 1;
            body[start..].to_string()
        }

        #[async_trait::async_trait]
        impl RemoteExec for HelperExec {
            async fn run(&self, _remote_command: &str) -> Result<RemoteOutput, RemoteExecError> {
                unimplemented!("HelperExec only exercises run_bytes for workspace sync")
            }

            fn spawn_lines(&self, _remote_command: &str) -> Result<LineStream, RemoteExecError> {
                unimplemented!("HelperExec only exercises run_bytes for workspace sync")
            }

            async fn run_bytes(
                &self,
                remote_command: &str,
                stdin: Option<Vec<u8>>,
            ) -> Result<Vec<u8>, RemoteExecError> {
                if remote_command.contains("__workspace") && remote_command.contains("stage") {
                    let payload = stdin.unwrap_or_default();
                    let dir = stage_to_dir(&payload, self.cache.path()).map_err(|e| {
                        RemoteExecError::NonZero {
                            code: Some(1),
                            stderr: e.to_string(),
                        }
                    })?;
                    let mut out = dir.into_bytes();
                    out.push(b'\n');
                    Ok(out)
                } else if remote_command.contains("__workspace")
                    && remote_command.contains("collect")
                {
                    let dir = last_quoted_arg(remote_command);
                    collect_from_dir(&dir, self.cache.path()).map_err(|e| {
                        RemoteExecError::NonZero {
                            code: Some(1),
                            stderr: e.to_string(),
                        }
                    })
                } else {
                    panic!("HelperExec: unexpected remote command: {remote_command}");
                }
            }
        }

        /// Build a coordinator workspace: a plain non-git dir when `use_git`
        /// is `false`, or a minimal git repo with one committed file when
        /// `true` — mirrors `workspace_stage::tests::git_init`.
        fn build_workspace(dir: &std::path::Path, use_git: bool) {
            std::fs::write(dir.join("a.txt"), "orig").unwrap();
            if use_git {
                let repo = git2::Repository::init(dir).unwrap();
                let mut cfg = repo.config().unwrap();
                cfg.set_str("user.name", "t").unwrap();
                cfg.set_str("user.email", "t@e").unwrap();
                let mut idx = repo.index().unwrap();
                idx.add_path(std::path::Path::new("a.txt")).unwrap();
                idx.write().unwrap();
                let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
                let sig = repo.signature().unwrap();
                repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                    .unwrap();
            }
        }

        #[tokio::test]
        async fn ssh_workspace_sync_round_trips_git_and_tar() {
            for use_git in [true, false] {
                // 1. Build the coordinator workspace and pack it.
                let coordinator = tempfile::tempdir().unwrap();
                build_workspace(coordinator.path(), use_git);
                let payload = rupu_workspace::pack(coordinator.path()).unwrap();
                assert_eq!(
                    payload.mode,
                    if use_git {
                        rupu_workspace::SyncMode::Git
                    } else {
                        rupu_workspace::SyncMode::Tar
                    },
                    "use_git={use_git}"
                );
                let encoded = crate::host::connector::encode_payload(&payload);

                // 2. SshHostConnector wired to a HelperExec backed by a fresh
                // tempdir cache root standing in for the remote host.
                let fake = std::sync::Arc::new(HelperExec::new());
                let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));

                // 3. Stage: connector pipes the encoded payload over
                // `run_bytes`, HelperExec runs the real `stage_to_dir`.
                let dir = conn.stage_workspace(encoded).await.unwrap_or_else(|e| {
                    panic!("stage_workspace failed (use_git={use_git}): {e:?}")
                });

                // 4. Simulate the remote agent editing a file under `dir`.
                std::fs::write(std::path::Path::new(&dir).join("a.txt"), "EDITED").unwrap();

                // 5. Collect: connector issues the collect command, HelperExec
                // runs the real `collect_from_dir`, returns the encoded delta.
                let delta_bytes = conn
                    .collect_workspace_delta(&dir)
                    .await
                    .unwrap_or_else(|e| {
                        panic!("collect_workspace_delta failed (use_git={use_git}): {e:?}")
                    });
                let delta = crate::host::connector::decode_delta(&delta_bytes).unwrap();
                assert!(
                    delta.changed.iter().any(|p| p == "a.txt"),
                    "use_git={use_git}: expected a.txt in changed set, got {:?}",
                    delta.changed
                );

                // 6. Apply the delta to a FRESH copy of the coordinator
                // workspace (not the one that was packed) and assert the
                // edit landed — proving parity with the Local/HttpCp path
                // over the SSH command/pipe wiring.
                let fresh = tempfile::tempdir().unwrap();
                build_workspace(fresh.path(), use_git);
                rupu_workspace::apply_deltas(fresh.path(), std::slice::from_ref(&delta)).unwrap();
                let applied = std::fs::read_to_string(fresh.path().join("a.txt")).unwrap();
                assert_eq!(
                    applied, "EDITED",
                    "use_git={use_git}: edit must land on the fresh coordinator copy"
                );

                // Scratch dir is cleaned up by collect_from_dir.
                assert!(!std::path::Path::new(&dir).exists());
            }
        }
    }
}
