//! `rupu node` — dial-home agent + local enroll helper.
//!
//! **Agent mode** (`rupu node --cp-url ws://<host>:7878/api/node/connect --token <tok>`):
//! Connects this machine to a remote rupu-cp as a tunnel node.  The
//! `/api/node/connect` path is appended automatically when the URL has
//! no path; `wss://` is rejected up front (this build has no TLS —
//! front the CP with a TLS-terminating proxy and rebuild with TLS
//! support to use it).  Sends `Hello`, awaits `Welcome`, then processes
//! inbound frames: `Run` → spawn `rupu workflow run` / `rupu run`, tail
//! artifact files, stream `Artifact` frames back; `Cancel` → kill
//! child; `Ping` → `Pong`; `ArtifactPull` → stream a finding-artifact blob
//! from this node's store, one chunk per loop turn, round-robin across
//! concurrent pulls.  Reconnects with exponential backoff (1 s … 60 s cap)
//! on disconnect.
//!
//! **Enroll mode** (`rupu node enroll <name>`):
//! Mints a tunnel host + one-time token in the local host store and
//! prints the ready-to-run `rupu node --cp-url ... --token ...`
//! command (this machine's routable IP, port 7878, full endpoint path).
//!
//! ## Node identity
//!
//! The node id is a `node_<ULID>` string persisted on first run at
//! `~/.rupu/node_id` (plain text, one line).  Pass `--node-id <id>` to
//! override.

#![deny(clippy::all)]

use std::collections::{HashMap, VecDeque};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Subcommand;
use futures_util::{SinkExt, StreamExt};
use rupu_cp::host::bucket::{Bucket, BucketError, ControlEnvelope, ObjectStoreBucket};
use rupu_cp::node::protocol::{
    ArtifactFile, Auth, Frame, RunSpec, RunSpecKind, ARTIFACT_CHUNK_BYTES, CAP_USAGE_LEDGER,
};
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
    /// (e.g. `ws://cp.example.com:7878/api/node/connect`; the
    /// `/api/node/connect` path is appended automatically when omitted).
    /// `wss://` requires a TLS-terminating proxy and a TLS-enabled build.
    /// Required in agent mode (when no subcommand is given).
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

        /// CP URL to include in the printed command
        /// (e.g. `ws://cp.example.com:7878`; the `/api/node/connect`
        /// path is appended automatically when omitted).  Default: this
        /// machine's detected routable IP on port 7878.
        #[arg(long)]
        cp_url: Option<String>,
    },

    /// Poll a bucket dead-drop, atomically claim jobs, run them locally,
    /// write results back, and apply queued control messages.
    Pull(PullArgs),
}

/// Args for `rupu node pull`.
#[derive(clap::Args, Debug)]
pub struct PullArgs {
    /// Bucket URL (e.g. `s3://my-bucket`, `gs://my-bucket`).
    /// Credentials are resolved via the environment credential chain.
    #[arg(long)]
    pub bucket: String,

    /// Optional key prefix within the bucket (e.g. `rupu/host-1`).
    #[arg(long)]
    pub prefix: Option<String>,

    /// Override the worker identity.  Default: the stable `node_<ULID>`
    /// persisted at `~/.rupu/node_id` (same as the tunnel node agent).
    #[arg(long)]
    pub host_id: Option<String>,

    /// Claim all currently-available jobs, drain them to terminal (bounded),
    /// then exit.  In loop mode (the default) the agent runs forever.
    #[arg(long)]
    pub once: bool,

    /// Poll interval between ticks in seconds (loop mode only).
    #[arg(long, default_value = "15")]
    pub interval: u64,
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
    /// `usage.jsonl` — the run's usage ledger. Only ever advanced on a
    /// tunnel connection whose CP advertised [`CAP_USAGE_LEDGER`].
    usage: u64,
    /// `coverage.jsonl` — the run's coverage stream. Only ever advanced on a
    /// tunnel connection whose CP advertised `mirror.coverage`.
    coverage: u64,
}

/// One bucket result stream's cursor (events, step_results, ...): how far into
/// the run file has been uploaded, and the next object number.
///
/// Both advance only when `put_result` returned `Ok` ([`upload_new_lines`]). A
/// failed put also pins the exact end of the chunk it carried (`pending_end`),
/// so the retry re-sends that chunk byte for byte under the same key. The CP
/// poller skips keys it already consumed, and a put that errored AFTER
/// landing (a timeout) may already have been consumed — a retry that grew
/// would have its extra lines skipped for good.
#[derive(Debug, Default)]
struct BucketStream {
    /// Bytes of the run file already uploaded.
    offset: u64,
    /// The next result object number for this stream.
    seq: u64,
    /// End offset of a chunk whose put failed and must be retried as-is.
    pending_end: Option<u64>,
}

/// The cursors for every file the bucket worker ships for a run.
#[derive(Debug, Default)]
struct BucketStreams {
    events: BucketStream,
    step_results: BucketStream,
    unit_checkpoints: BucketStream,
    usage: BucketStream,
    coverage: BucketStream,
}

impl BucketStreams {
    /// Upload every stream's new lines. All five are attempted even when one
    /// fails; returns `true` only when nothing is left unsent.
    async fn upload_all(&mut self, bucket: &dyn Bucket, rid: &str, run_dir: &Path) -> bool {
        let mut landed = true;
        landed &= upload_new_lines(
            bucket,
            rid,
            "events",
            &run_dir.join("events.jsonl"),
            &mut self.events,
        )
        .await;
        landed &= upload_new_lines(
            bucket,
            rid,
            "step_results",
            &run_dir.join("step_results.jsonl"),
            &mut self.step_results,
        )
        .await;
        landed &= upload_new_lines(
            bucket,
            rid,
            "unit_checkpoints",
            &run_dir.join("unit_checkpoints.jsonl"),
            &mut self.unit_checkpoints,
        )
        .await;
        // No capability gate on usage / coverage: an older CP's poller skips
        // the unknown `usage.*` / `coverage.*` keys.
        landed &= upload_usage_ledger(bucket, rid, run_dir, &mut self.usage).await;
        landed &= upload_new_lines(
            bucket,
            rid,
            "coverage",
            &coverage_path(run_dir),
            &mut self.coverage,
        )
        .await;
        landed
    }
}

/// Per-run state for the bucket pull agent: the child and the upload cursors,
/// plus a last-applied-control-seq tracker.
struct BucketRunState {
    child: tokio::process::Child,
    streams: BucketStreams,
    /// Highest control seq we've already applied (`None` = none applied yet).
    last_ctrl_seq: Option<u64>,
}

// ---------------------------------------------------------------------------
// Public handler
// ---------------------------------------------------------------------------

pub async fn handle(args: NodeArgs) -> ExitCode {
    let result = match args.action {
        Some(NodeAction::Enroll { name, cp_url }) => enroll_inner(&name, cp_url.as_deref()),
        Some(NodeAction::Pull(pull_args)) => pull(pull_args).await,
        None => {
            let Some(cp_url) = args.cp_url else {
                eprintln!(
                    "error: --cp-url is required in node agent mode\n\
                     hint: rupu node --cp-url ws://<cp-host>:7878 --token <token>"
                );
                return ExitCode::FAILURE;
            };
            let cp_url = normalize_cp_url(&cp_url);
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
    let store = HostStore {
        root: global.join("hosts"),
    };
    let (host, token) = enroll_node(&store, name).context("enroll node in host store")?;
    // Enroll runs on the CP machine (it writes the local host store), so this
    // machine's routable IP is the right default host for the printed command.
    let detected_ip = rupu_cp::net::detect_routable_ip();
    let (cmd, stdin_cmd) = enroll_commands(cp_url, detected_ip, &token, &host.id);
    println!("enrolled: {} ({})", host.name, host.id);
    println!();
    println!("⚠  token shown ONCE — copy it to the node now:");
    println!();
    println!("  {cmd}");
    println!();
    println!("Or to keep the token out of shell history:");
    println!();
    println!("  {stdin_cmd}");
    if cp_url.is_none() && detected_ip.is_none() {
        println!();
        println!("(could not detect this machine's IP — replace <cp-host> with the CP's address)");
    }
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
    // wss:// cannot work in this build — fail fast with an actionable error
    // instead of looping on TlsFeatureNotEnabled.
    reject_wss(cp_url)?;
    if cp_url.starts_with("ws://") {
        warn!(
            url = %cp_url,
            "node: connecting over plaintext ws:// — use wss:// in production"
        );
    }

    let exe = std::env::current_exe().context("resolve current executable path")?;
    let global = crate::paths::global_dir()?;
    let mut backoff_secs: u64 = 1;

    loop {
        // Plain user-facing lines alongside tracing: this is a foreground
        // agent the operator watches, and the CLI only surfaces warn+ from
        // tracing — without these a successful connect is silent.
        eprintln!("connecting to {cp_url} as {node_id} …");
        info!(url = %cp_url, node_id = %node_id, "node: connecting");
        match connect_and_run(cp_url, token, node_id, &exe, &global).await {
            Ok(()) => {
                // Clean close: reset backoff so the next attempt is prompt.
                backoff_secs = 1;
                eprintln!("connection closed — reconnecting in {backoff_secs}s");
                warn!("node: connection closed; reconnecting in {backoff_secs}s");
            }
            Err(e) => {
                eprintln!("connection failed: {e:#} — retrying in {backoff_secs}s");
                warn!(error = %e, "node: connection error; reconnecting in {backoff_secs}s");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = next_backoff(backoff_secs);
    }
}

/// Compute the next exponential-backoff interval, capped at [`BACKOFF_CAP`].
const BACKOFF_CAP: u64 = 60;
fn next_backoff(current: u64) -> u64 {
    (current * 2).min(BACKOFF_CAP)
}

// ---------------------------------------------------------------------------
// CP URL handling (normalization, scheme guard, enroll command builder)
// ---------------------------------------------------------------------------

/// The CP's node WebSocket endpoint path (see `rupu-cp`'s `node::server`).
const NODE_CONNECT_PATH: &str = "/api/node/connect";

/// Normalize a CP URL for the node agent: when the URL carries no path (or
/// just `/`), append [`NODE_CONNECT_PATH`]. A URL that already has a path is
/// used verbatim, so operators can point at a reverse proxy with a custom
/// prefix.
fn normalize_cp_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        // No scheme — leave it alone; the connect call will surface the error.
        return url.to_string();
    };
    let authority_start = scheme_end + 3;
    let rest = &url[authority_start..];
    match rest.find('/') {
        // No path at all: `ws://h:7878` → append.
        None => format!("{url}{NODE_CONNECT_PATH}"),
        // Bare trailing slash: `ws://h:7878/` → replace with the endpoint.
        Some(idx) if rest[idx..].len() == 1 => {
            format!("{}{NODE_CONNECT_PATH}", &url[..authority_start + idx])
        }
        // Real path present: use verbatim.
        Some(_) => url.to_string(),
    }
}

/// Fail fast on `wss://`: this build compiles `tokio-tungstenite` without a
/// TLS feature, so a `wss://` dial fails instantly with
/// `TlsFeatureNotEnabled`. Surface an actionable error instead.
fn reject_wss(url: &str) -> anyhow::Result<()> {
    if url.starts_with("wss://") {
        anyhow::bail!(
            "wss:// is not supported by this build (no TLS support compiled in) — \
             use ws:// (plaintext; keep it on a trusted network), or front the CP \
             with a TLS-terminating proxy and rebuild with TLS support"
        );
    }
    Ok(())
}

/// Build the two copy-paste commands `rupu node enroll` prints:
/// `(--token variant, --token-stdin variant)`.
///
/// When `--cp-url` was given it is normalized (path appended if missing);
/// otherwise the detected routable IP of this machine (enroll runs on the CP
/// box) with the default CP port 7878 is used, falling back to a
/// `<cp-host>` placeholder when detection fails.
fn enroll_commands(
    cp_url_flag: Option<&str>,
    detected_ip: Option<std::net::IpAddr>,
    token: &str,
    node_id: &str,
) -> (String, String) {
    let cp = match cp_url_flag {
        Some(u) => normalize_cp_url(u),
        None => match detected_ip {
            Some(std::net::IpAddr::V6(v6)) => format!("ws://[{v6}]:7878{NODE_CONNECT_PATH}"),
            Some(ip) => format!("ws://{ip}:7878{NODE_CONNECT_PATH}"),
            None => format!("ws://<cp-host>{NODE_CONNECT_PATH}"),
        },
    };
    (
        format!("rupu node --cp-url {cp} --token {token} --node-id {node_id}"),
        format!(
            "printf '%s' '{token}' | rupu node --cp-url {cp} --token-stdin --node-id {node_id}"
        ),
    )
}

// ---------------------------------------------------------------------------
// Single connection lifetime
// ---------------------------------------------------------------------------

/// How often the connection loop polls active runs' artifact files when no
/// inbound frame wakes it.
const RUN_FILE_POLL: std::time::Duration = std::time::Duration::from_millis(250);

async fn connect_and_run(
    cp_url: &str,
    token: &str,
    node_id: &str,
    exe: &Path,
    global: &Path,
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
        capabilities: rupu_cp::node::protocol::node_capabilities(),
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
    if !matches!(welcome_frame, Frame::Welcome { .. }) {
        anyhow::bail!(
            "expected Welcome from server, got: {}",
            serde_json::to_string(&welcome_frame).unwrap_or_else(|_| "?".into())
        );
    }
    // Only forward the usage ledger to a CP that said it mirrors it: an older
    // CP can't parse the `usage` artifact kind and would log-and-drop every
    // line. (Bucket hosts need no gate — an older poller just skips the key.)
    let usage_ok = welcome_advertises(&welcome_frame, CAP_USAGE_LEDGER);
    // Only a CP that advertised it can parse `ArtifactFile::Coverage` frames;
    // an older CP would reject the unknown variant, so the pump never sends
    // one unless this is set.
    let mirror_coverage =
        welcome_advertises(&welcome_frame, rupu_cp::node::protocol::CAP_MIRROR_COVERAGE);
    eprintln!("connected ✓ (authenticated as {node_id})");
    info!(node_id = %node_id, "node: authenticated (Welcome received)");

    // Runs root: <global>/runs/<run_id>/
    let runs_root = global.join("runs");

    let mut active: HashMap<String, RunState> = HashMap::new();
    // Artifact pulls being answered; each loop turn the front one sends one
    // frame and goes to the back (round-robin).
    let mut pulls: VecDeque<ArtifactPullStream> = VecDeque::new();
    let mut last_drain = tokio::time::Instant::now();

    loop {
        // Interleave: poll artifact files every RUN_FILE_POLL, or process a
        // WS frame immediately. While pulls are queued the wait is zero-length
        // so their chunks flow back-to-back — yet every turn still takes an
        // inbound frame first (`biased`), so a large blob never holds up a
        // Cancel, Run or Ping.
        let pulling = !pulls.is_empty();
        let wait = async move {
            if pulling {
                tokio::task::yield_now().await
            } else {
                tokio::time::sleep(RUN_FILE_POLL).await
            }
        };
        tokio::pin!(wait);

        let maybe_msg = tokio::select! {
            biased;
            msg = stream.next() => match msg {
                None => break,                            // server closed cleanly
                Some(m) => Some(m.context("recv frame")?),
            },
            _ = &mut wait => None,
        };

        // Drain artifact files for all active runs — on an inbound frame or
        // once per poll interval, not on every back-to-back pull turn (which
        // would re-read every active run's files once per chunk).
        let drain_due = maybe_msg.is_some() || last_drain.elapsed() >= RUN_FILE_POLL;
        let run_ids: Vec<String> = if drain_due {
            last_drain = tokio::time::Instant::now();
            active.keys().cloned().collect()
        } else {
            Vec::new()
        };
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
            // usage.jsonl — the routine incremental drain.
            for line in drain_usage_ledger(&run_dir, &mut state.offsets, usage_ok) {
                send_artifact(&mut sink, rid, ArtifactFile::Usage, line).await;
            }
            // coverage.jsonl — only a CP that advertised it parses these frames.
            ship_coverage(
                &mut sink,
                rid,
                &run_dir,
                &mut state.offsets.coverage,
                mirror_coverage,
            )
            .await;
            // run.json — check for terminal status.
            if let Some((status, body)) = read_terminal_status(&run_dir.join("run.json")) {
                // A ledger row can land between the drain above and this
                // terminal read, and the run leaves `active` this pass — so
                // one more drain, now that the run is known terminal (the
                // runner writes its ledger rows before the terminal status).
                // It goes BEFORE the terminal `run.json` and `RunFinished`
                // frames: the CP flips the run terminal on those, and a
                // reader that sees it terminal must see the whole ledger.
                // Offsets make it exact-once: rows the earlier drain sent are
                // never re-sent.
                for line in drain_usage_ledger(&run_dir, &mut state.offsets, usage_ok) {
                    send_artifact(&mut sink, rid, ArtifactFile::Usage, line).await;
                }
                // Final coverage drain: the run may have written its last lines
                // after the drain above and before run.json turned terminal.
                ship_coverage(
                    &mut sink,
                    rid,
                    &run_dir,
                    &mut state.offsets.coverage,
                    mirror_coverage,
                )
                .await;
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

        // Answer ONE frame of one artifact pull per turn (R11): a blob of up
        // to hundreds of MiB streamed inline would stall inbound frames and
        // run-file drains for the whole transfer. Pulls take turns
        // round-robin, so a small pull queued behind a large blob still
        // starts — and finishes — within the CP's idle timeout.
        if let Some(mut pull) = pulls.pop_front() {
            if let Some(frame) = pull.next_frame().await {
                send_frame(&mut sink, &frame).await;
            }
            if !pull.is_done() {
                pulls.push_back(pull);
            }
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
                                    usage: 0,
                                    coverage: 0,
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
                    // Last chance to ship coverage the run wrote since the
                    // previous drain — a cancelled run gets no terminal block.
                    ship_coverage(
                        &mut sink,
                        &run_id,
                        &runs_root.join(&run_id),
                        &mut state.offsets.coverage,
                        mirror_coverage,
                    )
                    .await;
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
            Frame::Approve { run_id, mode } => {
                info!(run_id = %run_id, "node: Approve received");
                if let Some(state) = active.get_mut(&run_id) {
                    let argv = build_control_argv(ControlKind::Approve, &run_id, &mode, None);
                    match spawn_control(exe, &argv) {
                        Ok(child) => {
                            state.child = child;
                        }
                        Err(e) => warn!(run_id = %run_id, error = %e, "node: approve spawn failed"),
                    }
                } else {
                    warn!(run_id = %run_id, "node: Approve for unknown run_id (ignored)");
                }
            }
            Frame::Reject { run_id, reason } => {
                info!(run_id = %run_id, "node: Reject received");
                if let Some(state) = active.get_mut(&run_id) {
                    let argv =
                        build_control_argv(ControlKind::Reject, &run_id, "", reason.as_deref());
                    match spawn_control(exe, &argv) {
                        Ok(child) => {
                            state.child = child;
                        }
                        Err(e) => warn!(run_id = %run_id, error = %e, "node: reject spawn failed"),
                    }
                } else {
                    warn!(run_id = %run_id, "node: Reject for unknown run_id (ignored)");
                }
            }
            Frame::ArtifactPull { req, sha256 } => {
                info!(req = %req, sha256 = %sha256, "node: ArtifactPull received");
                pulls.push_back(ArtifactPullStream::new(global, req, sha256));
            }
            Frame::Hello { .. }
            | Frame::Welcome { .. }
            | Frame::Pong {}
            | Frame::Artifact { .. }
            | Frame::RunFinished { .. }
            | Frame::ArtifactChunk { .. }
            | Frame::ArtifactPullDone { .. } => {
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

/// Which control subprocess to launch in response to an Approve/Reject frame.
#[derive(Debug, Clone, Copy)]
enum ControlKind {
    Approve,
    Reject,
}

/// Build the argv (after the executable) for the local approve/reject command
/// the node runs against a gated run.
///   Approve: `workflow approve <run_id> [--mode <mode>]`
///   Reject:  `workflow reject  <run_id> [--reason <reason>]`
fn build_control_argv(
    kind: ControlKind,
    run_id: &str,
    mode: &str,
    reason: Option<&str>,
) -> Vec<String> {
    let mut argv = vec!["workflow".to_string()];
    match kind {
        ControlKind::Approve => {
            argv.push("approve".to_string());
            argv.push(run_id.to_string());
            if !mode.is_empty() {
                argv.push("--mode".to_string());
                argv.push(mode.to_string());
            }
        }
        ControlKind::Reject => {
            argv.push("reject".to_string());
            argv.push(run_id.to_string());
            if let Some(r) = reason {
                argv.push("--reason".to_string());
                argv.push(r.to_string());
            }
        }
    }
    argv
}

/// Spawn a detached `rupu workflow approve|reject` child, same launch posture
/// as `spawn_run` (null stdio, own process group on Unix).
fn spawn_control(exe: &Path, argv: &[String]) -> anyhow::Result<tokio::process::Child> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn().context("spawn rupu control child")
}

fn spawn_run(exe: &Path, run_id: &str, spec: &RunSpec) -> anyhow::Result<tokio::process::Child> {
    check_spec(spec)?;
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

/// Refuse a spec whose fields `build_argv` could not honour, rather than
/// launch without them. `findings_profile` is an agent-run flag; `rupu
/// workflow run` resolves profiles per step from the workflow file.
fn check_spec(spec: &RunSpec) -> anyhow::Result<()> {
    if spec.kind == RunSpecKind::Workflow && spec.findings_profile.is_some() {
        anyhow::bail!(
            "run spec for workflow `{}` carries a findings_profile, which applies to agent runs only",
            spec.name
        );
    }
    Ok(())
}

/// Build the argv (after the executable) for a local `rupu workflow run`
/// or `rupu run` invocation dispatched by the node agent.
///
/// Workflow: `workflow run <name> [<target>] --run-id <id> --plain [--input k=v]… [--mode m]`
/// Agent:    `run <name> [<target>] --run-id <id> [--mode m] [--findings-profile f] [--prompt p] [--tmp (if target)]`
///
/// Flag names are verified against the clap definitions in `cmd/workflow.rs`
/// (`--run-id`, `--plain`, `--input`, `--mode`) and `cmd/run.rs`
/// (`--run-id`, `--mode`, `--findings-profile`, `--prompt`, `--tmp`).
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
            if let Some(f) = spec.findings_profile {
                argv.push("--findings-profile".to_string());
                argv.push(f.as_str().to_string());
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

/// The complete lines of `bytes[offset..limit]` (the whole tail when `limit`
/// is `None`): `(lines, next_offset)`, `next_offset` being just past the last
/// newline in that range. A trailing partial line is left out.
fn complete_lines(bytes: &[u8], offset: u64, limit: Option<u64>) -> (Vec<String>, u64) {
    let end = limit.map_or(bytes.len() as u64, |l| l.min(bytes.len() as u64));
    if end <= offset {
        return (vec![], offset);
    }
    let new = &bytes[offset as usize..end as usize];
    // Only consume up to and including the last `\n` so partial lines
    // are held back until the writer flushes them.
    let last_nl = match new.iter().rposition(|&b| b == b'\n') {
        Some(idx) => idx,
        None => return (vec![], offset),
    };
    let complete = &new[..=last_nl];
    let lines = std::str::from_utf8(complete)
        .unwrap_or("")
        .lines()
        .map(|l| l.to_string())
        .collect();
    (lines, offset + complete.len() as u64)
}

/// The complete lines appended to `path` since byte `offset`, WITHOUT
/// consuming them: returns `(lines, next_offset)` where `next_offset` is just
/// past the last newline read. The caller commits `next_offset` once it has
/// done something durable with the lines — the bucket worker only after the
/// upload landed, so a failed put re-reads the same lines next tick.
///
/// Partial lines (no trailing `\n` yet) are left for a later call. If the file
/// does not exist or cannot be read, returns no lines and `offset` unchanged.
pub(crate) fn peek_new_lines(path: &Path, offset: u64) -> (Vec<String>, u64) {
    match std::fs::read(path) {
        Ok(bytes) => complete_lines(&bytes, offset, None),
        Err(_) => (vec![], offset),
    }
}

/// Like [`peek_new_lines`] but bounded: only the complete lines in
/// `[offset, end)`. A retried upload re-reads exactly the chunk it first
/// tried, however much the file has grown since.
pub(crate) fn peek_lines_until(path: &Path, offset: u64, end: u64) -> (Vec<String>, u64) {
    match std::fs::read(path) {
        Ok(bytes) => complete_lines(&bytes, offset, Some(end)),
        Err(_) => (vec![], offset),
    }
}

/// Drain any new complete lines appended to `path` since `*offset` bytes.
///
/// [`peek_new_lines`] plus the commit: advances `*offset` past the last
/// newline consumed and returns the completed lines as `String`s (without a
/// trailing newline). Right for the tunnel, which hands each line to the
/// socket as it reads it; the bucket worker uses [`upload_new_lines`], which
/// commits only after the upload lands.
///
/// Partial lines (no trailing `\n` yet) are left for the next call.
/// If the file does not exist or cannot be read, returns an empty `Vec`
/// and leaves `*offset` unchanged — the caller retries on the next tick.
pub fn drain_new_lines(path: &Path, offset: &mut u64) -> Vec<String> {
    let (lines, next) = peek_new_lines(path, *offset);
    *offset = next;
    lines
}

/// A run's usage ledger: `runs/<id>/usage.jsonl`.
fn usage_path(run_dir: &Path) -> std::path::PathBuf {
    run_dir.join("usage.jsonl")
}

/// Drain the run's usage ledger (`<run_dir>/usage.jsonl`) when `enabled`.
///
/// `enabled` is the tunnel capability gate (the CP advertised
/// [`CAP_USAGE_LEDGER`]); when it is `false` the ledger is left untouched and
/// `offsets.usage` does not move, so nothing is sent or consumed.
fn drain_usage_ledger(run_dir: &Path, offsets: &mut FileOffsets, enabled: bool) -> Vec<String> {
    if !enabled {
        return Vec::new();
    }
    drain_new_lines(&usage_path(run_dir), &mut offsets.usage)
}

/// True when `frame` is a `Welcome` that lists `capability`.
fn welcome_advertises(frame: &Frame, capability: &str) -> bool {
    matches!(frame, Frame::Welcome { capabilities } if capabilities.iter().any(|c| c == capability))
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
// Artifact pulls (unit-testable)
// ---------------------------------------------------------------------------

/// The answer to one `ArtifactPull`: the blob
/// `<global>/findings/artifacts/<aa>/<sha256>` as ordered `ArtifactChunk`
/// frames (`seq` from 0, each a whole [`ARTIFACT_CHUNK_BYTES`] decoded except
/// the last), then exactly one `ArtifactPullDone` — carrying the error when
/// the blob can't be found, opened or read — then `None`.
///
/// `connect_and_run` takes one frame per loop turn from its pull queue,
/// round-robin across pulls, so a large blob streams between inbound frames,
/// run-file drains and other pulls instead of holding them up. The blob is
/// opened on the first frame, so a pull that hasn't started holds no file
/// handle.
struct ArtifactPullStream {
    req: String,
    sha256: String,
    /// The blob's store path, or why there is none (a malformed sha).
    path: Result<PathBuf, String>,
    file: Option<tokio::fs::File>,
    seq: u64,
    done: bool,
}

impl ArtifactPullStream {
    fn new(global: &Path, req: String, sha256: String) -> Self {
        let path =
            crate::cmd::findings_helper::blob_path(global, &sha256).map_err(|e| e.to_string());
        Self {
            req,
            sha256,
            path,
            file: None,
            seq: 0,
            done: false,
        }
    }

    /// Whether the closing `ArtifactPullDone` has been yielded.
    fn is_done(&self) -> bool {
        self.done
    }

    /// The pull's next frame, or `None` once `ArtifactPullDone` has gone.
    async fn next_frame(&mut self) -> Option<Frame> {
        use base64::Engine as _;
        if self.done {
            return None;
        }
        let frame = match self.next_chunk().await {
            Ok(Some(data)) => {
                let seq = self.seq;
                self.seq += 1;
                return Some(Frame::ArtifactChunk {
                    req: self.req.clone(),
                    seq,
                    data_b64: base64::engine::general_purpose::STANDARD.encode(&data),
                });
            }
            Ok(None) => Frame::ArtifactPullDone {
                req: self.req.clone(),
                error: None,
            },
            Err(error) => {
                warn!(req = %self.req, sha256 = %self.sha256, %error, "node: artifact pull failed");
                Frame::ArtifactPullDone {
                    req: self.req.clone(),
                    error: Some(error),
                }
            }
        };
        self.done = true;
        self.file = None;
        Some(frame)
    }

    /// The next chunk's bytes — a whole [`ARTIFACT_CHUNK_BYTES`] unless the
    /// blob ends first — or `None` at the end of the blob.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        use tokio::io::AsyncReadExt as _;
        if self.file.is_none() {
            self.file = Some(self.open().await?);
        }
        let Some(file) = self.file.as_mut() else {
            return Err(format!("artifact {} was not opened", self.sha256));
        };
        let mut buf = vec![0u8; ARTIFACT_CHUNK_BYTES];
        let mut n = 0;
        // Fill a whole chunk: a read may return less.
        while n < buf.len() {
            let got = file
                .read(&mut buf[n..])
                .await
                .map_err(|e| format!("reading artifact {}: {e}", self.sha256))?;
            if got == 0 {
                break;
            }
            n += got;
        }
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }

    /// Open the blob without ever parking the caller: refuses anything but a
    /// regular file (a FIFO swapped into the store would otherwise block
    /// the open — and with it the node's frame loop — until a writer came).
    async fn open(&self) -> Result<tokio::fs::File, String> {
        let path = self.path.clone()?;
        let sha256 = self.sha256.clone();
        let opened =
            tokio::task::spawn_blocking(move || rupu_cp::api::fs_open::open_regular_file(&path))
                .await
                .map_err(|e| format!("opening artifact {sha256}: {e}"))?;
        let file = opened
            .map_err(|e| format!("artifact {} is not in this node's store: {e}", self.sha256))?;
        Ok(tokio::fs::File::from_std(file))
    }
}

// ---------------------------------------------------------------------------
// Bucket worker marker
// ---------------------------------------------------------------------------

/// The `nodes/<worker>.json` marker a bucket worker writes at startup.
fn bucket_worker_info(worker_id: &str) -> rupu_cp::host::bucket::WorkerInfo {
    rupu_cp::host::bucket::WorkerInfo {
        worker_id: worker_id.to_string(),
        rupu_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: rupu_cp::node::protocol::bucket_worker_capabilities(),
    }
}

// ---------------------------------------------------------------------------
// Read terminal status from run.json
// ---------------------------------------------------------------------------

/// Parse `run.json` and return `(status_str, raw_body)` if the run has
/// reached a terminal status; `None` if still in-flight, unreadable, or
/// not yet written.
fn read_terminal_status(run_json: &Path) -> Option<(String, String)> {
    let body = std::fs::read_to_string(run_json).ok()?;
    let status = terminal_status_in(&body)?;
    Some((status, body))
}

/// The status in a `run.json` body when it is terminal; `None` for an
/// in-flight, unparseable or status-less body.
fn terminal_status_in(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let status = v.get("status")?.as_str()?;
    matches!(status, "completed" | "failed" | "rejected" | "cancelled").then(|| status.to_string())
}

// ---------------------------------------------------------------------------
// Bucket pull agent helpers (unit-testable)
// ---------------------------------------------------------------------------

/// A run's coverage stream: `runs/<id>/coverage.jsonl`.
fn coverage_path(run_dir: &Path) -> std::path::PathBuf {
    run_dir.join(rupu_coverage::STREAM_FILE)
}

/// New lines of a run's coverage stream (`runs/<id>/coverage.jsonl`).
fn drain_coverage(run_dir: &Path, offset: &mut u64) -> Vec<String> {
    drain_new_lines(&coverage_path(run_dir), offset)
}

/// Send the run's new coverage lines to the CP as `ArtifactFile::Coverage`
/// frames — the one place the tunnel does it (the routine drain, the terminal
/// drain before `run.json` / `RunFinished`, and the cancel drain all call
/// this).
///
/// `mirror_coverage` is the capability gate (the CP's `Welcome` advertised
/// `mirror.coverage`); when it is `false` an older CP could not parse the
/// frame, so nothing is sent AND `*offset` does not move — the stream is not
/// consumed. Offsets make repeat calls exact-once: a later call sends only
/// what was appended since, and a trailing partial line is held back.
async fn ship_coverage<S>(
    sink: &mut S,
    run_id: &str,
    run_dir: &Path,
    offset: &mut u64,
    mirror_coverage: bool,
) where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    if !mirror_coverage {
        return;
    }
    for line in drain_coverage(run_dir, offset) {
        send_artifact(sink, run_id, ArtifactFile::Coverage, line).await;
    }
}

/// Build the result object key for a drained JSONL file chunk.
///
/// Format: `"<kind>.<seq:04>.jsonl"` — e.g. `"events.0001.jsonl"`.
/// The four-digit zero-padding keeps keys lexicographically ordered up to
/// 9 999 chunks per kind per run.
pub(crate) fn result_key(kind: &str, seq: u64) -> String {
    format!("{kind}.{seq:04}.jsonl")
}

/// Upload `lines` as one `<kind>.<seq>.jsonl` result object, advancing `seq`
/// only when the upload landed. Returns whether it did (`true` for no lines:
/// nothing was left unsent).
async fn put_lines(
    bucket: &dyn Bucket,
    rid: &str,
    kind: &str,
    lines: Vec<String>,
    seq: &mut u64,
) -> bool {
    if lines.is_empty() {
        return true;
    }
    let body = lines.join("\n") + "\n";
    let key = result_key(kind, *seq);
    match bucket.put_result(rid, &key, body.as_bytes()).await {
        Ok(()) => {
            *seq += 1;
            true
        }
        Err(e) => {
            warn!(run_id = %rid, key = %key, error = %e, "node pull: put {kind} result failed");
            false
        }
    }
}

/// Upload the new complete lines of the run file at `path` as the next
/// `<kind>.<seq>.jsonl` result object — the one primitive every bucket result
/// stream (events, step_results, unit_checkpoints, usage, coverage) goes
/// through. Returns `true` when nothing is left unsent.
///
/// The lines are PEEKED, not drained: `stream.offset` and `stream.seq` are
/// committed only once `put_result` returned `Ok`. A failed put leaves both
/// untouched and pins the chunk's end in `stream.pending_end`; the next call
/// re-uploads exactly that chunk under the same key (byte-exact, so a put that
/// errored after landing and was already consumed by the CP poller is
/// harmlessly overwritten with identical bytes), and only then the lines
/// written since go under the next key. A transient bucket error delays lines
/// instead of dropping them.
async fn upload_new_lines(
    bucket: &dyn Bucket,
    rid: &str,
    kind: &str,
    path: &Path,
    stream: &mut BucketStream,
) -> bool {
    if let Some(end) = stream.pending_end {
        let (lines, next) = peek_lines_until(path, stream.offset, end);
        if lines.is_empty() {
            // The pinned chunk held lines when it was first tried; an empty
            // read now (a transient read error, a truncated file) is NOT "all
            // sent". Clearing the pin here would let the next chunk grow into
            // a superset under the same key, which the CP poller may already
            // have consumed. Keep the pin until exactly that chunk lands.
            warn!(run_id = %rid, "node pull: a pinned {kind} chunk could not be re-read; holding it");
            return false;
        }
        if !put_lines(bucket, rid, kind, lines, &mut stream.seq).await {
            return false;
        }
        stream.offset = next;
        stream.pending_end = None;
    }
    let (lines, next) = peek_new_lines(path, stream.offset);
    // `put_lines` is `true` for no lines. (A chunk that was not UTF-8 reads as
    // no lines but still advances `next` — skipped, as `drain_new_lines` does.)
    if put_lines(bucket, rid, kind, lines, &mut stream.seq).await {
        stream.offset = next;
        true
    } else {
        stream.pending_end = Some(next);
        false
    }
}

/// Return the next control sequence number to assign given an existing list.
///
/// Returns `max(seq) + 1` if `existing` is non-empty, or `0` if empty.
/// Useful for the connector side when writing new control envelopes and
/// for verifying the last-applied watermark in tests.
// Called only in unit tests; suppress the dead_code lint for non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn next_control_seq(existing: &[(u64, Vec<u8>)]) -> u64 {
    existing.iter().map(|(s, _)| *s + 1).max().unwrap_or(0)
}

/// Upload the run's new usage-ledger rows as the next `usage.<seq>` result
/// object, committing the cursor once the put lands. Nothing new → no object.
/// No capability gate: an older CP's poller skips the unknown `usage.*` key.
async fn upload_usage_ledger(
    bucket: &dyn Bucket,
    rid: &str,
    run_dir: &Path,
    usage: &mut BucketStream,
) -> bool {
    upload_new_lines(bucket, rid, "usage", &usage_path(run_dir), usage).await
}

/// sha256s of the `stored: copied` artifacts that findings in a run's
/// coverage stream reference. A torn trailing line, an unparseable line, or
/// an entry whose hash isn't a store key is skipped, never an error.
///
/// The file is read as bytes and decoded lossily: a run that was killed
/// mid-write can leave its last line cut inside a multi-byte character, and a
/// strict UTF-8 read would fail the WHOLE stream — losing every reference on
/// the valid lines before it. The torn line just fails to parse and is skipped.
fn referenced_artifacts(stream: &Path) -> std::collections::BTreeSet<String> {
    let Ok(bytes) = std::fs::read(stream) else {
        return Default::default();
    };
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .filter_map(|l| serde_json::from_str::<rupu_coverage::StreamLine>(l).ok())
        .filter_map(|l| match l {
            rupu_coverage::StreamLine::Findings { record, .. } => record.report,
            _ => None,
        })
        .flat_map(|r| r.artifacts)
        .filter(|a| {
            a.stored == Some(rupu_coverage::report::ArtifactStorage::Copied)
                && rupu_coverage::report::is_sha256_hex(&a.sha256)
        })
        .map(|a| a.sha256)
        .collect()
}

/// Upload every blob this run's findings reference to `artifacts/<sha256>`,
/// so the coordinator can pull them later without this worker online (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §B1). A bucket worker
/// may never be polled again once its run finishes, so this is the only
/// chance — which makes the blobs part of what [`finish_bucket_run`] holds
/// the terminal run for. Returns `true` when nothing is left to upload.
///
/// A bucket error (the existence check or the upload) returns `false`: the
/// caller keeps the run active and retries next tick, and a blob that landed
/// meanwhile is skipped by its existence check. A blob this worker's store
/// does not hold as a regular file can never land, so it is logged and
/// skipped rather than holding the run forever — the coordinator reports it
/// unavailable.
async fn upload_referenced_artifacts(bucket: &dyn Bucket, global: &Path, run_dir: &Path) -> bool {
    let mut landed = true;
    for sha in referenced_artifacts(&run_dir.join(rupu_coverage::STREAM_FILE)) {
        match bucket.artifact_exists(&sha).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                warn!(sha = %sha, error = %e, "node pull: artifact existence check failed; holding the run to retry");
                landed = false;
                continue;
            }
        }
        let Ok(src) = crate::cmd::findings_helper::blob_path(global, &sha) else {
            continue;
        };
        // Not followed: the store only ever holds regular files, and a FIFO
        // or a dangling link would fail every retry.
        let in_store = std::fs::symlink_metadata(&src).is_ok_and(|m| m.file_type().is_file());
        if !in_store {
            warn!(
                sha = %sha, path = %src.display(),
                "node pull: a referenced artifact is not in this worker's store; the coordinator will report it unavailable"
            );
            continue;
        }
        if let Err(e) = bucket.put_artifact_file(&sha, &src).await {
            warn!(sha = %sha, error = %e, "node pull: artifact upload failed; holding the run to retry");
            landed = false;
        }
    }
    landed
}

/// The bucket loop's terminal pass for a run whose `run.json` reads terminal:
/// upload whatever every stream still holds, then the artifact blobs the
/// run's findings reference, then the terminal `run.json`, and only then write
/// the `finished` marker. Returns `true` when the marker landed. Every terminal
/// outcome (completed / failed / rejected / cancelled) comes through here.
///
/// A row the runner appended after the routine drain would otherwise never be
/// uploaded — the run leaves `active` once this returns `true` — and the CP
/// would finish the run short. The CP finishes a run on the terminal
/// `run.json` body and stops polling it from then on, so that body and the
/// marker are the CP's "nothing more is coming" signals: each is written ONLY
/// after everything before it landed — so a CP that sees the run terminal can
/// already pull every artifact blob its findings reference (a bucket worker
/// may never be polled again). The blobs go after the final coverage drain,
/// so the references are read from the run's complete stream. If any stream
/// or blob upload fails, nothing after it is attempted (no `run.json`, no
/// marker); if `run.json` or the marker fails, the marker is not written /
/// reported. Either way this returns `false` and the caller keeps the run
/// active, retrying next tick (a failed chunk retries byte-exact under its own
/// key; a blob that already landed is not re-uploaded). Every stream is still
/// attempted when one fails.
async fn finish_bucket_run(
    bucket: &dyn Bucket,
    rid: &str,
    run_dir: &Path,
    global: &Path,
    streams: &mut BucketStreams,
    run_json: &[u8],
    status: &str,
) -> bool {
    if !streams.upload_all(bucket, rid, run_dir).await {
        return false;
    }
    if !upload_referenced_artifacts(bucket, global, run_dir).await {
        return false;
    }
    // The routine pass withholds a terminal run.json (see `upload_routine`), so
    // this is the only place the CP can learn the terminal body.
    if let Err(e) = bucket.put_result(rid, "run.json", run_json).await {
        warn!(run_id = %rid, error = %e, "node pull: put terminal run.json failed");
        return false;
    }
    match bucket.put_finished(rid, status).await {
        Ok(()) => true,
        Err(e) => {
            warn!(run_id = %rid, status = %status, error = %e, "node pull: put_finished failed");
            false
        }
    }
}

/// The routine (non-terminal) uploads for a run: every stream, then the
/// in-progress `run.json`.
///
/// A `run.json` that already reads TERMINAL is deliberately NOT uploaded here.
/// The CP mirror finishes the run on that body and its poller only polls
/// non-terminal runs, so publishing it while a stream's chunk is still held
/// would make the CP stop reading the run — stranding that chunk and the
/// `finished` marker for good, even after the node heals. [`finish_bucket_run`]
/// uploads the terminal body, once every stream has landed.
async fn upload_routine(
    bucket: &dyn Bucket,
    rid: &str,
    run_dir: &Path,
    streams: &mut BucketStreams,
) {
    streams.upload_all(bucket, rid, run_dir).await;
    let Ok(body) = std::fs::read(run_dir.join("run.json")) else {
        return;
    };
    let terminal = std::str::from_utf8(&body)
        .ok()
        .and_then(terminal_status_in)
        .is_some();
    if terminal {
        return;
    }
    if let Err(e) = bucket.put_result(rid, "run.json", &body).await {
        warn!(run_id = %rid, error = %e, "node pull: put run.json failed");
    }
}

/// When the run's `run.json` reads terminal, run the terminal pass
/// ([`finish_bucket_run`]): `Some((status, marker_landed))`. `None` while the
/// run is still in flight.
async fn finish_if_terminal(
    bucket: &dyn Bucket,
    rid: &str,
    run_dir: &Path,
    global: &Path,
    streams: &mut BucketStreams,
) -> Option<(String, bool)> {
    let (status, body) = read_terminal_status(&run_dir.join("run.json"))?;
    let landed = finish_bucket_run(
        bucket,
        rid,
        run_dir,
        global,
        streams,
        body.as_bytes(),
        &status,
    )
    .await;
    Some((status, landed))
}

/// Write the `failed` marker for every claimed job whose spawn failed and
/// whose marker has not landed yet; the ones that still fail stay in `pending`
/// for the next tick. A job nobody marked would stay claimed and unfinished
/// forever.
async fn flush_failed_markers(bucket: &dyn Bucket, pending: &mut Vec<String>) {
    let mut still = Vec::new();
    for rid in pending.drain(..) {
        if let Err(e) = bucket.put_finished(&rid, "failed").await {
            warn!(run_id = %rid, error = %e, "node pull: put_finished(failed) after a spawn failure did not land; will retry");
            still.push(rid);
        }
    }
    *pending = still;
}

// ---------------------------------------------------------------------------
// Bucket pull agent loop
// ---------------------------------------------------------------------------

/// Claimed jobs the worker still owes work on after a tick.
#[derive(Debug, Default)]
struct PendingJobs {
    /// Claimed, but the job spec could not be fetched (a bucket error): the
    /// claim is ours and `list_jobs` no longer shows the job, so the fetch is
    /// retried every tick.
    unfetched: Vec<String>,
    /// Claimed jobs that cannot run (spawn failed, spec unusable) whose
    /// `failed` marker has not landed yet ([`flush_failed_markers`]).
    failed_unmarked: Vec<String>,
}

impl PendingJobs {
    fn is_empty(&self) -> bool {
        self.unfetched.is_empty() && self.failed_unmarked.is_empty()
    }
}

/// What a tick's claim step did.
#[derive(Debug, Default, PartialEq, Eq)]
struct ClaimOutcome {
    /// Jobs this tick won the claim for.
    won: usize,
    /// The first bucket error the claim step hit (`list_jobs` / `claim_job`),
    /// if any. It costs the tick's claims, never the worker.
    error: Option<String>,
}

/// `--once` has nothing left in flight and is about to exit successfully. That
/// is only honest if the claim step worked: a worker that claimed nothing
/// because the claim step itself failed (`claim_error` from the tick just run)
/// did nothing, and reporting "all runs terminal" with exit 0 would hide an
/// unreachable or broken bucket. A worker that did claim work (`claimed_any`)
/// is not failed by a later claim blip: it ran what it claimed.
fn once_exit_check(claimed_any: bool, claim_error: Option<&str>) -> anyhow::Result<()> {
    match claim_error {
        Some(e) if !claimed_any => {
            anyhow::bail!("--once: nothing was claimed because the claim step failed: {e}")
        }
        _ => Ok(()),
    }
}

/// One tick of the bucket worker: claim new jobs, then drain every active run.
///
/// Claiming is best-effort — a bucket error there only costs this tick's
/// claims — so the drain of runs already in flight (and the retry of held
/// chunks and unlanded markers) happens whatever the bucket did to the claim
/// step: an outage is exactly when held uploads need retrying, and a worker
/// that exits loses them.
async fn bucket_tick(
    bucket: &dyn Bucket,
    exe: &Path,
    host_id: &str,
    runs_root: &Path,
    global: &Path,
    active: &mut HashMap<String, BucketRunState>,
    pending: &mut PendingJobs,
) -> ClaimOutcome {
    let outcome = claim_new_jobs(bucket, exe, host_id, active, pending).await;
    // First attempt right away; a marker that did not land is retried on
    // every later tick until it does.
    flush_failed_markers(bucket, &mut pending.failed_unmarked).await;
    drain_active_runs(bucket, exe, runs_root, global, active).await;
    outcome
}

/// Step 1: claim every job that is up for grabs and start it.
///
/// No bucket error here ends the worker: a failing `list_jobs` or
/// `claim_job` skips that claim for this tick (the job is still listed next
/// tick if the claim did not land), and a claimed job whose spec cannot be
/// fetched is parked in `pending.unfetched` and retried every tick.
async fn claim_new_jobs(
    bucket: &dyn Bucket,
    exe: &Path,
    host_id: &str,
    active: &mut HashMap<String, BucketRunState>,
    pending: &mut PendingJobs,
) -> ClaimOutcome {
    let mut outcome = ClaimOutcome::default();
    // Specs that could not be fetched on an earlier tick come first.
    for run_id in std::mem::take(&mut pending.unfetched) {
        start_claimed_job(bucket, exe, run_id, active, pending).await;
    }
    let job_ids = match bucket.list_jobs().await {
        Ok(ids) => ids,
        Err(e) => {
            warn!(error = %e, "node pull: list_jobs failed; no claims this tick");
            outcome.error = Some(format!("list_jobs: {e}"));
            return outcome;
        }
    };
    for run_id in job_ids {
        match bucket.claim_job(&run_id, host_id).await {
            Ok(true) => outcome.won += 1,
            Ok(false) => {
                info!(run_id = %run_id, "node pull: job already claimed by another node");
                continue;
            }
            Err(e) => {
                warn!(run_id = %run_id, error = %e, "node pull: claim_job failed; skipping it this tick");
                outcome
                    .error
                    .get_or_insert_with(|| format!("claim_job {run_id}: {e}"));
                continue;
            }
        }
        info!(run_id = %run_id, "node pull: claimed job");
        start_claimed_job(bucket, exe, run_id, active, pending).await;
    }
    outcome
}

/// Fetch a claimed job's spec and spawn it.
///
/// - spec fetch fails (a bucket error): parked in `pending.unfetched`, retried
///   next tick — the claim is ours, so nobody else will run it.
/// - the job object is gone, or the spec does not deserialize: the job is
///   unusable and a retry cannot fix it, so it is marked `failed` (same as a
///   spawn failure) rather than ending the worker.
/// - spawn fails: marked `failed`.
async fn start_claimed_job(
    bucket: &dyn Bucket,
    exe: &Path,
    run_id: String,
    active: &mut HashMap<String, BucketRunState>,
    pending: &mut PendingJobs,
) {
    let job_bytes = match bucket.get_job(&run_id).await {
        Ok(b) => b,
        Err(BucketError::NotFound(_)) => {
            warn!(run_id = %run_id, "node pull: claimed job has no spec object; marking it failed");
            pending.failed_unmarked.push(run_id);
            return;
        }
        Err(e) => {
            warn!(run_id = %run_id, error = %e, "node pull: get_job failed; will retry next tick");
            pending.unfetched.push(run_id);
            return;
        }
    };
    let spec: RunSpec = match serde_json::from_slice(&job_bytes) {
        Ok(spec) => spec,
        Err(e) => {
            warn!(run_id = %run_id, error = %e, "node pull: job spec is malformed; marking it failed");
            pending.failed_unmarked.push(run_id);
            return;
        }
    };
    match spawn_run(exe, &run_id, &spec) {
        Ok(child) => {
            info!(run_id = %run_id, "node pull: run spawned");
            active.insert(
                run_id,
                BucketRunState {
                    child,
                    streams: BucketStreams::default(),
                    last_ctrl_seq: None,
                },
            );
        }
        Err(e) => {
            warn!(run_id = %run_id, error = %e, "node pull: spawn failed");
            pending.failed_unmarked.push(run_id);
        }
    }
}

/// Step 2: drain every active run — routine uploads, queued control messages,
/// and the terminal pass for a run whose `run.json` reads terminal. A run
/// leaves `active` only once its `finished` marker landed; otherwise it stays
/// and the next tick re-reads `run.json` and retries.
async fn drain_active_runs(
    bucket: &dyn Bucket,
    exe: &Path,
    runs_root: &Path,
    global: &Path,
    active: &mut HashMap<String, BucketRunState>,
) {
    let run_ids: Vec<String> = active.keys().cloned().collect();
    let mut finished: Vec<String> = Vec::new();

    for rid in &run_ids {
        let state = active.get_mut(rid).expect("rid came from active.keys()");
        let run_dir = runs_root.join(rid);

        // Each stream: peek the new lines, upload, and only then commit the
        // cursor — a failed put retries the same chunk under the same key next
        // tick instead of losing it. Plus the in-progress run.json.
        upload_routine(bucket, rid, &run_dir, &mut state.streams).await;

        // Drain queued control messages beyond the last-applied seq.
        match bucket.list_control(rid).await {
            Ok(controls) => {
                for (seq, bytes) in &controls {
                    // Skip already-applied controls.
                    if let Some(last) = state.last_ctrl_seq {
                        if *seq <= last {
                            continue;
                        }
                    }
                    match serde_json::from_slice::<ControlEnvelope>(bytes) {
                        Ok(envelope) => {
                            match envelope.kind.as_str() {
                                "cancel" => {
                                    info!(run_id = %rid, seq, "node pull: cancel");
                                    if let Err(e) = state.child.start_kill() {
                                        warn!(run_id = %rid, error = %e, "node pull: kill child failed");
                                    }
                                    // Cancel: advance unconditionally — the process
                                    // is gone (or already dead) regardless of kill() error.
                                    state.last_ctrl_seq = Some(*seq);
                                }
                                "approve" => {
                                    info!(run_id = %rid, seq, "node pull: approve");
                                    let argv = build_control_argv(
                                        ControlKind::Approve,
                                        rid,
                                        envelope.mode.as_deref().unwrap_or(""),
                                        None,
                                    );
                                    match spawn_control(exe, &argv) {
                                        Ok(child) => {
                                            state.child = child;
                                            // Advance ONLY on successful spawn so a
                                            // transient spawn error causes a retry
                                            // next tick instead of stranding the run.
                                            state.last_ctrl_seq = Some(*seq);
                                        }
                                        Err(e) => {
                                            warn!(run_id = %rid, error = %e, "node pull: approve spawn failed");
                                            // Leave last_ctrl_seq unchanged → retry.
                                        }
                                    }
                                }
                                "reject" => {
                                    info!(run_id = %rid, seq, "node pull: reject");
                                    let argv = build_control_argv(
                                        ControlKind::Reject,
                                        rid,
                                        "",
                                        envelope.reason.as_deref(),
                                    );
                                    match spawn_control(exe, &argv) {
                                        Ok(child) => {
                                            state.child = child;
                                            // Advance ONLY on successful spawn.
                                            state.last_ctrl_seq = Some(*seq);
                                        }
                                        Err(e) => {
                                            warn!(run_id = %rid, error = %e, "node pull: reject spawn failed");
                                            // Leave last_ctrl_seq unchanged → retry.
                                        }
                                    }
                                }
                                other => {
                                    warn!(run_id = %rid, seq, kind = other, "node pull: unknown control kind (ignored)");
                                    // Advance past unknown kinds so they are never
                                    // reprocessed (the kind won't become known on retry).
                                    state.last_ctrl_seq = Some(*seq);
                                }
                            }
                        }
                        Err(e) => {
                            warn!(run_id = %rid, seq, error = %e, "node pull: failed to deserialize ControlEnvelope (skipped)");
                            // Advance past corrupt envelopes so they are never
                            // reprocessed (the bytes won't change on retry).
                            state.last_ctrl_seq = Some(*seq);
                        }
                    }
                }
            }
            Err(e) => {
                warn!(run_id = %rid, error = %e, "node pull: list_control failed");
            }
        }

        // Check for terminal status → the terminal pass (final drain of
        // every stream, run.json, then the `finished` marker).
        match finish_if_terminal(bucket, rid, &run_dir, global, &mut state.streams).await {
            Some((status, true)) => {
                info!(run_id = %rid, status = %status, "node pull: run finished");
                finished.push(rid.clone());
            }
            Some((status, false)) => {
                warn!(
                    run_id = %rid,
                    status = %status,
                    "node pull: run is terminal but its final uploads or finished marker did not land; keeping it active to retry"
                );
            }
            None => {}
        }
    }

    for rid in &finished {
        active.remove(rid);
    }
}

/// Maximum polling iterations in `--once` mode before giving up on
/// active runs that have not reached a terminal status.
/// 200 × 250 ms = 50 s.
const ONCE_MAX_ITERS: u32 = 200;

async fn pull(args: PullArgs) -> anyhow::Result<()> {
    let bucket = ObjectStoreBucket::from_url(&args.bucket, args.prefix.as_deref())
        .context("build ObjectStoreBucket from url")?;
    let exe = std::env::current_exe().context("resolve current executable path")?;
    let global = crate::paths::global_dir()?;
    let runs_root = global.join("runs");
    let host_id = resolve_node_id(args.host_id).context("resolve host id")?;

    info!(
        host_id = %host_id,
        bucket = %args.bucket,
        once = args.once,
        interval = args.interval,
        "node pull: starting"
    );

    // Advertise what this worker honours: the bucket has no handshake, so the
    // CP reads these markers before putting a job that needs a newer worker
    // (e.g. one carrying `findings_profile`). A worker that can't write here
    // can't upload results either, so fail startup rather than run unseen.
    let info = bucket_worker_info(&host_id);
    bucket
        .put_worker_info(&host_id, &serde_json::to_vec(&info)?)
        .await
        .context("advertise worker capabilities (nodes/<worker>.json)")?;

    let mut active: HashMap<String, BucketRunState> = HashMap::new();
    let mut pending = PendingJobs::default();
    let mut once_iters: u32 = 0;
    // For `--once`: did this process claim anything, and did the claim step of
    // the tick just run fail?
    let mut claimed_any = false;

    loop {
        let claim = bucket_tick(
            &bucket,
            &exe,
            &host_id,
            &runs_root,
            &global,
            &mut active,
            &mut pending,
        )
        .await;
        claimed_any |= claim.won > 0;

        // ── Loop control ──────────────────────────────────────────────────────
        if args.once {
            if active.is_empty() && pending.is_empty() {
                // Nothing in flight: only a success if the claim step worked.
                once_exit_check(claimed_any, claim.error.as_deref())?;
                info!("node pull: --once: all runs terminal, exiting");
                break;
            }
            once_iters += 1;
            if once_iters >= ONCE_MAX_ITERS {
                warn!(
                    active = active.len(),
                    unfetched_jobs = pending.unfetched.len(),
                    unmarked_failures = pending.failed_unmarked.len(),
                    "node pull: --once: max-iterations reached, exiting with active runs \
                     (a terminal run whose final uploads or finished marker never landed is \
                     left active, with no marker), claimed jobs whose spec could not be \
                     fetched, or unmarked failures"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(args.interval)).await;
        }
    }

    Ok(())
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
        assert!(
            more.is_empty(),
            "second drain should be empty, got: {more:?}"
        );

        // Append 2 more lines → only those come back.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, r#"{{"type":"ev","n":3}}"#).unwrap();
        writeln!(f, r#"{{"type":"ev","n":4}}"#).unwrap();
        f.flush().unwrap();
        drop(f);

        let new_lines = drain_new_lines(&path, &mut offset);
        assert_eq!(
            new_lines.len(),
            2,
            "expected 2 new lines, got: {new_lines:?}"
        );
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
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f).unwrap();
        drop(f);

        let lines = drain_new_lines(&path, &mut offset);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("partial"));
    }

    // ------------------------------------------------------------------
    // usage ledger forwarding: capability gate + drain
    // ------------------------------------------------------------------

    fn offsets() -> FileOffsets {
        FileOffsets {
            events: 0,
            step_results: 0,
            unit_checkpoints: 0,
            usage: 0,
            coverage: 0,
        }
    }

    #[test]
    fn welcome_advertises_reads_the_capability_list() {
        let none = Frame::Welcome {
            capabilities: vec![],
        };
        let other = Frame::Welcome {
            capabilities: vec!["something_else".to_string()],
        };
        let usage = Frame::Welcome {
            capabilities: vec!["something_else".to_string(), CAP_USAGE_LEDGER.to_string()],
        };
        // An old CP's bare `{"type":"welcome"}` parses to an empty list.
        let old: Frame = serde_json::from_str(r#"{"type":"welcome"}"#).unwrap();
        assert!(!welcome_advertises(&none, CAP_USAGE_LEDGER));
        assert!(!welcome_advertises(&old, CAP_USAGE_LEDGER));
        assert!(!welcome_advertises(&other, CAP_USAGE_LEDGER));
        assert!(welcome_advertises(&usage, CAP_USAGE_LEDGER));
        // Not a Welcome at all.
        assert!(!welcome_advertises(&Frame::Pong {}, CAP_USAGE_LEDGER));
    }

    /// The usage ledger is drained incrementally into the frames' lines when
    /// the CP advertised the capability, and is left completely untouched
    /// (no lines, offset unmoved) when it did not.
    #[test]
    fn drain_usage_ledger_honours_the_capability_gate() {
        let dir = tempdir().unwrap();
        let ledger = dir.path().join("usage.jsonl");
        std::fs::write(
            &ledger,
            "{\"id\":\"01J0000000000000000000USG1\"}\n{\"id\":\"01J0000000000000000000USG2\"}\n",
        )
        .unwrap();

        // Gate closed (old CP): nothing sent, nothing consumed.
        let mut off = offsets();
        assert!(drain_usage_ledger(dir.path(), &mut off, false).is_empty());
        assert_eq!(off.usage, 0, "a gated ledger must not be consumed");

        // Gate open: both rows, then nothing until the ledger grows.
        let lines = drain_usage_ledger(dir.path(), &mut off, true);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("USG1") && lines[1].contains("USG2"));
        assert!(drain_usage_ledger(dir.path(), &mut off, true).is_empty());

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&ledger)
            .unwrap();
        writeln!(f, r#"{{"id":"01J0000000000000000000USG3"}}"#).unwrap();
        drop(f);
        let more = drain_usage_ledger(dir.path(), &mut off, true);
        assert_eq!(more.len(), 1);
        assert!(more[0].contains("USG3"));
        // The other artifacts' offsets are independent.
        assert_eq!(off.events, 0);
    }

    /// The terminal pass of the tunnel loop drains the ledger a SECOND time
    /// (a row can land between the routine drain and the terminal `run.json`
    /// read, and the run leaves `active` that pass). The loop itself is inline
    /// in `connect_and_run` and needs a live WebSocket, so what is testable —
    /// and load-bearing — is the helper sequence it runs: the second drain
    /// returns exactly the late row, never the ones the first already sent,
    /// and a gated (old-CP) connection still sends nothing on either pass.
    #[test]
    fn terminal_pass_second_drain_sends_only_the_late_row() {
        let dir = tempdir().unwrap();
        let ledger = dir.path().join("usage.jsonl");
        std::fs::write(&ledger, "{\"id\":\"USG1\"}\n{\"id\":\"USG2\"}\n").unwrap();

        let mut off = offsets();
        // Routine drain earlier in the pass.
        let first = drain_usage_ledger(dir.path(), &mut off, true);
        assert_eq!(first.len(), 2, "{first:?}");

        // The runner appends a final row, then writes the terminal run.json.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&ledger)
            .unwrap();
        writeln!(f, r#"{{"id":"USG3"}}"#).unwrap();
        drop(f);

        // Terminal pass: exactly the late row, once.
        let last = drain_usage_ledger(dir.path(), &mut off, true);
        assert_eq!(last.len(), 1, "{last:?}");
        assert!(last[0].contains("USG3"));
        assert!(drain_usage_ledger(dir.path(), &mut off, true).is_empty());

        // Same ledger, old CP: neither pass sends anything or moves the offset.
        let mut gated = offsets();
        assert!(drain_usage_ledger(dir.path(), &mut gated, false).is_empty());
        assert!(drain_usage_ledger(dir.path(), &mut gated, false).is_empty());
        assert_eq!(gated.usage, 0);
    }

    /// A [`Bucket`] that records the writes the terminal pass makes, in order,
    /// and can be told to fail some of them:
    /// - `ops`: the writes that LANDED — `put_result` as `(key, body)`,
    ///   `put_finished` as `("finished", status)`.
    /// - `attempts`: every `put_result` tried, failed ones included. A failed
    ///   put is recorded as an attempt because a real one can have landed
    ///   before it errored (a timeout).
    /// - `fail_prefix`: a `put_result` whose key starts with it fails, and
    ///   `"finished"` fails the marker.
    #[derive(Default)]
    struct RecordingBucket {
        ops: std::sync::Mutex<Vec<(String, String)>>,
        attempts: std::sync::Mutex<Vec<(String, String)>>,
        fail_prefix: std::sync::Mutex<Option<String>>,
        /// `artifacts/<sha256>` objects, in memory.
        artifacts: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    }

    impl RecordingBucket {
        fn failing(prefix: &str) -> Self {
            let b = Self::default();
            b.set_failing(Some(prefix));
            b
        }

        fn set_failing(&self, prefix: Option<&str>) {
            *self.fail_prefix.lock().unwrap() = prefix.map(str::to_string);
        }

        fn fails(&self, key: &str) -> bool {
            self.fail_prefix
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(|p| key.starts_with(p))
        }

        fn landed(&self) -> Vec<(String, String)> {
            self.ops.lock().unwrap().clone()
        }

        fn landed_keys(&self) -> Vec<String> {
            self.landed().into_iter().map(|(k, _)| k).collect()
        }

        /// The body that landed under `key`.
        fn landed_body(&self, key: &str) -> Option<String> {
            self.landed()
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, b)| b)
        }
    }

    #[async_trait::async_trait]
    impl Bucket for RecordingBucket {
        async fn put_job(&self, _: &str, _: &[u8]) -> Result<(), BucketError> {
            unreachable!("the terminal pass never writes jobs")
        }
        async fn list_jobs(&self) -> Result<Vec<String>, BucketError> {
            unreachable!()
        }
        async fn claim_job(&self, _: &str, _: &str) -> Result<bool, BucketError> {
            unreachable!()
        }
        async fn get_job(&self, _: &str) -> Result<Vec<u8>, BucketError> {
            unreachable!()
        }
        async fn put_control(&self, _: &str, _: u64, _: &[u8]) -> Result<(), BucketError> {
            unreachable!()
        }
        async fn list_control(&self, _: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError> {
            unreachable!()
        }
        async fn put_result(
            &self,
            _run_id: &str,
            key: &str,
            body: &[u8],
        ) -> Result<(), BucketError> {
            let entry = (key.to_string(), String::from_utf8_lossy(body).into_owned());
            self.attempts.lock().unwrap().push(entry.clone());
            if self.fails(key) {
                return Err(BucketError::Io(format!("injected failure for {key}")));
            }
            self.ops.lock().unwrap().push(entry);
            Ok(())
        }
        async fn list_results(&self, _: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError> {
            unreachable!()
        }
        async fn put_finished(&self, _run_id: &str, status: &str) -> Result<(), BucketError> {
            if self.fails("finished") {
                return Err(BucketError::Io("injected failure for finished".into()));
            }
            self.ops
                .lock()
                .unwrap()
                .push(("finished".to_string(), status.to_string()));
            Ok(())
        }
        async fn get_finished(&self, _: &str) -> Result<Option<String>, BucketError> {
            unreachable!()
        }
        async fn put_worker_info(&self, _: &str, _: &[u8]) -> Result<(), BucketError> {
            unreachable!("the terminal pass never writes worker info")
        }
        async fn list_worker_info(&self) -> Result<Vec<Vec<u8>>, BucketError> {
            unreachable!()
        }
        async fn probe(&self) -> Result<(), BucketError> {
            unreachable!()
        }
        async fn artifact_exists(&self, sha256: &str) -> Result<bool, BucketError> {
            Ok(self.artifacts.lock().unwrap().contains_key(sha256))
        }
        /// Stored under `artifacts/<sha256>`; logged as an `artifact:<sha256>`
        /// op (body = the blob) so a test sees where it falls relative to the
        /// `finished` marker.
        async fn put_artifact_file(
            &self,
            sha256: &str,
            src: &std::path::Path,
        ) -> Result<(), BucketError> {
            let key = format!("artifact:{sha256}");
            let body = std::fs::read(src).map_err(|e| BucketError::Io(e.to_string()))?;
            let entry = (key.clone(), String::from_utf8_lossy(&body).into_owned());
            self.attempts.lock().unwrap().push(entry.clone());
            if self.fails(&key) {
                return Err(BucketError::Io(format!("injected failure for {key}")));
            }
            self.ops.lock().unwrap().push(entry);
            self.artifacts
                .lock()
                .unwrap()
                .insert(sha256.to_string(), body);
            Ok(())
        }
        async fn get_artifact_to_file(
            &self,
            sha256: &str,
            dest: &std::path::Path,
            max_bytes: u64,
        ) -> Result<(), BucketError> {
            let body = self
                .artifacts
                .lock()
                .unwrap()
                .get(sha256)
                .cloned()
                .ok_or_else(|| BucketError::NotFound(format!("artifacts/{sha256}")))?;
            if body.len() as u64 > max_bytes {
                return Err(BucketError::Io(format!(
                    "artifact {sha256} exceeds its recorded {max_bytes} bytes"
                )));
            }
            std::fs::write(dest, body).map_err(|e| BucketError::Io(e.to_string()))
        }
    }

    const RUN_JSON: &[u8] = b"{\"status\":\"completed\"}";

    /// The bucket loop's terminal pass (same race as the tunnel's, above): a
    /// row appended after the routine drain is uploaded as the next `usage.*`
    /// object, exactly once and BEFORE the terminal `run.json` and the
    /// `finished` marker; with no late row the pass writes no empty usage
    /// object.
    #[tokio::test]
    async fn bucket_terminal_pass_uploads_the_late_row_before_finished() {
        let dir = tempdir().unwrap();
        let ledger = usage_path(dir.path());
        std::fs::write(&ledger, "{\"id\":\"USGA\"}\n{\"id\":\"USGB\"}\n").unwrap();
        let bucket = RecordingBucket::default();
        let mut streams = BucketStreams::default();

        // Routine drain earlier in the pass.
        assert!(upload_usage_ledger(&bucket, "run_BKT", dir.path(), &mut streams.usage).await);
        // The runner appends a final row, then writes the terminal run.json.
        append(&ledger, "{\"id\":\"USGC\"}\n");
        assert!(
            finish_bucket_run(
                &bucket,
                "run_BKT",
                dir.path(),
                dir.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );

        let ops = bucket.landed();
        let keys: Vec<&str> = ops.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "usage.0000.jsonl",
                "usage.0001.jsonl",
                "run.json",
                "finished"
            ],
            "{ops:?}"
        );
        assert!(ops[0].1.contains("USGA") && ops[0].1.contains("USGB"));
        assert!(!ops[0].1.contains("USGC"));
        assert_eq!(ops[1].1, "{\"id\":\"USGC\"}\n", "only the late row");
        assert_eq!(ops[2].1, String::from_utf8_lossy(RUN_JSON));
        assert_eq!(ops[3].1, "completed");
        assert_eq!(streams.usage.seq, 2);

        // Nothing late: the terminal pass is just run.json and the marker.
        let quiet = RecordingBucket::default();
        let mut streams = BucketStreams::default();
        upload_usage_ledger(&quiet, "run_QUIET", dir.path(), &mut streams.usage).await;
        assert!(
            finish_bucket_run(
                &quiet,
                "run_QUIET",
                dir.path(),
                dir.path(),
                &mut streams,
                RUN_JSON,
                "failed",
            )
            .await
        );
        assert_eq!(
            quiet.landed_keys(),
            ["usage.0000.jsonl", "run.json", "finished"]
        );
        assert_eq!(streams.usage.seq, 1);
    }

    /// A stream whose last upload fails must NOT get the `finished` marker (the
    /// CP stops reading a finished run, so the lines would be lost for good):
    /// the pass reports `false` — the loop then keeps the run active — every
    /// other stream still lands, and once the bucket recovers the next pass
    /// retries the failed chunk byte for byte under its own key, then writes
    /// the marker AFTER every object.
    #[tokio::test]
    async fn bucket_terminal_pass_withholds_the_marker_until_every_stream_landed() {
        let dir = tempdir().unwrap();
        append(&dir.path().join("events.jsonl"), "e1\ne2\n");
        append(&coverage_path(dir.path()), "c1\n");
        let bucket = RecordingBucket::failing("events");
        let mut streams = BucketStreams::default();

        let done = finish_bucket_run(
            &bucket,
            "run_T",
            dir.path(),
            dir.path(),
            &mut streams,
            RUN_JSON,
            "completed",
        )
        .await;
        assert!(!done, "an unlanded stream means the run is not finished");
        let keys = bucket.landed_keys();
        assert!(!keys.contains(&"finished".to_string()), "{keys:?}");
        assert!(
            !keys.contains(&"run.json".to_string()),
            "the terminal run.json is not published while a stream is held: {keys:?}"
        );
        assert!(
            keys.contains(&"coverage.0000.jsonl".to_string()),
            "the other streams are still attempted: {keys:?}"
        );

        // The run wrote one more event while the bucket was down; it recovers.
        append(&dir.path().join("events.jsonl"), "e3\n");
        bucket.set_failing(None);
        let done = finish_bucket_run(
            &bucket,
            "run_T",
            dir.path(),
            dir.path(),
            &mut streams,
            RUN_JSON,
            "completed",
        )
        .await;
        assert!(done);

        // The retried chunk is exactly what the failed put carried — not grown
        // by `e3` — and the later line is its own object.
        assert_eq!(
            bucket.landed_body("events.0000.jsonl").as_deref(),
            Some("e1\ne2\n")
        );
        assert_eq!(
            bucket.landed_body("events.0001.jsonl").as_deref(),
            Some("e3\n")
        );
        // Coverage landed once, on the first pass, and is not re-sent.
        assert_eq!(
            bucket
                .landed_keys()
                .iter()
                .filter(|k| k.starts_with("coverage"))
                .count(),
            1
        );
        // Everything landed before the marker; the marker is last.
        let keys = bucket.landed_keys();
        assert_eq!(
            keys.last().map(String::as_str),
            Some("finished"),
            "{keys:?}"
        );
        assert_eq!(keys.iter().filter(|k| *k == "finished").count(), 1);
    }

    /// The marker is the last write and its own failure also holds the run:
    /// the streams are already uploaded, so the retry is just the marker —
    /// nothing is uploaded twice.
    #[tokio::test]
    async fn bucket_terminal_pass_retries_a_failed_marker_without_reuploading() {
        let dir = tempdir().unwrap();
        append(&coverage_path(dir.path()), "c1\n");
        let bucket = RecordingBucket::failing("finished");
        let mut streams = BucketStreams::default();

        assert!(
            !finish_bucket_run(
                &bucket,
                "run_M",
                dir.path(),
                dir.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );
        assert_eq!(bucket.landed_keys(), ["coverage.0000.jsonl", "run.json"]);

        bucket.set_failing(None);
        assert!(
            finish_bucket_run(
                &bucket,
                "run_M",
                dir.path(),
                dir.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );
        assert_eq!(
            bucket.landed_keys(),
            // run.json is idempotent (same key, overwritten); the coverage
            // object is not re-uploaded.
            ["coverage.0000.jsonl", "run.json", "run.json", "finished"]
        );
    }

    /// A failed terminal `run.json` upload holds the marker too: the CP
    /// finishes the run from that body.
    #[tokio::test]
    async fn bucket_terminal_pass_withholds_the_marker_when_run_json_did_not_land() {
        let dir = tempdir().unwrap();
        let bucket = RecordingBucket::failing("run.json");
        let mut streams = BucketStreams::default();
        assert!(
            !finish_bucket_run(
                &bucket,
                "run_J",
                dir.path(),
                dir.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );
        assert!(bucket.landed().is_empty());
    }

    /// The same pass against a REAL failing store: it reports `false` and
    /// leaves no marker; after the store heals the next pass lands the lines
    /// under their original keys and then the marker.
    #[tokio::test]
    async fn bucket_terminal_pass_over_a_failing_store_finishes_after_it_heals() {
        let run_dir = tempdir().unwrap();
        append(&run_dir.path().join("events.jsonl"), "e1\ne2\n");
        append(&coverage_path(run_dir.path()), "begin\nfinding\n");
        let fb = FailingBucket::new();
        let mut streams = BucketStreams::default();

        assert!(
            !finish_bucket_run(
                &fb.bucket,
                "run_1",
                run_dir.path(),
                run_dir.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );
        fb.heal();
        assert_eq!(
            fb.bucket.get_finished("run_1").await.unwrap(),
            None,
            "no marker while the uploads had not landed"
        );

        assert!(
            finish_bucket_run(
                &fb.bucket,
                "run_1",
                run_dir.path(),
                run_dir.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );
        let results = fb.bucket.list_results("run_1").await.unwrap();
        let got: Vec<(&str, &str)> = results
            .iter()
            .map(|(k, v)| (k.as_str(), std::str::from_utf8(v).unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("coverage.0000.jsonl", "begin\nfinding\n"),
                ("events.0000.jsonl", "e1\ne2\n"),
                ("run.json", "{\"status\":\"completed\"}"),
            ]
        );
        assert_eq!(
            fb.bucket.get_finished("run_1").await.unwrap().as_deref(),
            Some("completed")
        );
    }

    /// A terminal `run.json` is what makes the CP mirror finish the run, and its
    /// poller stops polling a finished run. So while a stream's chunk is held
    /// (its put keeps failing) NEITHER the routine pass NOR the terminal pass may
    /// publish that body — over many ticks, with `run.json` puts perfectly
    /// healthy. After the bucket heals the order is: streams, run.json, marker.
    #[tokio::test]
    async fn a_terminal_run_json_is_not_published_while_a_stream_is_held() {
        let run_dir = tempdir().unwrap();
        append(&run_dir.path().join("events.jsonl"), "e1\n");
        std::fs::write(run_dir.path().join("run.json"), RUN_JSON).unwrap();
        let bucket = RecordingBucket::failing("events");
        let mut streams = BucketStreams::default();

        for _tick in 0..3 {
            upload_routine(&bucket, "run_H", run_dir.path(), &mut streams).await;
            let outcome = finish_if_terminal(
                &bucket,
                "run_H",
                run_dir.path(),
                run_dir.path(),
                &mut streams,
            )
            .await;
            assert_eq!(outcome, Some(("completed".to_string(), false)));
            assert!(
                bucket.landed().is_empty(),
                "nothing, in particular no terminal run.json, may land: {:?}",
                bucket.landed()
            );
        }

        bucket.set_failing(None);
        upload_routine(&bucket, "run_H", run_dir.path(), &mut streams).await;
        assert_eq!(bucket.landed_keys(), ["events.0000.jsonl"], "streams first");
        let outcome = finish_if_terminal(
            &bucket,
            "run_H",
            run_dir.path(),
            run_dir.path(),
            &mut streams,
        )
        .await;
        assert_eq!(outcome, Some(("completed".to_string(), true)));
        assert_eq!(
            bucket.landed_keys(),
            ["events.0000.jsonl", "run.json", "finished"]
        );
        assert_eq!(
            bucket.landed_body("events.0000.jsonl").as_deref(),
            Some("e1\n")
        );
    }

    /// The routine pass still publishes an in-progress `run.json` every tick
    /// (that is how the CP sees `awaiting_approval` and friends), and
    /// `finish_if_terminal` is a no-op for it.
    #[tokio::test]
    async fn an_in_progress_run_json_is_published_by_the_routine_pass() {
        let run_dir = tempdir().unwrap();
        std::fs::write(run_dir.path().join("run.json"), b"{\"status\":\"running\"}").unwrap();
        let bucket = RecordingBucket::default();
        let mut streams = BucketStreams::default();

        upload_routine(&bucket, "run_P", run_dir.path(), &mut streams).await;
        assert_eq!(bucket.landed_keys(), ["run.json"]);
        assert_eq!(
            finish_if_terminal(
                &bucket,
                "run_P",
                run_dir.path(),
                run_dir.path(),
                &mut streams
            )
            .await,
            None
        );
        assert_eq!(
            bucket.landed_keys(),
            ["run.json"],
            "not terminal: no marker"
        );
    }

    /// A pinned chunk that reads back EMPTY (a transient read error, a truncated
    /// file) is not "all sent": the pin stays, nothing is put, and once the
    /// bytes are readable again exactly that chunk is retried under its key —
    /// never a grown superset the CP may have half-consumed.
    #[tokio::test]
    async fn an_unreadable_pinned_chunk_stays_pinned_until_it_can_be_re_sent() {
        let run_dir = tempdir().unwrap();
        let stream = coverage_path(run_dir.path());
        std::fs::write(&stream, "a\nb\n").unwrap();
        let bucket = RecordingBucket::failing("coverage");
        let mut cur = BucketStream::default();

        assert!(!upload_new_lines(&bucket, "run_1", "coverage", &stream, &mut cur).await);
        let pinned = cur.pending_end;
        assert_eq!(pinned, Some(4));

        // The file cannot be read now; the bucket is back.
        std::fs::remove_file(&stream).unwrap();
        bucket.set_failing(None);
        assert!(!upload_new_lines(&bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!((cur.offset, cur.seq), (0, 0), "nothing committed");
        assert_eq!(cur.pending_end, pinned, "the pin is kept");
        assert!(bucket.landed().is_empty(), "nothing was uploaded");

        // Readable again, and the run has written more since.
        std::fs::write(&stream, "a\nb\nc\n").unwrap();
        assert!(upload_new_lines(&bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!(
            bucket.landed(),
            vec![
                ("coverage.0000.jsonl".to_string(), "a\nb\n".to_string()),
                ("coverage.0001.jsonl".to_string(), "c\n".to_string()),
            ]
        );
        assert_eq!((cur.seq, cur.pending_end), (2, None));
    }

    /// A bucket whose job-queue calls can be made to fail, over an in-memory
    /// store, for the claim step of a tick.
    struct FlakyJobBucket {
        inner: ObjectStoreBucket,
        fail_list_jobs: std::sync::atomic::AtomicBool,
        fail_claim_job: std::sync::atomic::AtomicBool,
        fail_get_job: std::sync::atomic::AtomicBool,
    }

    impl FlakyJobBucket {
        fn new() -> Self {
            Self {
                inner: ObjectStoreBucket::from_url("memory:///", None).unwrap(),
                fail_list_jobs: Default::default(),
                fail_claim_job: Default::default(),
                fail_get_job: Default::default(),
            }
        }

        fn set(flag: &std::sync::atomic::AtomicBool, on: bool) {
            flag.store(on, std::sync::atomic::Ordering::SeqCst);
        }

        fn is_on(flag: &std::sync::atomic::AtomicBool) -> bool {
            flag.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Bucket for FlakyJobBucket {
        async fn put_job(&self, run_id: &str, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_job(run_id, b).await
        }
        async fn list_jobs(&self) -> Result<Vec<String>, BucketError> {
            if Self::is_on(&self.fail_list_jobs) {
                return Err(BucketError::Io("injected list_jobs failure".into()));
            }
            self.inner.list_jobs().await
        }
        async fn claim_job(&self, run_id: &str, w: &str) -> Result<bool, BucketError> {
            if Self::is_on(&self.fail_claim_job) {
                return Err(BucketError::Io("injected claim_job failure".into()));
            }
            self.inner.claim_job(run_id, w).await
        }
        async fn get_job(&self, run_id: &str) -> Result<Vec<u8>, BucketError> {
            if Self::is_on(&self.fail_get_job) {
                return Err(BucketError::Io("injected get_job failure".into()));
            }
            self.inner.get_job(run_id).await
        }
        async fn put_control(&self, run_id: &str, seq: u64, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_control(run_id, seq, b).await
        }
        async fn list_control(&self, run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError> {
            self.inner.list_control(run_id).await
        }
        async fn put_result(&self, run_id: &str, key: &str, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_result(run_id, key, b).await
        }
        async fn list_results(&self, run_id: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError> {
            self.inner.list_results(run_id).await
        }
        async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError> {
            self.inner.put_finished(run_id, status).await
        }
        async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError> {
            self.inner.get_finished(run_id).await
        }
        async fn probe(&self) -> Result<(), BucketError> {
            self.inner.probe().await
        }
        async fn put_worker_info(&self, id: &str, b: &[u8]) -> Result<(), BucketError> {
            self.inner.put_worker_info(id, b).await
        }
        async fn list_worker_info(&self) -> Result<Vec<Vec<u8>>, BucketError> {
            self.inner.list_worker_info().await
        }
        async fn artifact_exists(&self, sha256: &str) -> Result<bool, BucketError> {
            self.inner.artifact_exists(sha256).await
        }
        async fn put_artifact_file(
            &self,
            sha256: &str,
            src: &std::path::Path,
        ) -> Result<(), BucketError> {
            self.inner.put_artifact_file(sha256, src).await
        }
        async fn get_artifact_to_file(
            &self,
            sha256: &str,
            dest: &std::path::Path,
            max_bytes: u64,
        ) -> Result<(), BucketError> {
            self.inner
                .get_artifact_to_file(sha256, dest, max_bytes)
                .await
        }
    }

    /// A harmless stand-in for the `rupu` executable: exits at once, ignoring
    /// whatever argv the worker builds.
    const TRUE_EXE: &str = "true";

    fn agent_job(name: &str) -> Vec<u8> {
        serde_json::to_vec(&RunSpec {
            kind: RunSpecKind::Agent,
            name: name.to_string(),
            inputs: BTreeMap::new(),
            prompt: Some("hi".into()),
            mode: None,
            target: None,
            findings_profile: None,
        })
        .unwrap()
    }

    /// One bucket tick against `TRUE_EXE`.
    async fn tick(
        bucket: &dyn Bucket,
        runs_root: &Path,
        active: &mut HashMap<String, BucketRunState>,
        pending: &mut PendingJobs,
    ) -> ClaimOutcome {
        bucket_tick(
            bucket,
            Path::new(TRUE_EXE),
            "host_1",
            runs_root,
            // The worker's global dir (its artifact store's root); these runs
            // reference no artifacts.
            runs_root.parent().unwrap_or(runs_root),
            active,
            pending,
        )
        .await
    }

    async fn idle_child() -> tokio::process::Child {
        tokio::process::Command::new(TRUE_EXE).spawn().unwrap()
    }

    /// A bucket outage on the claim step must not end the worker before the
    /// drain of the runs it already holds: a tick whose `list_jobs` fails still
    /// uploads an active run's lines (and a restart would lose every held
    /// chunk).
    #[tokio::test]
    async fn a_failing_list_jobs_tick_still_drains_an_active_run() {
        let bucket = FlakyJobBucket::new();
        FlakyJobBucket::set(&bucket.fail_list_jobs, true);
        let runs_root = tempdir().unwrap();
        std::fs::create_dir_all(runs_root.path().join("run_A")).unwrap();
        append(&runs_root.path().join("run_A").join("events.jsonl"), "e1\n");
        let mut active = HashMap::new();
        active.insert(
            "run_A".to_string(),
            BucketRunState {
                child: idle_child().await,
                streams: BucketStreams::default(),
                last_ctrl_seq: None,
            },
        );
        let mut pending = PendingJobs::default();

        let claim = tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert_eq!(claim.won, 0);
        assert!(
            claim
                .error
                .as_deref()
                .is_some_and(|e| e.contains("list_jobs")),
            "the tick reports the claim failure: {claim:?}"
        );

        let results = bucket.inner.list_results("run_A").await.unwrap();
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(results[0].0, "events.0000.jsonl");
        assert_eq!(std::str::from_utf8(&results[0].1).unwrap(), "e1\n");
        assert!(active.contains_key("run_A"), "the run is still in flight");
        assert!(pending.is_empty());
    }

    /// `--once` with nothing in flight exits 0 only if the claim step worked: a
    /// worker that claimed nothing BECAUSE the claim step failed did nothing,
    /// and "all runs terminal" would hide a broken bucket. One that did claim
    /// work is not failed by a later blip.
    #[test]
    fn once_exit_check_is_honest_about_a_failed_claim_step() {
        assert!(
            once_exit_check(false, None).is_ok(),
            "nothing to claim is fine"
        );
        assert!(once_exit_check(true, None).is_ok());
        assert!(
            once_exit_check(true, Some("list_jobs: boom")).is_ok(),
            "it ran what it claimed"
        );
        let err = once_exit_check(false, Some("list_jobs: boom")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("claim step failed") && msg.contains("list_jobs: boom"),
            "{msg}"
        );
    }

    /// The first tick of a `--once` worker against a bucket whose claim step
    /// fails leaves `active` and `pending` empty; what keeps that from reading
    /// as "all runs terminal" is the tick's reported claim error, for
    /// `list_jobs` and `claim_job` alike — and a healthy tick reports none.
    #[tokio::test]
    async fn a_tick_reports_whether_its_claim_step_failed() {
        let runs_root = tempdir().unwrap();
        let bucket = FlakyJobBucket::new();
        bucket.put_job("run_Q", &agent_job("a")).await.unwrap();

        FlakyJobBucket::set(&bucket.fail_list_jobs, true);
        let (mut active, mut pending) = (HashMap::new(), PendingJobs::default());
        let claim = tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert!(active.is_empty() && pending.is_empty());
        assert_eq!(claim.won, 0);
        let err = once_exit_check(false, claim.error.as_deref()).unwrap_err();
        assert!(format!("{err:#}").contains("list_jobs"), "{err:#}");

        FlakyJobBucket::set(&bucket.fail_list_jobs, false);
        FlakyJobBucket::set(&bucket.fail_claim_job, true);
        let claim = tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert!(active.is_empty() && pending.is_empty());
        assert_eq!(claim.won, 0);
        let err = once_exit_check(false, claim.error.as_deref()).unwrap_err();
        assert!(format!("{err:#}").contains("claim_job run_Q"), "{err:#}");

        // Healthy: the job is claimed and started, no claim error.
        FlakyJobBucket::set(&bucket.fail_claim_job, false);
        let claim = tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert_eq!(
            claim,
            ClaimOutcome {
                won: 1,
                error: None
            }
        );
        assert!(active.contains_key("run_Q"));
    }

    /// A failing `claim_job` skips that job for the tick (it is still listed
    /// next tick if the claim did not land); it neither ends the worker nor
    /// leaves the job half-claimed.
    #[tokio::test]
    async fn a_failing_claim_job_skips_the_job_and_it_is_claimed_next_tick() {
        let bucket = FlakyJobBucket::new();
        bucket.put_job("run_C", &agent_job("a")).await.unwrap();
        FlakyJobBucket::set(&bucket.fail_claim_job, true);
        let runs_root = tempdir().unwrap();
        let (mut active, mut pending) = (HashMap::new(), PendingJobs::default());

        tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert!(active.is_empty() && pending.is_empty());
        assert_eq!(bucket.inner.list_jobs().await.unwrap(), ["run_C"]);

        FlakyJobBucket::set(&bucket.fail_claim_job, false);
        tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert!(active.contains_key("run_C"));
    }

    /// A job claimed but whose spec could not be fetched is NOT dropped: the
    /// claim is ours and `list_jobs` no longer shows it, so nobody else would
    /// ever run it. It is parked and retried every tick.
    #[tokio::test]
    async fn a_claimed_job_whose_spec_cannot_be_fetched_is_retried_next_tick() {
        let bucket = FlakyJobBucket::new();
        bucket.put_job("run_G", &agent_job("a")).await.unwrap();
        FlakyJobBucket::set(&bucket.fail_get_job, true);
        let runs_root = tempdir().unwrap();
        let (mut active, mut pending) = (HashMap::new(), PendingJobs::default());

        tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert!(active.is_empty());
        assert_eq!(pending.unfetched, ["run_G"]);
        assert!(
            bucket.inner.list_jobs().await.unwrap().is_empty(),
            "the claim landed: the job is no longer listed"
        );

        FlakyJobBucket::set(&bucket.fail_get_job, false);
        tick(&bucket, runs_root.path(), &mut active, &mut pending).await;
        assert!(active.contains_key("run_G"), "spawned on the retry");
        assert!(pending.is_empty());
    }

    /// A job whose spec does not deserialize is unusable and no retry fixes it:
    /// it is marked `failed` (as a spawn failure is) and the worker carries on.
    #[tokio::test]
    async fn a_malformed_job_spec_is_marked_failed_and_does_not_end_the_worker() {
        let bucket = FlakyJobBucket::new();
        bucket
            .put_job("run_BAD", b"{ not a run spec")
            .await
            .unwrap();
        bucket.put_job("run_OK", &agent_job("a")).await.unwrap();
        let runs_root = tempdir().unwrap();
        let (mut active, mut pending) = (HashMap::new(), PendingJobs::default());

        tick(&bucket, runs_root.path(), &mut active, &mut pending).await;

        assert_eq!(
            bucket.get_finished("run_BAD").await.unwrap().as_deref(),
            Some("failed")
        );
        assert!(pending.is_empty());
        assert!(
            active.contains_key("run_OK"),
            "the good job beside it still runs"
        );
        assert!(!active.contains_key("run_BAD"));
    }

    /// A claimed job whose spawn failed is marked `failed`; a marker that did
    /// not land is retried on later ticks instead of being dropped.
    #[tokio::test]
    async fn a_failed_spawn_marker_that_did_not_land_is_retried() {
        let fb = FailingBucket::new();
        let mut pending = vec!["run_a".to_string(), "run_b".to_string()];

        flush_failed_markers(&fb.bucket, &mut pending).await;
        assert_eq!(
            pending,
            ["run_a", "run_b"],
            "still unmarked: kept for the next tick"
        );

        fb.heal();
        flush_failed_markers(&fb.bucket, &mut pending).await;
        assert!(pending.is_empty());
        for rid in ["run_a", "run_b"] {
            assert_eq!(
                fb.bucket.get_finished(rid).await.unwrap().as_deref(),
                Some("failed")
            );
        }
        // Nothing pending: nothing written, nothing to do.
        flush_failed_markers(&fb.bucket, &mut pending).await;
    }

    // ------------------------------------------------------------------
    // Artifact upload at run end
    // ------------------------------------------------------------------

    /// A `ledger:"findings"` stream line whose full report references
    /// `artifacts`.
    fn findings_stream_line(
        id: &str,
        artifacts: Vec<(&str, rupu_coverage::report::ArtifactStorage)>,
    ) -> String {
        use rupu_coverage::report::{ArtifactKind, ArtifactRef, FindingReport};
        let mut report: FindingReport = serde_json::from_str(include_str!(
            "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        report.artifacts = artifacts
            .into_iter()
            .map(|(sha, stored)| ArtifactRef {
                path: "poc/x".into(),
                sha256: sha.into(),
                size: 1,
                kind: Some(ArtifactKind::Binary),
                stored: Some(stored),
                host: None,
            })
            .collect();
        let record = rupu_coverage::FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: rupu_coverage::FindingScope::File,
            summary: "s".into(),
            severity: rupu_coverage::Severity::High,
            concern_id: None,
            evidence: rupu_coverage::FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: rupu_coverage::Attribution {
                run_id: "r".into(),
                model: "m".into(),
                surface: rupu_coverage::Surface::Agent,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: chrono::Utc::now(),
            profile: rupu_coverage::FindingProfile::Full,
            report: Some(report),
        };
        serde_json::to_string(&rupu_coverage::StreamLine::Findings {
            scope_name: "sec".into(),
            record,
        })
        .unwrap()
            + "\n"
    }

    fn begin_stream_line() -> String {
        serde_json::to_string(&rupu_coverage::StreamLine::Begin {
            v: 1,
            run_id: "r".into(),
        })
        .unwrap()
            + "\n"
    }

    #[test]
    fn referenced_artifacts_lists_only_copied_blobs_from_findings() {
        use rupu_coverage::report::ArtifactStorage;
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join(rupu_coverage::STREAM_FILE);
        let copied = "ab".repeat(32);
        let external = "cd".repeat(32);
        std::fs::write(
            &stream,
            begin_stream_line()
                + &findings_stream_line(
                    "f1",
                    vec![
                        (&copied, ArtifactStorage::Copied),
                        (&external, ArtifactStorage::External),
                    ],
                ),
        )
        .unwrap();
        assert_eq!(
            referenced_artifacts(&stream),
            std::collections::BTreeSet::from([copied])
        );
        assert!(referenced_artifacts(&tmp.path().join("absent")).is_empty());
    }

    /// The same blob referenced by two findings is listed once; a blob with
    /// a malformed hash (never a store key) and a torn trailing line are
    /// ignored rather than failing the scan.
    #[test]
    fn referenced_artifacts_dedups_and_skips_malformed_entries_and_torn_lines() {
        use rupu_coverage::report::ArtifactStorage;
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join(rupu_coverage::STREAM_FILE);
        let a = "ab".repeat(32);
        let b = "ef".repeat(32);
        let body = begin_stream_line()
            + &findings_stream_line("f1", vec![(&a, ArtifactStorage::Copied)])
            + &findings_stream_line(
                "f2",
                vec![
                    (&a, ArtifactStorage::Copied),
                    (&b, ArtifactStorage::Copied),
                    ("../../etc/passwd", ArtifactStorage::Copied),
                    (&"AB".repeat(32), ArtifactStorage::Copied),
                ],
            )
            + "{\"ledger\":\"findings\",\"scope_na";
        std::fs::write(&stream, body).unwrap();
        assert_eq!(
            referenced_artifacts(&stream),
            std::collections::BTreeSet::from([a, b])
        );
    }

    /// A run killed mid-write can leave the stream's last line cut inside a
    /// multi-byte UTF-8 character. That torn tail is skipped like any other
    /// unparseable line; it must not lose the references on the valid lines
    /// before it.
    #[test]
    fn referenced_artifacts_survive_a_tail_torn_mid_utf8_character() {
        use rupu_coverage::report::ArtifactStorage;
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join(rupu_coverage::STREAM_FILE);
        let a = "ab".repeat(32);
        let mut body = (begin_stream_line()
            + &findings_stream_line("f1", vec![(&a, ArtifactStorage::Copied)]))
            .into_bytes();
        // `{"ledger":"findings","scope_name":"caf` + the first byte of "é".
        body.extend_from_slice(b"{\"ledger\":\"findings\",\"scope_name\":\"caf");
        body.push(0xC3);
        assert!(
            String::from_utf8(body.clone()).is_err(),
            "tail must be invalid UTF-8"
        );
        std::fs::write(&stream, body).unwrap();
        assert_eq!(
            referenced_artifacts(&stream),
            std::collections::BTreeSet::from([a])
        );
    }

    /// The terminal pass uploads each `stored: copied` blob its run's findings
    /// reference from the worker's store to `artifacts/<sha>` after the final
    /// coverage drain and BEFORE the terminal `run.json` and the `finished`
    /// marker (a coordinator that sees the run terminal can already pull),
    /// even for a failed run; one the bucket already holds is not re-uploaded;
    /// one the store lacks is skipped without blocking `finished`.
    #[tokio::test]
    async fn bucket_terminal_pass_uploads_referenced_blobs_before_finished() {
        use rupu_coverage::report::ArtifactStorage;
        let global = tempdir().unwrap();
        let run_dir = tempdir().unwrap();
        let fresh = "ab".repeat(32);
        let already = "cd".repeat(32);
        let absent = "ee".repeat(32);
        let external = "f0".repeat(32);
        store_blob(global.path(), &fresh, b"fresh poc");
        store_blob(global.path(), &already, b"already uploaded");
        store_blob(global.path(), &external, b"never uploaded");
        std::fs::write(
            run_dir.path().join(rupu_coverage::STREAM_FILE),
            begin_stream_line()
                + &findings_stream_line(
                    "f1",
                    vec![
                        (&fresh, ArtifactStorage::Copied),
                        (&already, ArtifactStorage::Copied),
                        (&absent, ArtifactStorage::Copied),
                        (&external, ArtifactStorage::External),
                    ],
                ),
        )
        .unwrap();

        let bucket = RecordingBucket::default();
        bucket
            .artifacts
            .lock()
            .unwrap()
            .insert(already.clone(), b"already uploaded".to_vec());
        let mut streams = BucketStreams::default();
        assert!(
            finish_bucket_run(
                &bucket,
                "run_ART",
                run_dir.path(),
                global.path(),
                &mut streams,
                br#"{"status":"failed"}"#,
                "failed",
            )
            .await
        );

        let keys = bucket.landed_keys();
        assert_eq!(
            keys,
            vec![
                result_key("coverage", 0),
                format!("artifact:{fresh}"),
                "run.json".to_string(),
                "finished".to_string(),
            ],
            "{keys:?}"
        );
        assert_eq!(
            bucket.landed_body(&format!("artifact:{fresh}")).as_deref(),
            Some("fresh poc")
        );
        assert_eq!(bucket.landed_body("finished").as_deref(), Some("failed"));
        let stored = bucket.artifacts.lock().unwrap();
        assert_eq!(
            stored.get(&fresh).map(Vec::as_slice),
            Some(&b"fresh poc"[..])
        );
        assert!(!stored.contains_key(&absent) && !stored.contains_key(&external));
    }

    /// A blob upload that fails is part of what the terminal pass holds the
    /// run for: neither the terminal `run.json` nor the `finished` marker is
    /// written (the CP would stop polling the run and never learn the blob is
    /// there), and the next tick retries — the blob, then `run.json`, then
    /// `finished`, with the coverage chunk that already landed not re-sent.
    #[tokio::test]
    async fn a_failed_blob_upload_holds_the_terminal_run_until_it_lands() {
        use rupu_coverage::report::ArtifactStorage;
        let global = tempdir().unwrap();
        let run_dir = tempdir().unwrap();
        let sha = "ab".repeat(32);
        store_blob(global.path(), &sha, b"poc");
        std::fs::write(
            run_dir.path().join(rupu_coverage::STREAM_FILE),
            begin_stream_line()
                + &findings_stream_line("f1", vec![(&sha, ArtifactStorage::Copied)]),
        )
        .unwrap();
        let bucket = RecordingBucket::failing("artifact:");
        let mut streams = BucketStreams::default();

        assert!(
            !finish_bucket_run(
                &bucket,
                "run_HOLD",
                run_dir.path(),
                global.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await,
            "an unlanded blob means the run is not finished"
        );
        assert_eq!(bucket.landed_keys(), vec![result_key("coverage", 0)]);

        bucket.set_failing(None);
        assert!(
            finish_bucket_run(
                &bucket,
                "run_HOLD",
                run_dir.path(),
                global.path(),
                &mut streams,
                RUN_JSON,
                "completed",
            )
            .await
        );
        assert_eq!(
            bucket.landed_keys(),
            vec![
                result_key("coverage", 0),
                format!("artifact:{sha}"),
                "run.json".to_string(),
                "finished".to_string(),
            ]
        );
    }

    /// A run with no usage ledger yet (or never) drains to nothing.
    #[test]
    fn drain_usage_ledger_missing_file_is_empty() {
        let dir = tempdir().unwrap();
        let mut off = offsets();
        assert!(drain_usage_ledger(dir.path(), &mut off, true).is_empty());
        assert_eq!(off.usage, 0);
    }

    #[test]
    fn result_key_usage_matches_the_poller_classifier() {
        // The CP's bucket poller classifies by the `usage` prefix + `.jsonl`.
        assert_eq!(result_key("usage", 0), "usage.0000.jsonl");
        assert_eq!(result_key("usage", 7), "usage.0007.jsonl");
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
            findings_profile: None,
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
            findings_profile: None,
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
            findings_profile: None,
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
            findings_profile: None,
        };
        let argv = build_argv("run_W", &spec);
        assert_eq!(argv, vec!["run", "check", "--run-id", "run_W"]);
    }

    #[test]
    fn build_argv_agent_carries_the_findings_profile() {
        let spec = RunSpec {
            kind: RunSpecKind::Agent,
            name: "sec".to_string(),
            inputs: BTreeMap::new(),
            prompt: Some("audit".to_string()),
            mode: None,
            target: None,
            findings_profile: Some(rupu_coverage::FindingProfile::Summary),
        };
        let argv = build_argv("run_P", &spec);
        assert_eq!(
            argv,
            vec![
                "run",
                "sec",
                "--run-id",
                "run_P",
                "--findings-profile",
                "summary",
                "--prompt",
                "audit"
            ]
        );
        // Round-trips through the real `rupu run` parser.
        let args = crate::cmd::run::parse_launch_args(argv[1..].to_vec()).unwrap();
        assert_eq!(
            args.findings_profile,
            Some(rupu_coverage::FindingProfile::Summary)
        );
        check_spec(&spec).expect("an agent spec may carry a profile");
    }

    #[test]
    fn a_workflow_spec_carrying_a_findings_profile_is_refused() {
        let spec = RunSpec {
            kind: RunSpecKind::Workflow,
            name: "audit".to_string(),
            inputs: BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: Some(rupu_coverage::FindingProfile::Full),
        };
        let err = check_spec(&spec).unwrap_err().to_string();
        assert!(err.contains("agent runs only"), "{err}");
    }

    #[test]
    fn the_node_advertises_findings_profile_support() {
        assert!(rupu_cp::node::protocol::node_capabilities()
            .iter()
            .any(|c| c == rupu_cp::node::protocol::CAP_AGENT_FINDINGS_PROFILE));
    }

    // ------------------------------------------------------------------
    // build_control_argv: approve/reject argv builders
    // ------------------------------------------------------------------

    #[test]
    fn control_argv_approve_with_and_without_mode() {
        assert_eq!(
            build_control_argv(ControlKind::Approve, "run_1", "bypass", None),
            vec!["workflow", "approve", "run_1", "--mode", "bypass"]
        );
        assert_eq!(
            build_control_argv(ControlKind::Approve, "run_1", "", None),
            vec!["workflow", "approve", "run_1"]
        );
    }

    #[test]
    fn control_argv_reject_with_and_without_reason() {
        assert_eq!(
            build_control_argv(ControlKind::Reject, "run_1", "", Some("nope")),
            vec!["workflow", "reject", "run_1", "--reason", "nope"]
        );
        assert_eq!(
            build_control_argv(ControlKind::Reject, "run_1", "", None),
            vec!["workflow", "reject", "run_1"]
        );
    }

    // ------------------------------------------------------------------
    // next_backoff
    // ------------------------------------------------------------------

    #[test]
    fn next_backoff_doubles() {
        assert_eq!(next_backoff(1), 2);
    }

    #[test]
    fn next_backoff_caps_at_60() {
        assert_eq!(next_backoff(40), 60);
    }

    // ------------------------------------------------------------------
    // normalize_cp_url
    // ------------------------------------------------------------------

    #[test]
    fn normalize_appends_path_to_bare_host() {
        assert_eq!(
            normalize_cp_url("ws://cp.example.com"),
            "ws://cp.example.com/api/node/connect"
        );
    }

    #[test]
    fn normalize_appends_path_to_host_with_port() {
        assert_eq!(
            normalize_cp_url("ws://10.0.0.5:7878"),
            "ws://10.0.0.5:7878/api/node/connect"
        );
    }

    #[test]
    fn normalize_handles_trailing_slash() {
        assert_eq!(
            normalize_cp_url("ws://h:7878/"),
            "ws://h:7878/api/node/connect"
        );
    }

    #[test]
    fn normalize_leaves_explicit_path_alone() {
        assert_eq!(
            normalize_cp_url("ws://h:7878/api/node/connect"),
            "ws://h:7878/api/node/connect"
        );
        assert_eq!(
            normalize_cp_url("ws://h:7878/custom/prefix"),
            "ws://h:7878/custom/prefix"
        );
    }

    #[test]
    fn normalize_preserves_wss_scheme() {
        assert_eq!(
            normalize_cp_url("wss://h:7878"),
            "wss://h:7878/api/node/connect"
        );
    }

    #[test]
    fn normalize_passes_through_schemeless_input() {
        // Garbage in, garbage out — the connect call will produce the error.
        assert_eq!(normalize_cp_url("not-a-url"), "not-a-url");
    }

    // ------------------------------------------------------------------
    // reject_wss
    // ------------------------------------------------------------------

    #[test]
    fn reject_wss_fails_fast_with_friendly_message() {
        let err = reject_wss("wss://cp.example.com/api/node/connect")
            .expect_err("wss:// must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("wss:// is not supported by this build"),
            "unexpected message: {msg}"
        );
        assert!(msg.contains("use ws://"), "unexpected message: {msg}");
    }

    #[test]
    fn reject_wss_allows_plain_ws() {
        reject_wss("ws://cp.example.com/api/node/connect").expect("ws:// must pass");
    }

    // ------------------------------------------------------------------
    // enroll_commands
    // ------------------------------------------------------------------

    #[test]
    fn enroll_commands_default_uses_detected_ip_and_full_url() {
        let ip: std::net::IpAddr = "192.168.1.23".parse().unwrap();
        let (cmd, stdin_cmd) = enroll_commands(None, Some(ip), "tok123", "node_01AAA");
        assert_eq!(
            cmd,
            "rupu node --cp-url ws://192.168.1.23:7878/api/node/connect --token tok123 --node-id node_01AAA"
        );
        assert!(stdin_cmd.contains("ws://192.168.1.23:7878/api/node/connect"));
        assert!(stdin_cmd.contains("--token-stdin"));
        assert!(stdin_cmd.contains("--node-id node_01AAA"));
    }

    #[test]
    fn enroll_commands_normalizes_given_cp_url() {
        let (cmd, _) = enroll_commands(Some("ws://h:7878"), None, "tok", "node_1");
        assert!(
            cmd.contains("--cp-url ws://h:7878/api/node/connect"),
            "path must be appended: {cmd}"
        );
        assert!(cmd.contains("--node-id node_1"), "cmd: {cmd}");
    }

    #[test]
    fn enroll_commands_placeholder_when_no_ip_detected() {
        let (cmd, _) = enroll_commands(None, None, "tok", "node_1");
        assert!(
            cmd.contains("ws://<cp-host>/api/node/connect"),
            "cmd: {cmd}"
        );
        assert!(!cmd.contains("wss://"), "must not suggest wss: {cmd}");
    }

    // ------------------------------------------------------------------
    // detect_routable_ip (shared helper in rupu-cp)
    // ------------------------------------------------------------------

    #[test]
    fn detect_routable_ip_not_loopback_when_some() {
        // CI sandboxes may yield None — that is acceptable; but when an IP
        // is detected it must be a real interface address, never loopback.
        if let Some(ip) = rupu_cp::net::detect_routable_ip() {
            assert!(!ip.is_loopback(), "detected loopback: {ip}");
        }
    }

    // ------------------------------------------------------------------
    // drain_coverage: the run's coverage stream, tailed incrementally
    // ------------------------------------------------------------------

    #[test]
    fn drain_coverage_reads_the_run_stream_incrementally() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path();
        std::fs::write(run_dir.join(rupu_coverage::STREAM_FILE), "a\nb\n").unwrap();
        let mut off = 0u64;
        assert_eq!(drain_coverage(run_dir, &mut off), vec!["a", "b"]);
        std::fs::write(run_dir.join(rupu_coverage::STREAM_FILE), "a\nb\nc\n").unwrap();
        assert_eq!(drain_coverage(run_dir, &mut off), vec!["c"]);
    }

    /// An in-memory `Sink` that records what the tunnel would put on the wire.
    #[derive(Default)]
    struct VecSink(Vec<Message>);

    impl futures_util::Sink<Message> for VecSink {
        type Error = tokio_tungstenite::tungstenite::Error;

        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn start_send(
            mut self: std::pin::Pin<&mut Self>,
            item: Message,
        ) -> Result<(), Self::Error> {
            self.0.push(item);
            Ok(())
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    impl VecSink {
        /// Every frame sent, as `(run_id, line)` — asserting each one is a
        /// `Frame::Artifact { file: Coverage, .. }`.
        fn coverage_frames(&self) -> Vec<(String, String)> {
            self.0
                .iter()
                .map(|m| match parse_frame(m).unwrap() {
                    Frame::Artifact {
                        run_id,
                        file: ArtifactFile::Coverage,
                        line,
                    } => (run_id, line),
                    other => panic!("expected a coverage artifact frame, got {other:?}"),
                })
                .collect()
        }
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    /// A CP that never advertised `mirror.coverage` can't parse the frame:
    /// nothing is sent, and the offset stays put so a later connection whose
    /// CP does advertise it still ships the whole stream.
    #[tokio::test]
    async fn ship_coverage_gated_off_sends_nothing_and_does_not_move_the_offset() {
        let run_dir = tempdir().unwrap();
        append(&run_dir.path().join(rupu_coverage::STREAM_FILE), "a\nb\n");
        let mut sink = VecSink::default();
        let mut off = 0u64;

        ship_coverage(&mut sink, "run_1", run_dir.path(), &mut off, false).await;
        assert!(sink.0.is_empty(), "no frame to an incapable CP");
        assert_eq!(
            off, 0,
            "nothing consumed that a capable connection could send"
        );

        // The same stream, now to a capable CP: everything still ships.
        ship_coverage(&mut sink, "run_1", run_dir.path(), &mut off, true).await;
        assert_eq!(
            sink.coverage_frames(),
            vec![("run_1".into(), "a".into()), ("run_1".into(), "b".into())]
        );
    }

    /// One frame per complete line, in order; a trailing partial line (the
    /// writer is mid-flush) is held back until its newline lands.
    #[tokio::test]
    async fn ship_coverage_sends_each_complete_line_in_order_and_holds_back_a_partial() {
        let run_dir = tempdir().unwrap();
        let stream = run_dir.path().join(rupu_coverage::STREAM_FILE);
        append(&stream, "{\"n\":1}\n{\"n\":2}\n{\"n\":3");
        let mut sink = VecSink::default();
        let mut off = 0u64;

        ship_coverage(&mut sink, "run_9", run_dir.path(), &mut off, true).await;
        assert_eq!(
            sink.coverage_frames(),
            vec![
                ("run_9".to_string(), "{\"n\":1}".to_string()),
                ("run_9".to_string(), "{\"n\":2}".to_string()),
            ],
            "the unterminated third line must not be sent yet"
        );

        // The newline arrives: now (and only now) the third line goes.
        append(&stream, "}\n");
        ship_coverage(&mut sink, "run_9", run_dir.path(), &mut off, true).await;
        let frames = sink.coverage_frames();
        assert_eq!(frames.len(), 3, "{frames:?}");
        assert_eq!(frames[2], ("run_9".to_string(), "{\"n\":3}".to_string()));
    }

    /// The terminal / cancel drain is a SECOND call after the routine one: it
    /// ships exactly the lines appended in between, never the ones already sent.
    #[tokio::test]
    async fn ship_coverage_second_call_sends_only_the_lines_appended_since() {
        let run_dir = tempdir().unwrap();
        let stream = run_dir.path().join(rupu_coverage::STREAM_FILE);
        append(&stream, "a\nb\n");
        let mut sink = VecSink::default();
        let mut off = 0u64;

        ship_coverage(&mut sink, "run_2", run_dir.path(), &mut off, true).await;
        assert_eq!(sink.0.len(), 2);

        // Nothing new: the terminal drain of a quiet run sends nothing.
        ship_coverage(&mut sink, "run_2", run_dir.path(), &mut off, true).await;
        assert_eq!(sink.0.len(), 2, "no re-send of lines already shipped");

        // The run wrote its last lines just before turning terminal.
        append(&stream, "c\nd\n");
        ship_coverage(&mut sink, "run_2", run_dir.path(), &mut off, true).await;
        let lines: Vec<String> = sink.coverage_frames().into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            lines,
            ["a", "b", "c", "d"],
            "each line exactly once, in order"
        );
    }

    /// `peek_new_lines` reads without committing; `drain_new_lines` is the
    /// peek plus the commit.
    #[test]
    fn peek_new_lines_does_not_commit_the_offset() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("f.jsonl");
        std::fs::write(&path, "a\nb\npartial").unwrap();

        let (lines, next) = peek_new_lines(&path, 0);
        assert_eq!(lines, ["a", "b"]);
        assert_eq!(
            next, 4,
            "just past the last newline; the partial is held back"
        );
        // Peeking again from the same offset sees the same lines.
        assert_eq!(
            peek_new_lines(&path, 0),
            (vec!["a".to_string(), "b".to_string()], 4)
        );
        // From the returned offset there is nothing complete left.
        assert_eq!(peek_new_lines(&path, next), (vec![], next));
        // Missing file: no lines, offset handed back unchanged.
        assert_eq!(peek_new_lines(&dir.path().join("nope"), 7), (vec![], 7));

        let mut off = 0u64;
        assert_eq!(drain_new_lines(&path, &mut off), ["a", "b"]);
        assert_eq!(off, 4);
    }

    #[tokio::test]
    async fn put_lines_uploads_one_numbered_object_and_advances_seq() {
        let bucket = ObjectStoreBucket::from_url("memory:///", None).unwrap();
        let mut seq = 0u64;

        // Nothing to upload: no object, seq unchanged.
        put_lines(&bucket, "run_1", "coverage", vec![], &mut seq).await;
        assert_eq!(seq, 0);
        assert!(bucket.list_results("run_1").await.unwrap().is_empty());

        put_lines(
            &bucket,
            "run_1",
            "coverage",
            vec!["a".into(), "b".into()],
            &mut seq,
        )
        .await;
        put_lines(&bucket, "run_1", "coverage", vec!["c".into()], &mut seq).await;
        assert_eq!(seq, 2);

        let results = bucket.list_results("run_1").await.unwrap();
        let got: Vec<(&str, &str)> = results
            .iter()
            .map(|(k, v)| (k.as_str(), std::str::from_utf8(v).unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("coverage.0000.jsonl", "a\nb\n"),
                ("coverage.0001.jsonl", "c\n")
            ]
        );
    }

    // ------------------------------------------------------------------
    // bucket uploads: a failed put must not consume what it was uploading
    // ------------------------------------------------------------------

    /// A REAL `ObjectStoreBucket` (a `file://` root) whose puts fail: a regular
    /// file sits where the `runs/` directory has to be created, so the object
    /// store cannot lay down any result object. [`FailingBucket::heal`] removes
    /// the file and the very next put lands. No trait double — the failure is
    /// the store's own.
    struct FailingBucket {
        root: tempfile::TempDir,
        bucket: ObjectStoreBucket,
    }

    impl FailingBucket {
        fn new() -> Self {
            let root = tempdir().unwrap();
            std::fs::write(root.path().join("runs"), b"in the way").unwrap();
            let bucket =
                ObjectStoreBucket::from_url(&format!("file://{}", root.path().display()), None)
                    .unwrap();
            Self { root, bucket }
        }

        fn heal(&self) {
            std::fs::remove_file(self.root.path().join("runs")).unwrap();
        }
    }

    /// The put fails while the run keeps writing, then the bucket recovers: the
    /// rows the failed put carried are retried under the same key, byte for
    /// byte, and the row written meanwhile is the NEXT object.
    #[tokio::test]
    async fn upload_usage_ledger_failed_put_keeps_the_rows_for_the_next_tick() {
        let run_dir = tempdir().unwrap();
        let ledger = usage_path(run_dir.path());
        std::fs::write(&ledger, "{\"id\":\"U1\"}\n{\"id\":\"U2\"}\n").unwrap();
        let fb = FailingBucket::new();
        let mut usage = BucketStream::default();

        assert!(!upload_usage_ledger(&fb.bucket, "run_1", run_dir.path(), &mut usage).await);
        assert_eq!(usage.seq, 0, "a failed put must not burn the object number");
        assert_eq!(usage.offset, 0, "a failed put must not consume the rows");

        // The run appends a row while the bucket is down; then the bucket heals.
        append(&ledger, "{\"id\":\"U3\"}\n");
        fb.heal();

        assert!(upload_usage_ledger(&fb.bucket, "run_1", run_dir.path(), &mut usage).await);
        assert_eq!(usage.seq, 2);
        let results = fb.bucket.list_results("run_1").await.unwrap();
        let got: Vec<(&str, &str)> = results
            .iter()
            .map(|(k, v)| (k.as_str(), std::str::from_utf8(v).unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("usage.0000.jsonl", "{\"id\":\"U1\"}\n{\"id\":\"U2\"}\n"),
                ("usage.0001.jsonl", "{\"id\":\"U3\"}\n"),
            ],
            "same key as the failed try, same bytes; the new row is its own object"
        );
    }

    /// The coverage stream (and every other kind) goes through the same
    /// peek / put / commit helper: a failed put leaves the offset AND the seq
    /// unchanged and pins the chunk; the retry uploads exactly that chunk under
    /// the same key, and lines written since follow under the next key.
    #[tokio::test]
    async fn upload_new_lines_failed_put_leaves_offset_and_seq_and_the_retry_reuses_the_key() {
        let run_dir = tempdir().unwrap();
        let stream = coverage_path(run_dir.path());
        std::fs::write(&stream, "begin\nfinding\n").unwrap();
        let first_chunk_end = std::fs::metadata(&stream).unwrap().len();
        let fb = FailingBucket::new();
        let mut cur = BucketStream::default();

        assert!(!upload_new_lines(&fb.bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!(
            (cur.offset, cur.seq),
            (0, 0),
            "a failed put commits nothing"
        );
        assert_eq!(cur.pending_end, Some(first_chunk_end));

        // Still failing on the next tick, with more lines on disk: still nothing,
        // and the pinned chunk does not grow.
        append(&stream, "end\n");
        assert!(!upload_new_lines(&fb.bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!((cur.offset, cur.seq), (0, 0));
        assert_eq!(cur.pending_end, Some(first_chunk_end));

        fb.heal();
        assert!(upload_new_lines(&fb.bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!(cur.seq, 2);
        assert_eq!(cur.offset, std::fs::metadata(&stream).unwrap().len());
        assert_eq!(cur.pending_end, None);
        let results = fb.bucket.list_results("run_1").await.unwrap();
        let got: Vec<(&str, &str)> = results
            .iter()
            .map(|(k, v)| (k.as_str(), std::str::from_utf8(v).unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("coverage.0000.jsonl", "begin\nfinding\n"),
                ("coverage.0001.jsonl", "end\n")
            ],
            "nothing lost, nothing duplicated; the retry is the original chunk"
        );

        // Healthy bucket: the next lines are the next numbered object.
        append(&stream, "late\n");
        assert!(upload_new_lines(&fb.bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!(cur.seq, 3);
        let results = fb.bucket.list_results("run_1").await.unwrap();
        assert_eq!(results[2].0, "coverage.0002.jsonl");
        assert_eq!(std::str::from_utf8(&results[2].1).unwrap(), "late\n");

        // Nothing new: no object, nothing moves.
        let (off, seq) = (cur.offset, cur.seq);
        assert!(upload_new_lines(&fb.bucket, "run_1", "coverage", &stream, &mut cur).await);
        assert_eq!((cur.offset, cur.seq), (off, seq));
        assert_eq!(fb.bucket.list_results("run_1").await.unwrap().len(), 3);
    }

    /// A put that errors AFTER landing (a timeout) can already have been
    /// consumed by the CP poller, which skips keys it has seen. The retry must
    /// therefore carry the same bytes under the same key — a retry that had
    /// grown would have its extra lines skipped for good — and the lines
    /// written since go under the next key.
    #[tokio::test]
    async fn a_retried_chunk_is_byte_identical_even_when_the_run_wrote_more() {
        let run_dir = tempdir().unwrap();
        let stream = coverage_path(run_dir.path());
        std::fs::write(&stream, "a\nb\n").unwrap();
        let bucket = RecordingBucket::failing("coverage");
        let mut cur = BucketStream::default();

        assert!(!upload_new_lines(&bucket, "run_1", "coverage", &stream, &mut cur).await);
        append(&stream, "c\n");
        assert!(!upload_new_lines(&bucket, "run_1", "coverage", &stream, &mut cur).await);
        append(&stream, "d\n");
        bucket.set_failing(None);
        assert!(upload_new_lines(&bucket, "run_1", "coverage", &stream, &mut cur).await);

        let attempts = bucket.attempts.lock().unwrap().clone();
        let a = |k: &str, b: &str| (k.to_string(), b.to_string());
        assert_eq!(
            attempts,
            vec![
                a("coverage.0000.jsonl", "a\nb\n"), // failed (may have landed)
                a("coverage.0000.jsonl", "a\nb\n"), // failed again: identical
                a("coverage.0000.jsonl", "a\nb\n"), // landed: identical
                a("coverage.0001.jsonl", "c\nd\n"), // everything since
            ]
        );
        assert_eq!(cur.seq, 2);
        assert_eq!(cur.pending_end, None);
    }

    /// The bounded peek reads exactly `[offset, end)` however far the file grew.
    #[test]
    fn peek_lines_until_reads_only_the_pinned_chunk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("f.jsonl");
        std::fs::write(&path, "a\nb\nc\npartial").unwrap();
        assert_eq!(
            peek_lines_until(&path, 0, 4),
            (vec!["a".to_string(), "b".to_string()], 4)
        );
        assert_eq!(
            peek_lines_until(&path, 2, 6),
            (vec!["b".to_string(), "c".to_string()], 6)
        );
        // An end past the file is clamped to the complete lines there.
        assert_eq!(peek_lines_until(&path, 0, 99).1, 6);
        // Nothing complete in the range, or a missing file: unchanged offset.
        assert_eq!(peek_lines_until(&path, 6, 99), (vec![], 6));
        assert_eq!(
            peek_lines_until(&dir.path().join("nope"), 3, 9),
            (vec![], 3)
        );
    }

    /// `put_lines` alone: a failed upload leaves `seq` where it was and reports
    /// that nothing landed, so the caller keeps its offset.
    #[tokio::test]
    async fn put_lines_reports_whether_the_object_landed() {
        let fb = FailingBucket::new();
        let mut seq = 3u64;
        assert!(!put_lines(&fb.bucket, "run_1", "events", vec!["x".into()], &mut seq).await);
        assert_eq!(seq, 3);

        fb.heal();
        assert!(put_lines(&fb.bucket, "run_1", "events", vec!["x".into()], &mut seq).await);
        assert_eq!(seq, 4);
        // No lines is trivially "nothing left to commit".
        assert!(put_lines(&fb.bucket, "run_1", "events", vec![], &mut seq).await);
        assert_eq!(seq, 4);
    }

    // ------------------------------------------------------------------
    // result_key: bucket result object key helper
    // ------------------------------------------------------------------

    #[test]
    fn result_key_zero_padded_four_digits() {
        assert_eq!(result_key("events", 0), "events.0000.jsonl");
        assert_eq!(result_key("events", 1), "events.0001.jsonl");
        assert_eq!(result_key("events", 42), "events.0042.jsonl");
        assert_eq!(result_key("step_results", 9999), "step_results.9999.jsonl");
        assert_eq!(
            result_key("unit_checkpoints", 100),
            "unit_checkpoints.0100.jsonl"
        );
    }

    #[test]
    fn result_key_overflows_past_9999() {
        // Beyond four digits the seq simply expands — still monotonic.
        assert_eq!(result_key("events", 10000), "events.10000.jsonl");
    }

    // ------------------------------------------------------------------
    // next_control_seq: watermark helper
    // ------------------------------------------------------------------

    #[test]
    fn next_control_seq_empty_returns_zero() {
        assert_eq!(next_control_seq(&[]), 0);
    }

    #[test]
    fn next_control_seq_single_item() {
        assert_eq!(next_control_seq(&[(0, vec![])]), 1);
        assert_eq!(next_control_seq(&[(5, vec![])]), 6);
    }

    #[test]
    fn next_control_seq_multiple_items_returns_max_plus_one() {
        let items = vec![(1u64, vec![]), (3u64, vec![]), (2u64, vec![])];
        assert_eq!(next_control_seq(&items), 4, "should be max(1,3,2)+1=4");
    }

    // ------------------------------------------------------------------
    // ArtifactPullStream: one ArtifactPull answered frame by frame
    // ------------------------------------------------------------------

    /// Every frame `stream` yields, in order, until it ends.
    async fn collect_pull(stream: &mut ArtifactPullStream) -> Vec<Frame> {
        let mut frames = Vec::new();
        while let Some(f) = stream.next_frame().await {
            frames.push(f);
            assert!(frames.len() < 1000, "the pull stream never ended");
        }
        frames
    }

    /// Put `body` in `global`'s artifact store under `sha`.
    fn store_blob(global: &Path, sha: &str, body: &[u8]) {
        let p = crate::cmd::findings_helper::blob_path(global, sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }

    fn decode_b64(data_b64: &str) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(data_b64)
            .unwrap()
    }

    #[tokio::test]
    async fn artifact_blob_is_streamed_as_ordered_chunks_then_done() {
        let global = tempdir().unwrap();
        let sha = "cd".repeat(32);
        let body: Vec<u8> = (0..ARTIFACT_CHUNK_BYTES + 5)
            .map(|i| (i % 251) as u8)
            .collect();
        store_blob(global.path(), &sha, &body);

        let mut stream = ArtifactPullStream::new(global.path(), "req1".into(), sha);
        let frames = collect_pull(&mut stream).await;
        assert_eq!(frames.len(), 3, "{} frames", frames.len());
        let mut got = Vec::new();
        for (i, f) in frames[..2].iter().enumerate() {
            match f {
                Frame::ArtifactChunk { req, seq, data_b64 } => {
                    assert_eq!(req, "req1");
                    assert_eq!(*seq, i as u64);
                    got.extend(decode_b64(data_b64));
                }
                other => panic!("expected a chunk, got {other:?}"),
            }
        }
        // A chunk is a whole ARTIFACT_CHUNK_BYTES, never a short read.
        assert_eq!(got.len(), body.len());
        assert_eq!(got, body);
        assert_eq!(
            frames[2],
            Frame::ArtifactPullDone {
                req: "req1".into(),
                error: None
            }
        );
        assert!(stream.is_done());
        assert!(stream.next_frame().await.is_none(), "nothing after Done");
    }

    #[tokio::test]
    async fn a_missing_blob_is_one_done_frame_with_an_error() {
        let global = tempdir().unwrap();
        let mut stream = ArtifactPullStream::new(global.path(), "req2".into(), "ef".repeat(32));
        let frames = collect_pull(&mut stream).await;
        assert!(
            matches!(
                &frames[..],
                [Frame::ArtifactPullDone { req, error: Some(e) }]
                    if req == "req2" && e.contains("not in this node's store")
            ),
            "{frames:?}"
        );
    }

    #[tokio::test]
    async fn a_malformed_sha_is_one_done_frame_with_an_error() {
        let global = tempdir().unwrap();
        let mut stream =
            ArtifactPullStream::new(global.path(), "req3".into(), "../../etc/passwd".into());
        let frames = collect_pull(&mut stream).await;
        assert!(
            matches!(
                &frames[..],
                [Frame::ArtifactPullDone { error: Some(e), .. }] if e.contains("not a sha256")
            ),
            "{frames:?}"
        );
    }

    #[tokio::test]
    async fn an_empty_blob_is_just_done() {
        let global = tempdir().unwrap();
        let sha = "0e".repeat(32);
        store_blob(global.path(), &sha, b"");
        let mut stream = ArtifactPullStream::new(global.path(), "req4".into(), sha);
        assert_eq!(
            collect_pull(&mut stream).await,
            vec![Frame::ArtifactPullDone {
                req: "req4".into(),
                error: None
            }]
        );
    }

    /// The pull runs inside the node's frame loop, so opening the blob must
    /// never park it: a FIFO swapped into the store is refused at once.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_fifo_in_the_store_is_refused_without_blocking() {
        let global = tempdir().unwrap();
        let sha = "f1".repeat(32);
        let p = crate::cmd::findings_helper::blob_path(global.path(), &sha).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let made = std::process::Command::new("mkfifo").arg(&p).status();
        if !matches!(made, Ok(s) if s.success()) {
            eprintln!("mkfifo unavailable; skipping");
            return;
        }
        let mut stream = ArtifactPullStream::new(global.path(), "req5".into(), sha);
        let frames =
            tokio::time::timeout(std::time::Duration::from_secs(5), collect_pull(&mut stream))
                .await
                .expect("opening a FIFO blocked the pull");
        assert!(
            matches!(
                &frames[..],
                [Frame::ArtifactPullDone { error: Some(_), .. }]
            ),
            "{frames:?}"
        );
    }

    type CpWs = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    /// The next text frame from a test CP's socket.
    async fn next_cp_frame(ws: &mut CpWs) -> Frame {
        loop {
            let msg = ws
                .next()
                .await
                .expect("node closed the tunnel")
                .expect("ws read");
            if let Message::Text(_) = msg {
                return parse_frame(&msg).unwrap();
            }
        }
    }

    fn cp_text(f: &Frame) -> Message {
        Message::Text(serde_json::to_string(f).unwrap())
    }

    /// Play a CP: accept the node's tunnel, check its Hello advertises
    /// artifact pulls, and welcome it.
    async fn accept_node(listener: tokio::net::TcpListener) -> CpWs {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let hello = next_cp_frame(&mut ws).await;
        assert!(
            matches!(&hello, Frame::Hello { capabilities, .. }
                if capabilities.iter().any(|c| c == rupu_cp::node::protocol::CAP_FINDINGS_ARTIFACT_PULL)),
            "{hello:?}"
        );
        ws.send(cp_text(&Frame::Welcome {
            capabilities: vec![],
        }))
        .await
        .unwrap();
        ws
    }

    /// Run a node's connection loop against `url`, rooted at `global`.
    fn spawn_node(
        url: String,
        global: &Path,
        exe: &Path,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        let (global, exe) = (global.to_path_buf(), exe.to_path_buf());
        tokio::spawn(
            async move { connect_and_run(&url, "tok", "node-pull-test", &exe, &global).await },
        )
    }

    /// A blob of `chunks` whole chunks in `global`'s store; returns its sha
    /// and bytes.
    fn store_chunks(global: &Path, sha: &str, chunks: usize) -> Vec<u8> {
        let body: Vec<u8> = (0..chunks * ARTIFACT_CHUNK_BYTES)
            .map(|i| (i % 253) as u8)
            .collect();
        store_blob(global, sha, &body);
        body
    }

    /// R11: while a node streams a multi-chunk pull it still answers inbound
    /// frames between chunks — not only after the whole blob has gone.
    #[tokio::test]
    async fn a_streaming_pull_does_not_hold_up_inbound_frames() {
        const CHUNKS: usize = 8;
        let global = tempdir().unwrap();
        let sha = "a1".repeat(32);
        let body = store_chunks(global.path(), &sha, CHUNKS);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/api/node/connect", listener.local_addr().unwrap());
        let cp = tokio::spawn(async move {
            let mut ws = accept_node(listener).await;
            ws.send(cp_text(&Frame::ArtifactPull {
                req: "r1".into(),
                sha256: sha,
            }))
            .await
            .unwrap();
            let mut frames = Vec::new();
            loop {
                let f = next_cp_frame(&mut ws).await;
                let done = matches!(f, Frame::ArtifactPullDone { .. });
                frames.push(f);
                // The node is mid-pull once its first chunk arrives: ask it
                // something then.
                if frames.len() == 1 {
                    ws.send(cp_text(&Frame::Ping {})).await.unwrap();
                }
                if done {
                    return frames;
                }
            }
        });
        let node = spawn_node(url, global.path(), Path::new("/nonexistent/rupu"));

        let frames = tokio::time::timeout(std::time::Duration::from_secs(60), cp)
            .await
            .expect("the pull never finished")
            .unwrap();
        node.abort();

        let pong_at = frames
            .iter()
            .position(|f| matches!(f, Frame::Pong {}))
            .expect("the Ping went unanswered until after the whole blob");
        assert!(
            pong_at + 2 < frames.len(),
            "Pong at {pong_at} of {} frames: the Ping waited for the blob",
            frames.len()
        );
        // The pull itself is intact: ordered chunks reassembling the blob.
        let (mut got, mut next_seq) = (Vec::new(), 0u64);
        for f in &frames {
            match f {
                Frame::ArtifactChunk { req, seq, data_b64 } => {
                    assert_eq!(req, "r1");
                    assert_eq!(*seq, next_seq);
                    next_seq += 1;
                    got.extend(decode_b64(data_b64));
                }
                Frame::Pong {} | Frame::ArtifactPullDone { error: None, .. } => {}
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert_eq!(next_seq, CHUNKS as u64);
        assert!(got == body, "reassembled blob differs");
    }

    /// Concurrent pulls share the loop round-robin: a small pull requested
    /// behind a large blob finishes first instead of waiting out the whole
    /// blob (and the CP's idle timeout for its first frame).
    #[tokio::test]
    async fn concurrent_pulls_are_served_round_robin() {
        const CHUNKS: usize = 8;
        let global = tempdir().unwrap();
        let (big, small) = ("c3".repeat(32), "d4".repeat(32));
        store_chunks(global.path(), &big, CHUNKS);
        store_blob(global.path(), &small, b"small blob");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/api/node/connect", listener.local_addr().unwrap());
        let cp = tokio::spawn(async move {
            let mut ws = accept_node(listener).await;
            for (req, sha256) in [("big", big), ("small", small)] {
                ws.send(cp_text(&Frame::ArtifactPull {
                    req: req.into(),
                    sha256,
                }))
                .await
                .unwrap();
            }
            let (mut frames, mut done) = (Vec::new(), 0);
            while done < 2 {
                let f = next_cp_frame(&mut ws).await;
                if matches!(f, Frame::ArtifactPullDone { .. }) {
                    done += 1;
                }
                frames.push(f);
            }
            frames
        });
        let node = spawn_node(url, global.path(), Path::new("/nonexistent/rupu"));

        let frames = tokio::time::timeout(std::time::Duration::from_secs(60), cp)
            .await
            .expect("the pulls never finished")
            .unwrap();
        node.abort();

        let done_at = |want: &str| {
            frames
                .iter()
                .position(
                    |f| matches!(f, Frame::ArtifactPullDone { req, error: None } if req == want),
                )
                .unwrap_or_else(|| panic!("no clean Done for {want}: {frames:?}"))
        };
        assert!(
            done_at("small") < done_at("big"),
            "the small pull waited behind the big blob"
        );
        let small_bytes: Vec<u8> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::ArtifactChunk { req, data_b64, .. } if req == "small" => {
                    Some(decode_b64(data_b64))
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(small_bytes, b"small blob");
        let big_seqs: Vec<u64> = frames
            .iter()
            .filter_map(|f| match f {
                Frame::ArtifactChunk { req, seq, .. } if req == "big" => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(big_seqs, (0..CHUNKS as u64).collect::<Vec<_>>());
    }

    // ------------------------------------------------------------------
    // Bucket worker marker
    // ------------------------------------------------------------------

    /// A bucket worker advertises what holds over a bucket — never the
    /// tunnel-only artifact pull, which it has no frames to answer.
    #[test]
    fn the_bucket_worker_does_not_advertise_the_tunnel_artifact_pull() {
        let info = bucket_worker_info("worker-1");
        assert_eq!(info.worker_id, "worker-1");
        assert_eq!(info.rupu_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            info.capabilities,
            vec![rupu_cp::node::protocol::CAP_AGENT_FINDINGS_PROFILE.to_string()]
        );
    }

    /// R11's other half: an active run's files keep draining while a pull
    /// streams (at the poll cadence, not once per chunk).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_streaming_pull_still_drains_active_runs() {
        // More chunks than loopback socket buffers hold, so the node is still
        // mid-pull when the CP resumes reading after its pause.
        const CHUNKS: usize = 16;
        // The node tracks a run by its files, not its child, so any
        // spawnable no-op executable stands in for `rupu`.
        let Some(exe) = ["/usr/bin/true", "/bin/true"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.exists())
        else {
            eprintln!("no `true` executable; skipping");
            return;
        };
        let global = tempdir().unwrap();
        let sha = "b2".repeat(32);
        store_chunks(global.path(), &sha, CHUNKS);
        let events = global
            .path()
            .join("runs")
            .join("run_drain")
            .join("events.jsonl");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/api/node/connect", listener.local_addr().unwrap());
        let cp = tokio::spawn(async move {
            let mut ws = accept_node(listener).await;
            ws.send(cp_text(&Frame::Run {
                run_id: "run_drain".into(),
                spec: RunSpec {
                    kind: RunSpecKind::Workflow,
                    name: "wf".into(),
                    inputs: BTreeMap::new(),
                    prompt: None,
                    mode: None,
                    target: None,
                    findings_profile: None,
                },
            }))
            .await
            .unwrap();
            ws.send(cp_text(&Frame::ArtifactPull {
                req: "r2".into(),
                sha256: sha,
            }))
            .await
            .unwrap();
            let mut frames = vec![next_cp_frame(&mut ws).await];
            assert!(
                matches!(frames[0], Frame::ArtifactChunk { seq: 0, .. }),
                "{:?}",
                frames[0]
            );
            // Mid-pull, the run writes an event; stop reading for longer than
            // the poll interval, then take everything up to the pull's end.
            std::fs::create_dir_all(events.parent().unwrap()).unwrap();
            std::fs::write(&events, "{\"e\":1}\n").unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            loop {
                let f = next_cp_frame(&mut ws).await;
                let done = matches!(f, Frame::ArtifactPullDone { .. });
                frames.push(f);
                if done {
                    return frames;
                }
            }
        });
        let node = spawn_node(url, global.path(), exe);

        let frames = tokio::time::timeout(std::time::Duration::from_secs(60), cp)
            .await
            .expect("the pull never finished")
            .unwrap();
        node.abort();

        let event_at = frames
            .iter()
            .position(|f| {
                matches!(f, Frame::Artifact { run_id, file: ArtifactFile::Events, line }
                    if run_id == "run_drain" && line == "{\"e\":1}")
            })
            .expect("the run's event was not drained during the pull");
        let last_chunk_at = frames
            .iter()
            .rposition(|f| matches!(f, Frame::ArtifactChunk { .. }))
            .unwrap();
        assert!(
            event_at < last_chunk_at,
            "event at {event_at}, last chunk at {last_chunk_at}: drained only after the blob"
        );
    }
}
