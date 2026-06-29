//! `rupu node` — dial-home agent + local enroll helper.
//!
//! **Agent mode** (`rupu node --cp-url <wss://...> --token <tok>`):
//! Connects this machine to a remote rupu-cp as a tunnel node.  Sends
//! `Hello`, awaits `Welcome`, then processes inbound frames: `Run` →
//! spawn `rupu workflow run` / `rupu run`, tail artifact files, stream
//! `Artifact` frames back; `Cancel` → kill child; `Ping` → `Pong`.
//! Reconnects with exponential backoff (1 s … 60 s cap) on disconnect.
//!
//! **Enroll mode** (`rupu node enroll <name>`):
//! Mints a tunnel host + one-time token in the local host store and
//! prints the `rupu node --cp-url ... --token ...` command to run on
//! the box.
//!
//! ## Node identity
//!
//! The node id is a `node_<ULID>` string persisted on first run at
//! `~/.rupu/node_id` (plain text, one line).  Pass `--node-id <id>` to
//! override.

#![deny(clippy::all)]

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Subcommand;
use futures_util::{SinkExt, StreamExt};
use rupu_cp::node::protocol::{ArtifactFile, Auth, Frame, RunSpec, RunSpecKind};
use rupu_workspace::{enroll_node, HostStore};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};
use ulid::Ulid;

// ---------------------------------------------------------------------------
// Clap types
// ---------------------------------------------------------------------------

/// Args for `rupu node`.  When no subcommand is given the node agent
/// runs; `enroll` is the only subcommand.
#[derive(clap::Args, Debug)]
pub struct NodeArgs {
    /// WebSocket URL of the control-plane node endpoint
    /// (e.g. `wss://cp.example.com`).  Required in agent mode
    /// (when no subcommand is given).
    #[arg(long)]
    pub cp_url: Option<String>,

    /// Authentication token.  Mutually exclusive with `--token-stdin`.
    #[arg(long, conflicts_with = "token_stdin")]
    pub token: Option<String>,

    /// Read the authentication token from stdin (one line).
    /// Preferred over `--token` so the secret does not land in shell
    /// history.  Mutually exclusive with `--token`.
    #[arg(long)]
    pub token_stdin: bool,

    /// Override the node identity string.  Default: a persistent
    /// `node_<ULID>` written to `~/.rupu/node_id` on first run.
    #[arg(long)]
    pub node_id: Option<String>,

    #[command(subcommand)]
    pub action: Option<NodeAction>,
}

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// Enroll a new tunnel node in the local host store and print
    /// the `rupu node --cp-url ... --token ...` command to run on
    /// the box.  The token is shown ONCE and never persisted on disk.
    Enroll {
        /// Display name for the node (e.g. `build-box-01`).
        name: String,

        /// CP URL hint to include in the printed command
        /// (e.g. `wss://cp.example.com`).  Informational only — the
        /// printed command is a copy-paste template for the operator.
        #[arg(long)]
        cp_url: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Active-run bookkeeping (module-level, not inside a function)
// ---------------------------------------------------------------------------

struct RunState {
    child: tokio::process::Child,
    offsets: FileOffsets,
}

struct FileOffsets {
    events: u64,
    step_results: u64,
    unit_checkpoints: u64,
}

// ---------------------------------------------------------------------------
// Public handler
// ---------------------------------------------------------------------------

pub async fn handle(args: NodeArgs) -> ExitCode {
    let result = match args.action {
        Some(NodeAction::Enroll { name, cp_url }) => {
            enroll_inner(&name, cp_url.as_deref())
        }
        None => {
            let Some(cp_url) = args.cp_url else {
                eprintln!(
                    "error: --cp-url is required in node agent mode\n\
                     hint: rupu node --cp-url wss://<cp-host> --token <token>"
                );
                return ExitCode::FAILURE;
            };
            let token = match resolve_token(args.token, args.token_stdin) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    eprintln!("error: provide --token <tok> or --token-stdin");
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    eprintln!("error: {e:#}");
                    return ExitCode::FAILURE;
                }
            };
            let node_id = match resolve_node_id(args.node_id) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    return ExitCode::FAILURE;
                }
            };
            run_agent_loop(&cp_url, &token, &node_id).await
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => crate::output::diag::fail(e),
    }
}

// ---------------------------------------------------------------------------
// Enroll
// ---------------------------------------------------------------------------

fn enroll_inner(name: &str, cp_url: Option<&str>) -> anyhow::Result<()> {
    let global = crate::paths::global_dir()?;
    let store = HostStore { root: global.join("hosts") };
    let (host, token) = enroll_node(&store, name).context("enroll node in host store")?;
    let cp_placeholder = cp_url.unwrap_or("wss://<cp-host>");
    println!("enrolled: {} ({})", host.name, host.id);
    println!();
    println!("⚠  token shown ONCE — copy it to the node now:");
    println!();
    println!(
        "  rupu node --cp-url {cp} --token {tok} --node-id {nid}",
        cp = cp_placeholder,
        tok = token,
        nid = host.id,
    );
    println!();
    println!("Or to keep the token out of shell history:");
    println!();
    println!(
        "  printf '%s' '{tok}' | rupu node --cp-url {cp} --token-stdin --node-id {nid}",
        tok = token,
        cp = cp_placeholder,
        nid = host.id,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Token resolution
// ---------------------------------------------------------------------------

fn resolve_token(flag: Option<String>, stdin: bool) -> anyhow::Result<Option<String>> {
    if let Some(t) = flag {
        return Ok(Some(t));
    }
    if stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("read token from stdin")?;
        let t = buf.trim().to_string();
        if t.is_empty() {
            anyhow::bail!("--token-stdin: no token received on stdin");
        }
        return Ok(Some(t));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Node id persistence  (~/.rupu/node_id)
// ---------------------------------------------------------------------------

fn resolve_node_id(override_id: Option<String>) -> anyhow::Result<String> {
    if let Some(id) = override_id {
        return Ok(id);
    }
    let global = crate::paths::global_dir()?;
    let node_id_path = global.join("node_id");
    if node_id_path.is_file() {
        let raw = std::fs::read_to_string(&node_id_path).context("read node_id file")?;
        let id = raw.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    let id = format!("node_{}", Ulid::new());
    crate::paths::ensure_dir(&global)?;
    std::fs::write(&node_id_path, &id).context("write node_id file")?;
    info!(
        node_id = %id,
        path = %node_id_path.display(),
        "node: generated stable node id"
    );
    Ok(id)
}

// ---------------------------------------------------------------------------
// Agent loop (reconnect with exponential backoff)
// ---------------------------------------------------------------------------

async fn run_agent_loop(cp_url: &str, token: &str, node_id: &str) -> anyhow::Result<()> {
    if cp_url.starts_with("ws://") {
        warn!(
            url = %cp_url,
            "node: connecting over plaintext ws:// — use wss:// in production"
        );
    }

    let exe = std::env::current_exe().context("resolve current executable path")?;
    let mut backoff_secs: u64 = 1;
    const BACKOFF_CAP: u64 = 60;

    loop {
        info!(url = %cp_url, node_id = %node_id, "node: connecting");
        match connect_and_run(cp_url, token, node_id, &exe).await {
            Ok(()) => {
                warn!("node: connection closed; reconnecting in {backoff_secs}s");
            }
            Err(e) => {
                warn!(error = %e, "node: connection error; reconnecting in {backoff_secs}s");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(BACKOFF_CAP);
    }
}

// ---------------------------------------------------------------------------
// Single connection lifetime
// ---------------------------------------------------------------------------

async fn connect_and_run(
    cp_url: &str,
    token: &str,
    node_id: &str,
    exe: &Path,
) -> anyhow::Result<()> {
    // Dial the CP.
    let (ws_stream, _) = tokio_tungstenite::connect_async(cp_url)
        .await
        .context("ws connect")?;
    let (mut sink, mut stream) = ws_stream.split();

    // Send Hello.
    let hello = Frame::Hello {
        node_id: node_id.to_string(),
        auth: Auth::Token {
            token: token.to_string(),
        },
        rupu_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: vec![],
    };
    sink.send(Message::Text(serde_json::to_string(&hello)?))
        .await
        .context("send Hello")?;

    // Await Welcome.
    let welcome_msg = stream
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("server closed before Welcome"))?
        .context("recv Welcome")?;
    let welcome_frame = parse_frame(&welcome_msg)?;
    if !matches!(welcome_frame, Frame::Welcome {}) {
        anyhow::bail!(
            "expected Welcome from server, got: {}",
            serde_json::to_string(&welcome_frame).unwrap_or_else(|_| "?".into())
        );
    }
    info!(node_id = %node_id, "node: authenticated (Welcome received)");

    // Runs root: <global>/runs/<run_id>/
    let global = crate::paths::global_dir()?;
    let runs_root = global.join("runs");

    let mut active: HashMap<String, RunState> = HashMap::new();

    loop {
        // Interleave: poll artifact files every 250 ms, or process a WS frame immediately.
        let sleep_fut = tokio::time::sleep(std::time::Duration::from_millis(250));
        tokio::pin!(sleep_fut);

        let maybe_msg = tokio::select! {
            msg = stream.next() => match msg {
                None => break,                            // server closed cleanly
                Some(m) => Some(m.context("recv frame")?),
            },
            _ = &mut sleep_fut => None,
        };

        // Drain artifact files for all active runs.
        let run_ids: Vec<String> = active.keys().cloned().collect();
        let mut finished: Vec<String> = Vec::new();

        for rid in &run_ids {
            let state = active.get_mut(rid).expect("run_ids came from active");
            let run_dir = runs_root.join(rid);

            // events.jsonl
            for line in drain_new_lines(&run_dir.join("events.jsonl"), &mut state.offsets.events) {
                send_artifact(&mut sink, rid, ArtifactFile::Events, line).await;
            }
            // step_results.jsonl
            for line in drain_new_lines(
                &run_dir.join("step_results.jsonl"),
                &mut state.offsets.step_results,
            ) {
                send_artifact(&mut sink, rid, ArtifactFile::StepResults, line).await;
            }
            // unit_checkpoints.jsonl
            for line in drain_new_lines(
                &run_dir.join("unit_checkpoints.jsonl"),
                &mut state.offsets.unit_checkpoints,
            ) {
                send_artifact(&mut sink, rid, ArtifactFile::UnitCheckpoints, line).await;
            }
            // run.json — check for terminal status.
            if let Some((status, body)) = read_terminal_status(&run_dir.join("run.json")) {
                send_artifact(&mut sink, rid, ArtifactFile::RunJson, body).await;
                let frame = Frame::RunFinished {
                    run_id: rid.clone(),
                    status,
                };
                send_frame(&mut sink, &frame).await;
                finished.push(rid.clone());
            }
        }
        for rid in finished {
            active.remove(&rid);
        }

        // Process incoming WS frame (if one arrived).
        let Some(msg) = maybe_msg else {
            continue;
        };
        let frame = parse_frame(&msg)?;
        match frame {
            Frame::Run { run_id, spec } => {
                info!(run_id = %run_id, "node: Run received");
                match spawn_run(exe, &run_id, &spec) {
                    Ok(child) => {
                        active.insert(
                            run_id,
                            RunState {
                                child,
                                offsets: FileOffsets {
                                    events: 0,
                                    step_results: 0,
                                    unit_checkpoints: 0,
                                },
                            },
                        );
                    }
                    Err(e) => {
                        warn!(run_id = %run_id, error = %e, "node: spawn failed");
                        let err_frame = Frame::RunFinished {
                            run_id: run_id.clone(),
                            status: "failed".to_string(),
                        };
                        send_frame(&mut sink, &err_frame).await;
                    }
                }
            }
            Frame::Cancel { run_id } => {
                info!(run_id = %run_id, "node: Cancel received");
                if let Some(mut state) = active.remove(&run_id) {
                    // Kill the direct child.  Process group would give
                    // a cleaner kill of any grandchildren, but requires
                    // `process_group(0)` at spawn time and libc kill(−pgid)
                    // for the signal — both are safe but add complexity.
                    // `start_kill` on the tokio Child is sufficient here;
                    // the grandchildren will be reparented to init/launchd
                    // and eventually exit naturally.
                    //
                    // NOTE: We set process_group(0) at spawn time (see
                    // `spawn_run`), so the child is already in its own
                    // process group.  To kill the whole group without unsafe
                    // would require nix/libc; we only kill the direct child
                    // here and note this limitation.
                    if let Err(e) = state.child.start_kill() {
                        warn!(run_id = %run_id, error = %e, "node: kill child failed");
                    }
                    let cancelled_frame = Frame::RunFinished {
                        run_id: run_id.clone(),
                        status: "cancelled".to_string(),
                    };
                    send_frame(&mut sink, &cancelled_frame).await;
                } else {
                    warn!(run_id = %run_id, "node: Cancel for unknown run_id (ignored)");
                }
            }
            Frame::Ping {} => {
                send_frame(&mut sink, &Frame::Pong {}).await;
            }
            Frame::Hello { .. }
            | Frame::Welcome {}
            | Frame::Pong {}
            | Frame::Artifact { .. }
            | Frame::RunFinished { .. } => {
                warn!(
                    frame = %serde_json::to_string(&frame).unwrap_or_else(|_| "?".into()),
                    "node: unexpected server-sent frame type (ignored)"
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Run spawning
// ---------------------------------------------------------------------------

fn spawn_run(exe: &Path, run_id: &str, spec: &RunSpec) -> anyhow::Result<tokio::process::Child> {
    let argv = build_argv(run_id, spec);
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Own process group on Unix so a future kill of the direct child
    // does not cascade to the node agent itself via SIGINT propagation.
    // `process_group(0)` is safe (no `unsafe` block required).
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn().context("spawn rupu child")
}

/// Build the argv (after the executable) for a local `rupu workflow run`
/// or `rupu run` invocation dispatched by the node agent.
///
/// Workflow: `workflow run <name> [<target>] --run-id <id> --plain [--input k=v]… [--mode m]`
/// Agent:    `run <name> [<target>] --run-id <id> [--mode m] [--prompt p] [--tmp (if target)]`
///
/// Flag names are verified against the clap definitions in `cmd/workflow.rs`
/// (`--run-id`, `--plain`, `--input`, `--mode`) and `cmd/run.rs`
/// (`--run-id`, `--mode`, `--prompt`, `--tmp`).
pub(crate) fn build_argv(run_id: &str, spec: &RunSpec) -> Vec<String> {
    match spec.kind {
        RunSpecKind::Workflow => {
            let mut argv = vec!["workflow".to_string(), "run".to_string(), spec.name.clone()];
            if let Some(t) = &spec.target {
                argv.push(t.clone());
            }
            argv.push("--run-id".to_string());
            argv.push(run_id.to_string());
            argv.push("--plain".to_string());
            for (k, v) in &spec.inputs {
                argv.push("--input".to_string());
                argv.push(format!("{k}={v}"));
            }
            if let Some(m) = &spec.mode {
                argv.push("--mode".to_string());
                argv.push(m.clone());
            }
            argv
        }
        RunSpecKind::Agent => {
            let mut argv = vec!["run".to_string(), spec.name.clone()];
            if let Some(t) = &spec.target {
                argv.push(t.clone());
            }
            argv.push("--run-id".to_string());
            argv.push(run_id.to_string());
            if let Some(m) = &spec.mode {
                argv.push("--mode".to_string());
                argv.push(m.clone());
            }
            if let Some(p) = &spec.prompt {
                argv.push("--prompt".to_string());
                argv.push(p.clone());
            }
            if spec.target.is_some() {
                argv.push("--tmp".to_string());
            }
            argv
        }
    }
}

// ---------------------------------------------------------------------------
// Artifact tail helper  (unit-testable)
// ---------------------------------------------------------------------------

/// Drain any new complete lines appended to `path` since `*offset` bytes.
///
/// Reads the file atomically, returns only the bytes after `*offset`,
/// advances `*offset` past the last newline consumed, and returns the
/// completed lines as `String`s (without a trailing newline).
///
/// Partial lines (no trailing `\n` yet) are left for the next call.
/// If the file does not exist or cannot be read, returns an empty `Vec`
/// and leaves `*offset` unchanged — the caller retries on the next tick.
pub fn drain_new_lines(path: &Path, offset: &mut u64) -> Vec<String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return vec![],
    };
    let file_len = bytes.len() as u64;
    if file_len <= *offset {
        return vec![];
    }
    let new = &bytes[*offset as usize..];
    // Only consume up to and including the last `\n` so partial lines
    // are held back until the writer flushes them.
    let last_nl = match new.iter().rposition(|&b| b == b'\n') {
        Some(idx) => idx,
        None => return vec![],
    };
    let complete = &new[..=last_nl];
    *offset += complete.len() as u64;
    std::str::from_utf8(complete)
        .unwrap_or("")
        .lines()
        .map(|l| l.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Frame / message helpers
// ---------------------------------------------------------------------------

fn parse_frame(msg: &Message) -> anyhow::Result<Frame> {
    let text = match msg {
        Message::Text(t) => t.as_str(),
        Message::Binary(b) => {
            let s = std::str::from_utf8(b).context("binary WS frame as UTF-8")?;
            return serde_json::from_str(s).context("parse Frame from binary");
        }
        Message::Close(_) => anyhow::bail!("server sent Close frame"),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
            anyhow::bail!("unexpected low-level WS message")
        }
    };
    serde_json::from_str(text).context("parse Frame JSON")
}

async fn send_frame<S>(sink: &mut S, frame: &Frame)
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match serde_json::to_string(frame) {
        Ok(s) => {
            if let Err(e) = sink.send(Message::Text(s)).await {
                warn!(error = %e, "node: WS send error");
            }
        }
        Err(e) => {
            warn!(error = %e, "node: failed to serialize outbound frame");
        }
    }
}

async fn send_artifact<S>(sink: &mut S, run_id: &str, file: ArtifactFile, line: String)
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = Frame::Artifact {
        run_id: run_id.to_string(),
        file,
        line,
    };
    send_frame(sink, &frame).await;
}

// ---------------------------------------------------------------------------
// Read terminal status from run.json
// ---------------------------------------------------------------------------

/// Parse `run.json` and return `(status_str, raw_body)` if the run has
/// reached a terminal status; `None` if still in-flight, unreadable, or
/// not yet written.
fn read_terminal_status(run_json: &Path) -> Option<(String, String)> {
    let body = std::fs::read_to_string(run_json).ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let status = v.get("status")?.as_str()?;
    let terminal = matches!(status, "completed" | "failed" | "rejected" | "cancelled");
    if terminal {
        Some((status.to_string(), body))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_cp::node::protocol::RunSpecKind;
    use std::collections::BTreeMap;
    use std::io::Write;
    use tempfile::tempdir;

    // ------------------------------------------------------------------
    // drain_new_lines: the unit-testable tail helper
    // ------------------------------------------------------------------

    /// Write N lines to a temp file, drain once → N lines in order;
    /// drain again (same offset) → empty; append 2 more → get those 2.
    #[test]
    fn drain_new_lines_basic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 0..3u32 {
            writeln!(f, r#"{{"type":"ev","n":{i}}}"#).unwrap();
        }
        f.flush().unwrap();
        drop(f);

        let mut offset = 0u64;

        // First drain: 3 lines.
        let lines = drain_new_lines(&path, &mut offset);
        assert_eq!(lines.len(), 3, "expected 3 lines, got: {lines:?}");
        assert!(lines[0].contains("\"n\":0"), "line 0: {}", lines[0]);
        assert!(lines[1].contains("\"n\":1"), "line 1: {}", lines[1]);
        assert!(lines[2].contains("\"n\":2"), "line 2: {}", lines[2]);

        // Second drain with advanced offset → nothing new.
        let more = drain_new_lines(&path, &mut offset);
        assert!(more.is_empty(), "second drain should be empty, got: {more:?}");

        // Append 2 more lines → only those come back.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, r#"{{"type":"ev","n":3}}"#).unwrap();
        writeln!(f, r#"{{"type":"ev","n":4}}"#).unwrap();
        f.flush().unwrap();
        drop(f);

        let new_lines = drain_new_lines(&path, &mut offset);
        assert_eq!(new_lines.len(), 2, "expected 2 new lines, got: {new_lines:?}");
        assert!(new_lines[0].contains("\"n\":3"), "new[0]: {}", new_lines[0]);
        assert!(new_lines[1].contains("\"n\":4"), "new[1]: {}", new_lines[1]);
    }

    /// Missing file → empty Vec, offset unchanged.
    #[test]
    fn drain_new_lines_missing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let mut offset = 0u64;
        let lines = drain_new_lines(&path, &mut offset);
        assert!(lines.is_empty());
        assert_eq!(offset, 0, "offset must not advance for a missing file");
    }

    /// Partial line (no trailing newline) is not returned until complete.
    #[test]
    fn drain_new_lines_partial_line_held_back() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("partial.jsonl");
        // Write without a trailing newline.
        std::fs::write(&path, b"{\"partial\":true}").unwrap();

        let mut offset = 0u64;
        let lines = drain_new_lines(&path, &mut offset);
        assert!(
            lines.is_empty(),
            "partial line must be held back until newline: {lines:?}"
        );
        assert_eq!(offset, 0, "offset must not advance for partial line");

        // Append the newline → now the line is returned.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f).unwrap();
        drop(f);

        let lines = drain_new_lines(&path, &mut offset);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("partial"));
    }

    // ------------------------------------------------------------------
    // build_argv: argv builders
    // ------------------------------------------------------------------

    #[test]
    fn build_argv_workflow_full() {
        let mut inputs = BTreeMap::new();
        inputs.insert("k".to_string(), "v".to_string());
        inputs.insert("a".to_string(), "b".to_string());
        let spec = RunSpec {
            kind: RunSpecKind::Workflow,
            name: "audit".to_string(),
            inputs,
            prompt: None,
            mode: Some("bypass".to_string()),
            target: Some("github:o/r".to_string()),
        };
        let argv = build_argv("run_X", &spec);
        assert_eq!(
            argv,
            vec![
                "workflow",
                "run",
                "audit",
                "github:o/r",
                "--run-id",
                "run_X",
                "--plain",
                "--input",
                "a=b",
                "--input",
                "k=v",
                "--mode",
                "bypass",
            ]
        );
    }

    #[test]
    fn build_argv_workflow_minimal() {
        let spec = RunSpec {
            kind: RunSpecKind::Workflow,
            name: "simple".to_string(),
            inputs: BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        let argv = build_argv("run_Y", &spec);
        assert_eq!(
            argv,
            vec!["workflow", "run", "simple", "--run-id", "run_Y", "--plain"]
        );
    }

    #[test]
    fn build_argv_agent_full() {
        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: "triage".to_string(),
            inputs: BTreeMap::new(),
            prompt: Some("look at this PR".to_string()),
            mode: Some("bypass".to_string()),
            target: Some("github:o/r".to_string()),
        };
        let argv = build_argv("run_Z", &spec);
        assert_eq!(
            argv,
            vec![
                "run",
                "triage",
                "github:o/r",
                "--run-id",
                "run_Z",
                "--mode",
                "bypass",
                "--prompt",
                "look at this PR",
                "--tmp",
            ]
        );
    }

    #[test]
    fn build_argv_agent_minimal() {
        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: "check".to_string(),
            inputs: BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        };
        let argv = build_argv("run_W", &spec);
        assert_eq!(argv, vec!["run", "check", "--run-id", "run_W"]);
    }
}
