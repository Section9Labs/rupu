//! SSH transport: dispatch/observe/control runs on a host reachable over `ssh`.
//!
//! Auth is delegated entirely to the system `ssh` (ssh-agent / `~/.ssh/config`
//! / default keys); rupu stores no key material. Every remote argument is
//! shell-escaped before being joined into the remote command, because `ssh`
//! re-parses remote args through the remote login shell.

use std::path::Path;
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
        mirror_stream_run_events, read_transcript_file, EventByteStream, HostCapabilities,
        HostConnector, HostConnectorError, HostInfo, RunKind, RunListQuery, MAX_WORKSPACE_BYTES,
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

/// Build the args (after the `ssh` program) to run `remote_command` on `host`.
///
/// Flags emitted:
/// - `-o BatchMode=yes`  — fail fast on missing key rather than prompting
/// - `-o ConnectTimeout=10` — don't hang indefinitely on unreachable hosts
/// - `-i <identity_file>` — if provided
/// - `-p <port>` — if provided
/// - `<host>` — always present
/// - `<remote_command>` — always last
pub(crate) fn ssh_argv(
    host: &str,
    port: Option<u16>,
    identity_file: Option<&Path>,
    remote_command: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
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
    // Preserve first-seen order (rows arrive newest-first from the CLI).
    let mut order: Vec<String> = Vec::new();
    let mut by_cycle: BTreeMap<
        String,
        (
            String,
            Option<String>,
            String,
            String,
            Vec<String>,
            Vec<String>,
        ),
    > = BTreeMap::new();
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

/// Returns `true` when `status` is a terminal [`rupu_orchestrator::RunStatus`]
/// serialized value.  Mirrors [`RunStatus::is_terminal`] using the
/// `#[serde(rename_all = "snake_case")]` wire form.
fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "rejected" | "cancelled")
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

// ── SshHostConnector ──────────────────────────────────────────────────────────

/// [`HostConnector`] backed by SSH transport.
///
/// Dispatches workflow/agent runs as detached remote processes via
/// `setsid … </dev/null >/dev/null 2>&1 &`, mirrors their artifact files
/// via an `ssh tail -f` pump that routes `==>` file headers to the right
/// [`ArtifactFile`] variant, and issues control operations as one-shot
/// remote `rupu workflow` commands.  Auth is entirely delegated to the
/// system `ssh`; rupu stores no key material.
pub(crate) struct SshHostConnector {
    pub host_id: String,
    pub exec: Arc<dyn RemoteExec>,
    pub mirror: Arc<NodeMirror>,
    pub run_store: Arc<RunStore>,
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
        Self {
            host_id: host_id.into(),
            exec,
            mirror,
            run_store,
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

    /// Wrap a shell-escaped remote command so the run is detached and
    /// survives the SSH session closing.
    fn detach(remote_cmd: &str) -> String {
        format!("setsid {remote_cmd} </dev/null >/dev/null 2>&1 &")
    }

    /// Spawn a background tokio task that tails the JSONL artifact files
    /// for `run_id` on the remote host and feeds each line to
    /// [`NodeMirror::append`].
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
        let host_id = self.host_id.clone();

        // $HOME must expand on the remote shell — build as raw command strings
        // rather than through build_remote_command / shell_escape. Single-quoting
        // every token (as build_remote_command does) would prevent $HOME from
        // expanding, producing a literal path that never exists on the remote.
        // run_id contains only [A-Za-z0-9_] (ULID prefix), so unquoted
        // concatenation is safe.
        let tail_cmd = format!(
            "tail -n +1 -F \
             $HOME/.rupu/runs/{run_id}/events.jsonl \
             $HOME/.rupu/runs/{run_id}/step_results.jsonl \
             $HOME/.rupu/runs/{run_id}/unit_checkpoints.jsonl"
        );
        let cat_cmd = format!("cat $HOME/.rupu/runs/{run_id}/run.json");

        tokio::spawn(async move {
            let mut current: Option<ArtifactFile> = None;
            // Set to true when the interval-poll arm observes a terminal status
            // and calls mirror.finish.  Used below to skip the fallback cat.
            let mut terminal_seen = false;

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
                                        // Route subsequent lines based on filename
                                        // suffix — the expanded absolute path from
                                        // `tail` still ends with the same basename.
                                        current = if path.ends_with("events.jsonl") {
                                            Some(ArtifactFile::Events)
                                        } else if path.ends_with("step_results.jsonl") {
                                            Some(ArtifactFile::StepResults)
                                        } else if path.ends_with("unit_checkpoints.jsonl") {
                                            Some(ArtifactFile::UnitCheckpoints)
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
                            if let Ok(out) = exec.run(&cat_cmd).await {
                                if out.success && !out.stdout.trim().is_empty() {
                                    let trimmed = out.stdout.trim().to_string();
                                    if let Ok(rec) =
                                        serde_json::from_str::<serde_json::Value>(&trimmed)
                                    {
                                        if let Some(status) =
                                            rec.get("status").and_then(|v| v.as_str())
                                        {
                                            if is_terminal_status(status) {
                                                let status = status.to_string();
                                                let _ = mirror.append(
                                                    &run_id,
                                                    &host_id,
                                                    ArtifactFile::RunJson,
                                                    &trimmed,
                                                );
                                                let _ = mirror.finish(
                                                    &run_id, &host_id, &status,
                                                );
                                                terminal_seen = true;
                                                break;
                                                // Dropping `stream` here kills the
                                                // ssh child via kill_on_drop.
                                            }
                                        }
                                    }
                                }
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
                if let Ok(out) = exec.run(&cat_cmd).await {
                    if out.success && !out.stdout.trim().is_empty() {
                        let trimmed = out.stdout.trim().to_string();
                        let _ = mirror.append(&run_id, &host_id, ArtifactFile::RunJson, &trimmed);
                        // Use the observed status only if it is terminal; a
                        // non-terminal status (e.g. "running") would be wrong to
                        // persist as final since the executor may still be alive.
                        // Finish as "failed" in that case — it is better to surface
                        // a definite failure than to leave the run in Running forever.
                        let finish_status =
                            if let Ok(rec) = serde_json::from_str::<serde_json::Value>(&trimmed) {
                                if let Some(s) = rec.get("status").and_then(|v| v.as_str()) {
                                    if is_terminal_status(s) {
                                        s.to_string()
                                    } else {
                                        "failed".to_string()
                                    }
                                } else {
                                    "failed".to_string()
                                }
                            } else {
                                "failed".to_string()
                            };
                        let _ = mirror.finish(&run_id, &host_id, &finish_status);
                        return;
                    }
                }
                // cat failed entirely — mark failed so the run is not stuck.
                let _ = mirror.finish(&run_id, &host_id, "failed");
            }
        });
    }

    /// Issue a one-shot `rupu workflow <tail...>` command on the remote host.
    ///
    /// Used by [`cancel_run`], [`approve_run`], and [`reject_run`].
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
            return Err(HostConnectorError::Unreachable(out.stderr));
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

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let argv = Self::workflow_argv(&req, &run_id);
        let remote_cmd = build_remote_command(&argv);
        let detached = Self::detach(&remote_cmd);

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
        let run_id = format!("run_{}", Ulid::new());

        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: req.agent.clone(),
            inputs: std::collections::BTreeMap::new(),
            prompt: req.prompt.clone(),
            mode: req.mode.clone(),
            target: req.target.clone(),
        };

        self.mirror
            .create_run(&run_id, &self.host_id, &spec)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;

        let argv = Self::agent_argv(&req, &run_id);
        let remote_cmd = build_remote_command(&argv);
        let detached = Self::detach(&remote_cmd);

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

    async fn stream_run_events(&self, run_id: &str) -> Result<EventByteStream, HostConnectorError> {
        mirror_stream_run_events(&self.run_store, &self.host_id, run_id).await
    }

    async fn get_transcript(&self, path: &str) -> Result<serde_json::Value, HostConnectorError> {
        read_transcript_file(path)
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
        match scope {
            // The CLI lists active sessions by default; `--archived` restricts
            // to the archived scope. "active"/None → default (no flag).
            Some("archived") => argv.push("--archived".into()),
            _ => {}
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
        let cycle_rows = self.list_autoflow_runs().await.unwrap_or_default();
        // NOTE: `run_rows` are `RunListRow`-shaped (id / workflow_name / status /
        // started_at / finished_at / trigger / usage / turns / duration_ms).
        // `rupu run list` emits that type verbatim so remote == local by
        // construction; there is deliberately NO mapper. The id field is `id`.

        let now = chrono::Utc::now();
        let since = range.since(now);
        let in_range = |t: chrono::DateTime<chrono::Utc>| since.map(|s| t >= s).unwrap_or(true);

        let cycles: Vec<CycleRollup> = cycle_rows
            .iter()
            .filter_map(|c| {
                Some(CycleRollup {
                    cycle_id: c.get("cycle_id")?.as_str()?.to_string(),
                    worker_name: c
                        .get("worker_name")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    started_at: c
                        .get("started_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&chrono::Utc))?,
                    finished_at: c
                        .get("finished_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&chrono::Utc)),
                    ran: c.get("ran_cycles").and_then(|v| v.as_u64()).unwrap_or(0),
                    skipped: c
                        .get("skipped_cycles")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    failed: c.get("failed_cycles").and_then(|v| v.as_u64()).unwrap_or(0),
                    // Status is filled in below, once the run rows are indexed.
                    runs: c
                        .get("run_ids")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .map(|id| CycleRun {
                                    run_id: id.to_string(),
                                    status: "unknown".to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .filter(|c| in_range(c.started_at))
            .collect();

        // Index the CLI's run rows so each cycle's runs can carry a status —
        // the `+N clean` pill needs it, and this costs no extra round-trip.
        let status_of: std::collections::HashMap<&str, &str> = run_rows
            .iter()
            .filter_map(|r| {
                Some((
                    // `id`, NOT `run_id`: `rupu run list` emits `RunListRow`
                    // verbatim and its field is `id`. Reading `run_id` here
                    // yields an empty map and silently loses every status.
                    r.get("id")?.as_str()?,
                    r.get("status")?.as_str()?,
                ))
            })
            .collect();
        let mut cycles = cycles;
        for c in cycles.iter_mut() {
            for run in c.runs.iter_mut() {
                if let Some(st) = status_of.get(run.run_id.as_str()) {
                    run.status = st.to_string();
                }
            }
        }

        let cycle_of: std::collections::HashMap<String, String> = cycles
            .iter()
            .flat_map(|c| {
                c.runs
                    .iter()
                    .map(|r| (r.run_id.clone(), c.cycle_id.clone()))
            })
            .collect();

        let mut active = ActiveCounts::default();
        let mut active_runs = Vec::new();
        let mut recent_manual = Vec::new();
        let mut buckets: std::collections::BTreeMap<String, TerminalBucket> = Default::default();

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
            if !terminal {
                active_runs.push(ActiveRunBar {
                    run_id: id.to_string(),
                    workflow_name: workflow_name.clone(),
                    status: status.to_string(),
                    started_at,
                    trigger: trigger.to_string(),
                    cycle_id: cycle_of.get(id).cloned(),
                });
            } else {
                let key = started_at.format("%Y-%m-%d").to_string();
                let b = buckets.entry(key).or_insert(TerminalBucket {
                    ts: started_at,
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

            // A run belonging to a cycle is grouped under that cycle in the
            // feed even when it has no trigger provenance of its own — it must
            // never ALSO leak into recent_manual, or the same run renders twice
            // (once under its cycle, once standalone). That double-listing is
            // the exact autoflow-flooding bug this redesign exists to fix.
            // The local build_summary has the identical guard.
            if trigger == "manual" && !cycle_of.contains_key(id) {
                recent_manual.push(RecentRun {
                    id: id.to_string(),
                    workflow_name,
                    status: status.to_string(),
                    started_at,
                    finished_at: row
                        .get("finished_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&chrono::Utc)),
                    trigger: "manual".to_string(),
                });
            }
        }

        active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

        Ok(DashboardSummary {
            active,
            terminal_buckets: buckets.into_values().collect(),
            active_runs,
            cycles,
            recent_manual,
            // Findings are not exposed by the CLI; 0 here means "not reported by
            // this host", and the aggregate sums only hosts that report.
            findings_open: 0,
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
        let argv = ssh_argv("edge", None, None, "'true'");
        assert!(!argv.iter().any(|a| a == "-i"));
        assert!(!argv.iter().any(|a| a == "-p"));
        assert!(argv.iter().any(|a| a == "edge"));
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
        /// If set, returned as stdout when `run()` is called for a `cat …` command.
        cat_stdout: Option<String>,
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
                // Return canned cat_stdout for the `cat …/run.json` call so the
                // pump can read back the terminal status.
                let stdout = if remote.starts_with("cat ") {
                    self.cat_stdout.clone().unwrap_or_default()
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
        let runs_json = r#"{"kind":"run_list","version":1,"rows":[
            {"id":"r1","workflow_name":"w","status":"running",
             "started_at":"2026-07-16T14:02:11Z","finished_at":null,"trigger":"manual",
             "usage":{"input_tokens":0,"output_tokens":0,"cached_tokens":0,
                      "total_tokens":0,"cost_usd":null,"priced":false,"runs":1},
             "turns":0,"duration_ms":null},
            {"id":"r2","workflow_name":"w","status":"awaiting_approval",
             "started_at":"2026-07-16T14:03:11Z","finished_at":null,"trigger":"cron",
             "usage":{"input_tokens":0,"output_tokens":0,"cached_tokens":0,
                      "total_tokens":0,"cost_usd":null,"priced":false,"runs":1},
             "turns":0,"duration_ms":null}
        ],"summary":{"count":2,"limit":10000,"status_filter":null}}"#;
        let cycles_json = r#"{"kind":"autoflow_history","version":1,"rows":[]}"#;

        let stub = std::sync::Arc::new(StubExec {
            runs_json: runs_json.into(),
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
        assert_eq!(
            s.active_runs.len(),
            2,
            "both non-terminal runs become swimlane bars"
        );
        assert!(
            s.captured_at >= before,
            "captured_at must be stamped when the host was actually read"
        );
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

    // ── Workspace sync (stage/collect/discard) tests ─────────────────────────

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
