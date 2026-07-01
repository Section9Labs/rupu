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
        mirror_get_run, mirror_list_runs, mirror_stream_run_events, read_transcript_file,
        EventByteStream, HostCapabilities, HostConnector, HostConnectorError, HostInfo,
        RunListQuery,
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
    pub pricing: rupu_config::PricingConfig,
}

impl SshHostConnector {
    /// Construct a new connector.
    pub fn new(
        host_id: impl Into<String>,
        exec: Arc<dyn RemoteExec>,
        mirror: Arc<NodeMirror>,
        run_store: Arc<RunStore>,
        pricing: rupu_config::PricingConfig,
    ) -> Self {
        Self {
            host_id: host_id.into(),
            exec,
            mirror,
            run_store,
            pricing,
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
}

#[async_trait::async_trait]
impl HostConnector for SshHostConnector {
    async fn info(&self) -> Result<HostInfo, HostConnectorError> {
        let probe = build_remote_command(&["true".to_string()]);
        let reachable = matches!(self.exec.run(&probe).await, Ok(o) if o.success);
        Ok(HostInfo {
            reachable,
            version: None,
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

    async fn list_runs(
        &self,
        params: RunListQuery,
    ) -> Result<Vec<serde_json::Value>, HostConnectorError> {
        mirror_list_runs(&self.run_store, &self.host_id, &params, &self.pricing)
    }

    async fn get_run(&self, run_id: &str) -> Result<serde_json::Value, HostConnectorError> {
        mirror_get_run(&self.run_store, &self.host_id, run_id, &self.pricing)
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

    // ── Workspace sync (DEFERRED) ─────────────────────────────────────────────
    //
    // A correct SSH staging impl ships the wire-encoded payload to the host and
    // runs the codec via the *remote* `rupu` binary (the host needs no git/tar
    // of its own — the codec lives in the binary). That requires a hidden
    // remote helper subcommand (`rupu __workspace stage|collect`) which is a
    // cross-crate change to rupu-cli's dispatcher; it is intentionally deferred
    // to a follow-up rather than shipped as a silent self-contained fallback.
    // Until then SSH hosts explicitly report the capability as unsupported, so
    // a workspace-sync fan-out to an SSH host fails loudly instead of silently
    // dropping the unit's changes. Local and HttpCp transports are fully wired.
    async fn stage_workspace(&self, _payload: Vec<u8>) -> Result<String, HostConnectorError> {
        tracing::warn!(
            host = %self.host_id,
            "workspace sync over SSH is not yet implemented (deferred); \
             returning Unsupported"
        );
        Err(HostConnectorError::Unsupported(
            "workspace sync over ssh (deferred)".into(),
        ))
    }

    async fn collect_workspace_delta(
        &self,
        _working_dir: &str,
    ) -> Result<Vec<u8>, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "workspace sync over ssh (deferred)".into(),
        ))
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

    // ── SshHostConnector tests ────────────────────────────────────────────────

    use crate::host::connector::HostConnectorError;

    struct FakeExec {
        commands: std::sync::Mutex<Vec<String>>,
        tail_lines: Vec<String>,
        fail: bool,
        fail_stderr: String,
        /// If set, returned as stdout when `run()` is called for a `cat …` command.
        cat_stdout: Option<String>,
    }

    impl FakeExec {
        fn ok(tail_lines: Vec<String>) -> Self {
            Self {
                commands: Default::default(),
                tail_lines,
                fail: false,
                fail_stderr: String::new(),
                cat_stdout: None,
            }
        }

        fn offline(stderr: impl Into<String>) -> Self {
            Self {
                commands: Default::default(),
                tail_lines: vec![],
                fail: true,
                fail_stderr: stderr.into(),
                cat_stdout: None,
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
    }

    fn make_conn(
        fake: std::sync::Arc<FakeExec>,
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
        let conn = SshHostConnector::new(
            "host_abc",
            fake,
            mirror,
            std::sync::Arc::clone(&run_store),
            rupu_config::PricingConfig::default(),
        );
        (conn, run_store, tmp)
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
}
