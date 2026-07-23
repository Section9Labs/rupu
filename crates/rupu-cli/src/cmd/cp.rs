//! `rupu cp` — control-plane HTTP server subcommand.

use crate::paths;
use clap::Subcommand;
use rupu_cp::host::bucket::{ObjectStoreBucket, poll_bucket_run};
use rupu_orchestrator::runs::RunStore;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Start the control-plane HTTP server.
    Serve {
        /// Address to bind. Defaults to 127.0.0.1:7878.
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: SocketAddr,
        /// Optional bearer token. If set, `/api/*` requires
        /// `Authorization: Bearer <token>` (the web UI and `/healthz` remain
        /// open on localhost).
        #[arg(long)]
        token: Option<String>,
        /// Do not open the served URL in a browser on startup. By default the
        /// URL is opened when running interactively (a terminal); the URL is
        /// always printed regardless.
        #[arg(long)]
        no_open: bool,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Serve {
            bind,
            token,
            no_open,
        } => {
            let global_dir = match paths::global_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    return ExitCode::FAILURE;
                }
            };
            // `[cp]` runtime settings gate the two background-tick loops
            // below (autoflow reconcile / cron tick). A missing/malformed
            // config file just falls back to `CpConfig::default()` — both
            // loops enabled, 60s cadence — same as an absent `[cp]` section.
            let cp_runtime_cfg = {
                let global_cfg_path = global_dir.join("config.toml");
                rupu_config::layer_files(Some(&global_cfg_path), None)
                    .unwrap_or_default()
                    .cp
            };
            // Spawn the background resume worker. It builds the SAME
            // RunStore the CP's AppState does (`<global_dir>/runs`), so it
            // claims/approves/resumes runs the web UI marked for resume.
            let store = Arc::new(RunStore::new(global_dir.join("runs")));
            let worker_id = format!("cp-serve-{}", std::process::id());
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            tracing::info!(
                worker_id = %worker_id,
                "resume worker active: finishing web-approved gates"
            );
            let worker_handle = tokio::spawn(run_resume_worker(
                Arc::clone(&store),
                worker_id,
                rupu_workspace::HostStore { root: global_dir.join("hosts") },
                shutdown_rx,
            ));
            let poller_handle = tokio::spawn(run_bucket_poller(
                Arc::clone(&store),
                rupu_workspace::HostStore { root: global_dir.join("hosts") },
                shutdown_tx.subscribe(),
            ));

            // Autoflow reconcile loop (T6, dogfood-autoflows): periodically
            // calls the SAME entrypoint `rupu autoflow tick` uses
            // (`autoflow_runtime::tick_with_resolver`, covering both issue
            // and PR entity autoflows) so `cp serve` fires autoflows
            // without a separate `rupu autoflow serve` process or external
            // scheduler. Gated by `[cp].autoflow_reconcile_enabled`
            // (default: on); cadence from `[cp].autoflow_reconcile_interval_secs`
            // (default: 60s).
            let autoflow_resolver: Arc<dyn rupu_auth::CredentialResolver> =
                Arc::new(rupu_auth::KeychainResolver::new());
            let autoflow_reconcile_handle = tokio::spawn(run_periodic_tick(
                "autoflow-reconcile",
                cp_runtime_cfg.autoflow_reconcile_enabled,
                Duration::from_secs(cp_runtime_cfg.autoflow_reconcile_interval_secs.max(1)),
                shutdown_tx.subscribe(),
                move || {
                    let resolver = Arc::clone(&autoflow_resolver);
                    async move {
                        if let Err(e) =
                            crate::cmd::autoflow_runtime::tick_with_resolver(resolver).await
                        {
                            tracing::warn!(error = %e, "cp serve: autoflow reconcile tick failed");
                        }
                    }
                },
            ));

            // Cron / event-trigger tick loop (T6, dogfood-autoflows):
            // periodically calls the SAME entrypoint `rupu cron tick` uses
            // (`crate::cmd::cron::tick`, covering both cron-scheduled and
            // polled-event workflow fires) so nightly/event-triggered
            // workflows fire without an external `cron` entry. Gated by
            // `[cp].cron_tick_enabled` (default: on); cadence from
            // `[cp].cron_tick_interval_secs` (default: 60s).
            let cron_tick_handle = tokio::spawn(run_periodic_tick(
                "cron-tick",
                cp_runtime_cfg.cron_tick_enabled,
                Duration::from_secs(cp_runtime_cfg.cron_tick_interval_secs.max(1)),
                shutdown_tx.subscribe(),
                || async {
                    if let Err(e) = crate::cmd::cron::tick(false, false, false).await {
                        tracing::warn!(error = %e, "cp serve: cron tick failed");
                    }
                },
            ));

            // Adapter for rupu-cp's RunLauncher port: spawns detached
            // `rupu workflow run …` children using this same binary.
            let exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: cannot resolve current executable for run launcher: {e}");
                    let _ = shutdown_tx.send(true);
                    let _ = worker_handle.await;
                    return ExitCode::FAILURE;
                }
            };

            // Gate sweep loop (Plan 4): periodically enforces gate
            // `on_timeout` routing (approve/reject/fail) for overdue
            // AwaitingApproval runs, runs the `on_reject` cleanup chain for
            // web-initiated timeout-rejects, and reaps orphaned local runs
            // whose runner process died — so a timed-out gate or a dead
            // runner never wedges Live Events. Gated by
            // `[cp].gate_sweep_enabled` (default: on); cadence from
            // `[cp].gate_sweep_interval_secs` (default: 60s).
            let gate_sweep_store = Arc::clone(&store);
            let gate_sweep_hosts = rupu_workspace::HostStore { root: global_dir.join("hosts") };
            let gate_sweep_exe = exe.clone();
            let gate_sweep_handle = tokio::spawn(run_periodic_tick(
                "gate-sweep",
                cp_runtime_cfg.gate_sweep_enabled,
                Duration::from_secs(cp_runtime_cfg.gate_sweep_interval_secs.max(1)),
                shutdown_tx.subscribe(),
                move || {
                    let store = Arc::clone(&gate_sweep_store);
                    let hosts = rupu_workspace::HostStore { root: gate_sweep_hosts.root.clone() };
                    let exe = gate_sweep_exe.clone();
                    async move {
                        run_gate_sweep(store, hosts, exe).await;
                    }
                },
            ));
            let launcher: Arc<dyn rupu_cp::launcher::RunLauncher> =
                Arc::new(crate::cp_launcher::SubprocessLauncher { exe: exe.clone() });

            // Adapter for rupu-cp's AgentLauncher port: spawns detached
            // `rupu run <agent> …` children using this same binary.
            let agent_launcher: Option<Arc<dyn rupu_cp::agent_launcher::AgentLauncher>> = Some(
                Arc::new(crate::cp_agent_launcher::SubprocessAgentLauncher { exe: exe.clone() }),
            );

            // Adapter for rupu-cp's SessionSender port: shells
            // `rupu session send <id> "<prompt>" --detach` using this same
            // binary, reusing the launcher's resolved exe.
            let session_sender: Arc<dyn rupu_cp::session_sender::SessionSender> =
                Arc::new(crate::cp_session_sender::SubprocessSessionSender { exe: exe.clone() });

            // Adapter for rupu-cp's SessionMutator port: shells
            // `rupu session archive|restore|delete <id>` using this same binary.
            let session_mutator: Option<Arc<dyn rupu_cp::session_mutator::SessionMutator>> = Some(
                Arc::new(crate::cp_session_mutator::SubprocessSessionMutator { exe: exe.clone() }),
            );

            // Adapter for rupu-cp's SessionStarter port: shells
            // `rupu session start <agent> … --detach` using this same binary.
            let session_starter: Option<Arc<dyn rupu_cp::session_starter::SessionStarter>> = Some(
                Arc::new(crate::cp_session_starter::SubprocessSessionStarter { exe }),
            );

            // Adapter for rupu-cp's DefinitionGenerator port: calls the
            // orchestrator generation core with the real resolver.
            let generator: Option<Arc<dyn rupu_cp::definition_generator::DefinitionGenerator>> =
                Some(Arc::new(
                    crate::cp_definition_generator::RuntimeDefinitionGenerator {
                        global_dir: global_dir.clone(),
                    },
                ));

            // Repo lister for the web Run target picker.
            let repos: Option<Arc<dyn rupu_cp::repos::RepoLister>> = {
                let resolver = rupu_auth::KeychainResolver::new();
                let global_cfg = global_dir.join("config.toml");
                let cfg = rupu_config::layer_files(Some(&global_cfg), None).unwrap_or_default();
                let registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);
                Some(Arc::new(crate::cp_repos::CpRepoLister { registry }))
            };

            let serve_result = rupu_cp::serve(rupu_cp::ServeOpts {
                bind,
                token,
                global_dir,
                open_browser: !no_open,
                launcher: Some(launcher),
                session_sender: Some(session_sender),
                repos,
                agent_launcher,
                session_starter,
                generator,
                session_mutator,
            })
            .await;

            // Signal every background loop to stop and wait for them to drain.
            let _ = shutdown_tx.send(true);
            let _ = worker_handle.await;
            let _ = poller_handle.await;
            let _ = autoflow_reconcile_handle.await;
            let _ = cron_tick_handle.await;
            let _ = gate_sweep_handle.await;

            serve_result
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Generic periodic-tick loop shared by the autoflow-reconcile and
/// cron-tick background loops (T6, dogfood-autoflows). Mirrors the
/// sleep-vs-shutdown `tokio::select!` shape of [`run_bucket_poller`] /
/// [`run_resume_worker`], but takes the per-iteration unit of work as an
/// injected closure so the two concrete loops below can share one tested
/// implementation instead of hand-rolling the same interval/shutdown
/// plumbing twice.
///
/// When `enabled` is `false` the loop never starts (`tick` is never
/// called, not even once) — this is the `[cp]` config flag's off switch,
/// not a silent no-op: it's the documented way to disable a loop, logged
/// once at startup.
async fn run_periodic_tick<F, Fut>(
    loop_name: &'static str,
    enabled: bool,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
    mut tick: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if !enabled {
        tracing::info!(loop_name = %loop_name, "background loop disabled via [cp] config");
        return;
    }
    tracing::info!(
        loop_name = %loop_name,
        interval_secs = interval.as_secs(),
        "background loop active"
    );
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!(loop_name = %loop_name, "background loop shutting down");
                    break;
                }
                continue;
            }
        }
        tick().await;
    }
}

/// The IO action the gate sweep should take for a single run, decided by
/// the pure [`sweep_decision`] classifier and mapped to store calls by
/// [`run_gate_sweep`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepAction {
    /// Do nothing beyond the mandatory `expire_if_overdue` call the IO
    /// always makes for an `AwaitingApproval` run (which finalizes a
    /// `Fail`/default timeout, or no-ops when the run isn't overdue).
    Skip,
    /// The gate timed out with `on_timeout: approve`: `expire_if_overdue`
    /// leaves the record `AwaitingApproval` (untouched) and the IO spawns a
    /// detached `rupu workflow approve <id>` to auto-approve + resume.
    ExpireApprove,
    /// The gate timed out with `on_timeout: reject`: `expire_if_overdue`
    /// finalizes the run `Rejected` and the IO then runs the gate's
    /// `on_reject` cleanup chain (`build_reject_cleanup_opts` +
    /// `run_reject_cleanup`).
    ExpireThenCleanupReject,
    /// A `Running`/`Pending` run whose recorded local runner pid is dead:
    /// the IO calls `reap_if_orphaned` to finalize it `Failed`.
    Reap,
}

/// Pure per-run classifier for the cp-serve gate sweep (Plan 4). Split out
/// from the IO tick body [`run_gate_sweep`] so its truth table is unit
/// testable without a live store or daemon.
///
/// Contract split: for an `AwaitingApproval` run the IO layer ALWAYS calls
/// `expire_if_overdue` first (safe: it no-ops when the run isn't overdue,
/// and finalizes the `Fail`/default case on its own). This fn only
/// classifies the POST-expire action — hence `Fail`/`None`/not-expired all
/// map to `Skip` (the expire call already did the work). `is_remote` short
/// circuits to `Skip` for every status: a run owned by a remote host is
/// driven by that host's transport/sweep, and a dead *local* pid check is
/// meaningless for it (mirrors the resume worker's `remote_workers` guard).
fn sweep_decision(
    status: rupu_orchestrator::RunStatus,
    on_timeout: Option<rupu_orchestrator::TimeoutAction>,
    expired: bool,
    pid_alive: Option<bool>,
    is_remote: bool,
) -> SweepAction {
    use rupu_orchestrator::{RunStatus, TimeoutAction};
    if is_remote {
        return SweepAction::Skip;
    }
    match status {
        RunStatus::AwaitingApproval => {
            if !expired {
                return SweepAction::Skip;
            }
            match on_timeout {
                Some(TimeoutAction::Reject) => SweepAction::ExpireThenCleanupReject,
                Some(TimeoutAction::Approve) => SweepAction::ExpireApprove,
                // Fail is finalized inside the mandatory expire call; None
                // collapses to the same default. No extra post-action.
                Some(TimeoutAction::Fail) | None => SweepAction::Skip,
            }
        }
        RunStatus::Running | RunStatus::Pending => match pid_alive {
            Some(false) => SweepAction::Reap,
            // Alive, or unknown (no recorded pid): leave it be.
            _ => SweepAction::Skip,
        },
        _ => SweepAction::Skip,
    }
}

/// How often the bucket poller wakes to check for new result objects.
const BUCKET_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Background worker that polls each bucket host for completed result objects
/// and mirrors them into the central [`RunStore`] via [`rupu_cp::node::NodeMirror`].
///
/// This is the counterpart to the tunnel read-pump: instead of streaming events
/// over a WebSocket, the node writes artifacts into the shared object-store bucket
/// and this poller reads them back on a fixed interval.
///
/// A per-`(host_id, run_id)` [`HashSet<String>`] accumulates the keys that have
/// already been mirrored, so re-polling never double-appends.
async fn run_bucket_poller(
    store: Arc<RunStore>,
    hosts: rupu_workspace::HostStore,
    mut shutdown: watch::Receiver<bool>,
) {
    // Outer key: "<host_id>\x00<run_id>", value: set of consumed bucket keys.
    let mut consumed: HashMap<String, HashSet<String>> = HashMap::new();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(BUCKET_POLL_INTERVAL) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("bucket poller shutting down");
                    break;
                }
                continue;
            }
        }

        let bucket_hosts: Vec<rupu_workspace::Host> = hosts
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| matches!(h.transport, rupu_workspace::HostTransport::Bucket { .. }))
            .collect();

        for host in &bucket_hosts {
            let (url, prefix) = match &host.transport {
                rupu_workspace::HostTransport::Bucket { url, prefix } => {
                    (url.clone(), prefix.clone())
                }
                _ => continue,
            };

            let bucket = match ObjectStoreBucket::from_url(&url, prefix.as_deref()) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        host_id = %host.id,
                        error = %e,
                        "bucket poller: failed to build bucket from url"
                    );
                    continue;
                }
            };

            let mirror = rupu_cp::node::NodeMirror::new(Arc::clone(&store));

            // Find in-flight runs attributed to this bucket host.
            let inflight: Vec<String> = match store.list() {
                Ok(runs) => runs
                    .into_iter()
                    .filter(|r| {
                        r.worker_id.as_deref() == Some(host.id.as_str()) && !r.status.is_terminal()
                    })
                    .map(|r| r.id)
                    .collect(),
                Err(e) => {
                    tracing::warn!(
                        host_id = %host.id,
                        error = %e,
                        "bucket poller: RunStore::list failed"
                    );
                    continue;
                }
            };

            for run_id in inflight {
                // Compound map key avoids collisions across hosts.
                let map_key = format!("{}\x00{}", host.id, run_id);

                let poll_result = {
                    let consumed_set = consumed.entry(map_key.clone()).or_default();
                    poll_bucket_run(&bucket, &mirror, &host.id, &run_id, consumed_set).await
                };

                match poll_result {
                    Ok(true) => {
                        tracing::info!(
                            host_id = %host.id,
                            run_id = %run_id,
                            "bucket poller: run finished, removing from tracking"
                        );
                        consumed.remove(&map_key);
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(
                            host_id = %host.id,
                            run_id = %run_id,
                            error = %e,
                            "bucket poller: poll_bucket_run failed"
                        );
                    }
                }
            }
        }
    }
}

/// Background worker that finishes web-approved workflow gates.
///
/// When an operator approves a gate in the web UI, the run gets a
/// `resume_requested_at` marker but stays `AwaitingApproval` (the web
/// process has no execution runtime). This worker polls for marked runs,
/// claims each via a lease, then spawns a detached
/// `rupu workflow approve <run_id> [--mode <m>]` child process which does the
/// `store.approve` + in-process resume in ITS OWN process. Running the resume
/// as a separate, killable process means Cancel can stop it and a resume
/// crash can't take down `cp serve`. The marker and claim are cleared after a
/// successful spawn (the child now owns the run), and also on spawn failure so
/// a poisoned run is not retried forever.
async fn run_resume_worker(
    store: Arc<RunStore>,
    worker_id: String,
    hosts: rupu_workspace::HostStore,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(4)) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!(worker_id = %worker_id, "resume worker shutting down");
                    break;
                }
                continue;
            }
        }

        let now = chrono::Utc::now();
        let pending = match store.list_pending_resume(now) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "resume worker: list_pending_resume failed");
                continue;
            }
        };

        // Defense-in-depth: never resume a run that belongs to a REMOTE host
        // (tunnel, ssh, or bucket); its real run lives on that host and is resumed via
        // the transport, not by this local worker. (Remote runs also never carry
        // the resume_requested_at marker, so this is belt-and-suspenders.)
        // KEY ASYMMETRY: a Tunnel run's worker_id is the node_id; an SSH run's
        // worker_id is the host record id (host_<ULID>); a Bucket run's worker_id
        // is also the host record id.
        let remote_workers: std::collections::HashSet<String> = hosts
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|h| match h.transport {
                rupu_workspace::HostTransport::Tunnel { node_id } => Some(node_id),
                rupu_workspace::HostTransport::Ssh { .. } => Some(h.id),
                rupu_workspace::HostTransport::Bucket { .. } => Some(h.id),
                _ => None,
            })
            .collect();

        for run in pending {
            if let Some(w) = run.worker_id.as_deref() {
                if remote_workers.contains(w) {
                    tracing::debug!(run_id = %run.id, worker = %w,
                        "resume worker: skipping remote-host run");
                    continue;
                }
            }
            let claimed = match store.claim_resume(&run.id, &worker_id, now) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(run_id = %run.id, error = %e, "resume worker: claim failed");
                    continue;
                }
            };
            if !claimed {
                // Another worker holds a live lease.
                continue;
            }
            tracing::info!(run_id = %run.id, worker_id = %worker_id, "resume worker: claimed run");

            // Spawn the resuming subprocess off-thread so claiming the next
            // run doesn't block on process creation. Move owned data in.
            // Which subcommand to spawn depends on the run's CURRENT status
            // (captured from `list_pending_resume`, still fresh — the claim
            // lease above prevents a concurrent worker from racing it):
            // `AwaitingApproval` → `workflow approve` (approval-gate resume,
            // unchanged); `Paused` → `workflow resume` (cooperative-pause
            // resume, T4 — that command now also accepts `Paused` and reads
            // the persisted mid-step seed via `RunStore::read_paused_seed`).
            let subcommand = match run.status {
                rupu_orchestrator::RunStatus::Paused => "resume",
                _ => "approve",
            };
            let store = Arc::clone(&store);
            let run_id = run.id.clone();
            tokio::spawn(async move {
                let now2 = chrono::Utc::now();
                // Capture the requested resume mode while the marker is still
                // present, then hand the run off to a detached
                // `rupu workflow <subcommand> <run_id> [--mode <m>]` child.
                // The child does `store.approve`/the checkpoint-resume flip +
                // the in-process resume in ITS OWN process, so the resumed
                // run is independently killable (Cancel) and a resume crash
                // can't take down `cp serve`. The web marker leaves the run
                // AwaitingApproval/Paused, so the child's precondition holds.
                let mode = store.load(&run_id).ok().and_then(|r| r.resume_mode.clone());

                let exe = match std::env::current_exe() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!(run_id = %run_id, error = %e, "resume worker: cannot resolve current exe; clearing marker");
                        if let Err(ce) = store.clear_resume(&run_id, now2) {
                            tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                        }
                        return;
                    }
                };

                let mut argv: Vec<&str> = vec!["workflow", subcommand, &run_id];
                if let Some(m) = mode.as_deref() {
                    argv.push("--mode");
                    argv.push(m);
                }

                match std::process::Command::new(&exe).args(&argv).spawn() {
                    Ok(_child) => {
                        // Detached: do NOT wait. The child now owns the run;
                        // clear the marker so we don't re-claim it.
                        tracing::info!(run_id = %run_id, subcommand, "spawned workflow subprocess to resume");
                        if let Err(ce) = store.clear_resume(&run_id, now2) {
                            tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                        } else {
                            tracing::info!(run_id = %run_id, "resume worker: cleared resume marker");
                        }
                    }
                    Err(e) => {
                        // Don't retry a poisoned spawn forever; clear marker.
                        tracing::error!(run_id = %run_id, subcommand, error = %e, "resume worker: spawn workflow subprocess failed; clearing marker");
                        if let Err(ce) = store.clear_resume(&run_id, now2) {
                            tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                        }
                    }
                }
            });
        }
    }
}

/// One pass of the cp-serve gate sweep (Plan 4). For every run in the
/// store it classifies the needed action via [`sweep_decision`] and maps it
/// to store IO:
///
/// * `AwaitingApproval` (non-remote): resolve the gate's `on_timeout`, call
///   `expire_if_overdue` (finalizes the `Fail`/default timeout, or no-ops
///   when not overdue), then — per the decision — spawn a detached
///   `rupu workflow approve <id>` (`on_timeout: approve`) or run the
///   `on_reject` cleanup chain (`on_timeout: reject`).
/// * `Running`/`Pending` (non-remote) with a dead recorded runner pid:
///   `reap_if_orphaned` finalizes it `Failed`.
///
/// Best-effort/fail-closed: every skip and every per-run error is logged and
/// swallowed (`continue`) — one poisoned run never aborts the sweep. Runs
/// owned by a remote host are skipped entirely (mirrors the resume worker's
/// `remote_workers` guard); their real runner lives on another host.
async fn run_gate_sweep(
    store: Arc<RunStore>,
    hosts: rupu_workspace::HostStore,
    exe: std::path::PathBuf,
) {
    let now = chrono::Utc::now();

    // Same remote-owner guard the resume worker uses: a run whose worker_id
    // names a remote host (tunnel node_id / ssh host id / bucket host id) is
    // driven by that host, not by this local sweep.
    let remote_workers: HashSet<String> = hosts
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|h| match h.transport {
            rupu_workspace::HostTransport::Tunnel { node_id } => Some(node_id),
            rupu_workspace::HostTransport::Ssh { .. } => Some(h.id),
            rupu_workspace::HostTransport::Bucket { .. } => Some(h.id),
            _ => None,
        })
        .collect();

    let runs = match store.list() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "gate sweep: RunStore::list failed");
            return;
        }
    };

    for mut rec in runs {
        let run_id = rec.id.clone();
        let is_remote = rec
            .worker_id
            .as_deref()
            .map(|w| remote_workers.contains(w))
            .unwrap_or(false);

        match rec.status {
            rupu_orchestrator::RunStatus::AwaitingApproval => {
                if is_remote {
                    tracing::debug!(run_id = %run_id, "gate sweep: skipping remote-host awaiting run");
                    continue;
                }
                let expired = rec.expires_at.is_some_and(|exp| now > exp);
                // Cheap short-circuit: a gate that isn't overdue yet needs no
                // snapshot read / expire call (the decision would be `Skip`).
                if !expired {
                    continue;
                }
                let on_timeout = store.resolve_gate_timeout(&rec);
                let decision = sweep_decision(rec.status, on_timeout, expired, None, is_remote);
                // Capture the gate step id BEFORE expire_if_overdue clears it
                // (the reject branch nulls awaiting_step_id).
                let gate_step_id = rec.awaiting_step_id.clone();
                let expire_res = store.expire_if_overdue(&mut rec, now, on_timeout);
                let outcome = match expire_res {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::warn!(run_id = %run_id, error = %e, "gate sweep: expire_if_overdue failed");
                        continue;
                    }
                };
                match decision {
                    SweepAction::Skip => {
                        // Not overdue, or Fail/default already finalized inside
                        // the expire call.
                        if matches!(outcome, Some(rupu_orchestrator::TimeoutAction::Fail)) {
                            tracing::info!(run_id = %run_id, "gate sweep: gate timed out → run failed");
                        }
                    }
                    SweepAction::ExpireApprove => {
                        // expire left the record AwaitingApproval; hand off to a
                        // detached `rupu workflow approve <id>`, which re-resolves
                        // the on_timeout: approve policy and does the approve +
                        // in-process resume in its own killable process (mirrors
                        // the resume worker's detached spawn).
                        let mut argv: Vec<&str> = vec!["workflow", "approve", &run_id];
                        if let Some(m) = rec.resume_mode.as_deref() {
                            argv.push("--mode");
                            argv.push(m);
                        }
                        match std::process::Command::new(&exe).args(&argv).spawn() {
                            Ok(_child) => {
                                tracing::info!(run_id = %run_id, "gate sweep: on_timeout=approve → spawned detached workflow approve");
                            }
                            Err(e) => {
                                tracing::error!(run_id = %run_id, error = %e, "gate sweep: failed to spawn workflow approve for on_timeout=approve");
                            }
                        }
                    }
                    SweepAction::ExpireThenCleanupReject => {
                        // expire finalized the run Rejected. Run the same
                        // on_reject cleanup chain the CLI reject path runs.
                        if !matches!(outcome, Some(rupu_orchestrator::TimeoutAction::Reject)) {
                            tracing::warn!(run_id = %run_id, "gate sweep: expected reject outcome for on_timeout=reject but got {outcome:?}; skipping cleanup");
                            continue;
                        }
                        let step_id = gate_step_id.unwrap_or_default();
                        let reason = rec
                            .error_message
                            .clone()
                            .unwrap_or_else(|| "gate timed out (on_timeout: reject)".to_string());
                        tracing::info!(run_id = %run_id, step_id = %step_id, "gate sweep: on_timeout=reject → run auto-rejected; running on_reject cleanup");
                        if crate::cmd::workflow::cheap_on_reject_chain_len(&store, &run_id, &step_id)
                            != Some(0)
                        {
                            match crate::resume::build_reject_cleanup_opts(
                                &store,
                                &run_id,
                                &step_id,
                                &reason,
                                rec.resume_mode.as_deref(),
                            )
                            .await
                            {
                                Ok((opts, chain_len)) => {
                                    match rupu_orchestrator::runner::run_reject_cleanup(
                                        opts, &step_id, &reason, "timeout",
                                    )
                                    .await
                                    {
                                        Ok(()) => {
                                            tracing::info!(run_id = %run_id, chain_len, "gate sweep: on_reject cleanup chain executed");
                                        }
                                        Err(e) => {
                                            tracing::warn!(run_id = %run_id, error = %e, "gate sweep: on_reject cleanup chain errored (run is already rejected)");
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(run_id = %run_id, error = %e, "gate sweep: could not build on_reject cleanup opts (run is already rejected)");
                                }
                            }
                        }
                    }
                    SweepAction::Reap => {
                        tracing::warn!(run_id = %run_id, "gate sweep: unexpected Reap decision for AwaitingApproval run; skipping");
                    }
                }
            }
            rupu_orchestrator::RunStatus::Running | rupu_orchestrator::RunStatus::Pending => {
                let pid_alive = rec.runner_pid.map(rupu_orchestrator::runs::pid_is_running);
                let decision = sweep_decision(rec.status, None, false, pid_alive, is_remote);
                match decision {
                    SweepAction::Reap => match store.reap_if_orphaned(&mut rec, now) {
                        Ok(true) => {
                            tracing::warn!(run_id = %run_id, "gate sweep: reaped orphaned run (runner pid dead)");
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(run_id = %run_id, error = %e, "gate sweep: reap_if_orphaned failed");
                        }
                    },
                    _ => {
                        if is_remote {
                            tracing::debug!(run_id = %run_id, "gate sweep: skipping remote-host in-flight run");
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{run_periodic_tick, sweep_decision, SweepAction};
    use rupu_orchestrator::{RunStatus, TimeoutAction};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// Plan 4 gate sweep: the pure classifier's full truth table. `expired`
    /// only matters for `AwaitingApproval`; `pid_alive` only for
    /// `Running`/`Pending`; `is_remote` short-circuits everything to `Skip`.
    #[test]
    fn sweep_decision_truth_table() {
        // AwaitingApproval + expired: routed by on_timeout.
        assert_eq!(
            sweep_decision(
                RunStatus::AwaitingApproval,
                Some(TimeoutAction::Reject),
                true,
                None,
                false
            ),
            SweepAction::ExpireThenCleanupReject
        );
        assert_eq!(
            sweep_decision(
                RunStatus::AwaitingApproval,
                Some(TimeoutAction::Approve),
                true,
                None,
                false
            ),
            SweepAction::ExpireApprove
        );
        assert_eq!(
            sweep_decision(
                RunStatus::AwaitingApproval,
                Some(TimeoutAction::Fail),
                true,
                None,
                false
            ),
            SweepAction::Skip
        );
        assert_eq!(
            sweep_decision(RunStatus::AwaitingApproval, None, true, None, false),
            SweepAction::Skip
        );
        // AwaitingApproval but not expired → Skip regardless of policy.
        assert_eq!(
            sweep_decision(
                RunStatus::AwaitingApproval,
                Some(TimeoutAction::Reject),
                false,
                None,
                false
            ),
            SweepAction::Skip
        );
        // Remote-owned awaiting run: never touched by the local sweep.
        assert_eq!(
            sweep_decision(
                RunStatus::AwaitingApproval,
                Some(TimeoutAction::Reject),
                true,
                None,
                true
            ),
            SweepAction::Skip
        );

        // Running/Pending: reap only a dead LOCAL pid.
        assert_eq!(
            sweep_decision(RunStatus::Running, None, false, Some(false), false),
            SweepAction::Reap
        );
        assert_eq!(
            sweep_decision(RunStatus::Pending, None, false, Some(false), false),
            SweepAction::Reap
        );
        // Dead pid but remote-owned → Skip (local pid check is meaningless).
        assert_eq!(
            sweep_decision(RunStatus::Running, None, false, Some(false), true),
            SweepAction::Skip
        );
        // Alive pid → Skip.
        assert_eq!(
            sweep_decision(RunStatus::Running, None, false, Some(true), false),
            SweepAction::Skip
        );
        // No recorded pid (unknown liveness) → Skip.
        assert_eq!(
            sweep_decision(RunStatus::Running, None, false, None, false),
            SweepAction::Skip
        );

        // Terminal / paused / other → always Skip.
        for status in [
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Rejected,
            RunStatus::Cancelled,
            RunStatus::Paused,
        ] {
            assert_eq!(
                sweep_decision(status, Some(TimeoutAction::Reject), true, Some(false), false),
                SweepAction::Skip
            );
        }
    }

    /// T6 (dogfood-autoflows): the shared loop body must invoke the
    /// injected tick fn once per interval, N times over N intervals. No
    /// real autoflow reconciler or cron tick runs here — the tick fn is a
    /// plain counter that flips the SAME `watch` channel the loop is
    /// already select!-ing on once it's been called `N` times, so the
    /// test is deterministic (no wall-clock race): the loop's next
    /// `shutdown.changed()` observes the flip immediately and exits.
    #[tokio::test]
    async fn run_periodic_tick_invokes_injected_fn_once_per_interval_until_shutdown() {
        const N: usize = 3;
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_tick = Arc::clone(&counter);
        let shutdown_tx_for_tick = shutdown_tx.clone();

        run_periodic_tick(
            "test-loop",
            true,
            Duration::from_millis(1),
            shutdown_rx,
            move || {
                let counter = Arc::clone(&counter_for_tick);
                let shutdown_tx = shutdown_tx_for_tick.clone();
                async move {
                    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    if n >= N {
                        let _ = shutdown_tx.send(true);
                    }
                }
            },
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), N);
    }

    /// T6: `enabled: false` must be a hard off switch — the injected tick
    /// fn never runs, not even once, and the loop returns immediately
    /// instead of hanging.
    #[tokio::test]
    async fn run_periodic_tick_disabled_never_invokes_injected_fn() {
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_tick = Arc::clone(&counter);

        run_periodic_tick(
            "test-loop-disabled",
            false,
            Duration::from_millis(1),
            shutdown_rx,
            move || {
                let counter = Arc::clone(&counter_for_tick);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
