//! `rupu cp` — control-plane HTTP server subcommand.

use crate::paths;
use clap::Subcommand;
use rupu_cp::host::bucket::{poll_bucket_run, ObjectStoreBucket};
use rupu_orchestrator::runs::RunStore;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Start the control-plane HTTP server.
    ///
    /// Also runs three in-process background loops on a timer, not just the
    /// HTTP server: the autoflow reconciler, the cron/event-trigger tick, and
    /// the gate sweep (enforces gate `on_timeout` routing and reaps orphaned
    /// runs with a dead `runner_pid`). Each loop is gated by a `[cp]` config
    /// key and defaults to enabled at a 60s interval: `autoflow_reconcile_enabled`,
    /// `cron_tick_enabled`, `gate_sweep_enabled` (see `rupu-config`'s `CpConfig`).
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

            // `cp serve`'s own traffic (fleet HTTP to remote hosts, the ASN
            // refresh download, the SCM registry built below) is
            // deliberately NOT recorded, per the netflow per-run plan: a
            // daemon has no single run to attribute its own egress to, and
            // installing a daemon-lifetime sink here is exactly the "first
            // run wins" defect this plan removes (the old shared
            // `$RUPU_HOME/netflow/flows.jsonl` ledger this used to install
            // via a process-wide `http::init` no longer exists). Every
            // `HostConnector`/ASN-refresh/registry construction below takes
            // `Arc::new(NullSink)` explicitly instead.

            // `[cp]` runtime settings gate the two background-tick loops
            // below (autoflow reconcile / cron tick). A missing/malformed
            // config file just falls back to `CpConfig::default()` — both
            // loops enabled, 60s cadence — same as an absent `[cp]` section.
            // `[netflow]` rides along in the same load: the gate-sweep tick
            // (below) also drives the automatic ASN-table refresh, so it
            // needs `NetflowConfig` alongside `CpConfig`.
            let (cp_runtime_cfg, netflow_cfg) = {
                let global_cfg_path = global_dir.join("config.toml");
                let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), None)
                    .unwrap_or_default();
                (cfg.cp, cfg.netflow)
            };
            // Spawn the background resume worker. It builds the SAME
            // RunStore the CP's AppState does (`<global_dir>/runs`), so it
            // claims/approves/resumes runs the web UI marked for resume.
            let store = Arc::new(RunStore::new(global_dir.join("runs")));
            let worker_id = format!("cp-serve-{}", std::process::id());
            let gate_sweep_worker_id = format!("{worker_id}-gate-sweep");
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            tracing::info!(
                worker_id = %worker_id,
                "resume worker active: finishing web-approved gates"
            );
            let worker_handle = tokio::spawn(run_resume_worker(
                Arc::clone(&store),
                worker_id,
                rupu_workspace::HostStore {
                    root: global_dir.join("hosts"),
                },
                shutdown_rx,
            ));
            let poller_handle = tokio::spawn(run_bucket_poller(
                Arc::clone(&store),
                rupu_workspace::HostStore {
                    root: global_dir.join("hosts"),
                },
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
            // runner never wedges Live Events. The gate-sweep BODY (not the
            // loop itself, see below) is gated by `[cp].gate_sweep_enabled`
            // (default: on); cadence from `[cp].gate_sweep_interval_secs`
            // (default: 60s).
            //
            // The same tick also drives the netflow ASN-table refresh
            // (netflow Plan 2, Task 9): rather than a dedicated timer, it
            // piggybacks on this already-running loop and checks
            // `should_refresh_asn` — cheap (an `fs::metadata` call) on every
            // tick, with the actual download spawned off, never inline,
            // only on the rare tick where the table is missing or older
            // than `[netflow].asn_refresh_interval_days`. The operator must
            // never have to run a command for enrichment to work.
            //
            // The loop's own `enabled` flag (below) is therefore
            // `sweep_loop_enabled(cp, netflow)` (`gate_sweep_enabled ||
            // asn_auto_refresh`), NOT `gate_sweep_enabled` alone: collapsing
            // the two into one boolean would mean an operator who disables
            // gate-timeout enforcement but leaves ASN auto-refresh on (the
            // default for both) gets zero refresh ticks. `gate_sweep_enabled`
            // still independently guards whether `run_gate_sweep` itself
            // runs each tick; `should_refresh_asn` remains the sole
            // authority over whether the ASN refresh fires.
            let gate_sweep_store = Arc::clone(&store);
            let gate_sweep_hosts = rupu_workspace::HostStore {
                root: global_dir.join("hosts"),
            };
            let gate_sweep_exe = exe.clone();
            let gate_sweep_enabled = cp_runtime_cfg.gate_sweep_enabled;
            let gate_sweep_handle = tokio::spawn(run_periodic_tick(
                "gate-sweep",
                sweep_loop_enabled(&cp_runtime_cfg, &netflow_cfg),
                Duration::from_secs(cp_runtime_cfg.gate_sweep_interval_secs.max(1)),
                shutdown_tx.subscribe(),
                move || {
                    let store = Arc::clone(&gate_sweep_store);
                    let hosts = rupu_workspace::HostStore {
                        root: gate_sweep_hosts.root.clone(),
                    };
                    let exe = gate_sweep_exe.clone();
                    let worker_id = gate_sweep_worker_id.clone();
                    let netflow_cfg = netflow_cfg.clone();
                    async move {
                        if gate_sweep_enabled {
                            run_gate_sweep(store, hosts, exe, worker_id).await;
                        }

                        // ASN freshness. Best-effort and never blocking: a
                        // failed refresh leaves the existing table
                        // untouched and enrichment degrades to "not
                        // loaded" rather than to wrong answers.
                        //
                        // Single-flight guarded: `should_refresh_asn` only
                        // looks at on-disk mtime, so a download that stalls
                        // forever (no server-side close, dead connection
                        // that never times out at the TCP layer) would
                        // otherwise still look "stale" next tick and spawn
                        // a second, overlapping refresh. `AsnTable::write`
                        // uses a FIXED `<path>.db.tmp` intermediate file,
                        // not a per-call unique name, so two concurrent
                        // refreshes would race writing that same file — a
                        // data race, not just wasted bandwidth.
                        // `try_claim_asn_refresh` makes at most one refresh
                        // in flight at a time; the guard is released on
                        // BOTH the `Ok` and `Err` arms so a failed refresh
                        // can never wedge it shut.
                        //
                        // A bounded total timeout closes the other half of
                        // that same hazard: without ONE, a genuinely stuck
                        // download (not a race, just a slow/dead peer)
                        // holds `ASN_REFRESH_IN_FLIGHT` forever, and the
                        // guard above then silently ends auto-refresh for
                        // the rest of the daemon's life — every future tick
                        // sees the guard held and skips. Generous because
                        // this is a multi-megabyte download, not a normal
                        // API call: minutes, not the usual per-request
                        // seconds-scale timeout.
                        const ASN_REFRESH_TIMEOUT: std::time::Duration =
                            std::time::Duration::from_secs(300);
                        if let Some(db) = rupu_netflow::asn::asn_db_path() {
                            if should_refresh_asn(&netflow_cfg, &db)
                                && try_claim_asn_refresh(&ASN_REFRESH_IN_FLIGHT)
                            {
                                let url = netflow_cfg.asn_source_url.clone();
                                tokio::spawn(async move {
                                    let ctx =
                                        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System);
                                    // `Arc::new(NullSink)`: `cp serve`'s own
                                    // background ASN refresh is daemon
                                    // traffic, not run-scoped — deliberately
                                    // unrecorded (see the netflow sink note
                                    // at the top of `Action::Serve`).
                                    let client = match rupu_netflow::http::client_with(
                                        ctx,
                                        reqwest::Client::builder().timeout(ASN_REFRESH_TIMEOUT),
                                        Arc::new(rupu_netflow::NullSink),
                                    ) {
                                        Ok(c) => c,
                                        Err(e) => {
                                            tracing::warn!(error = %e, "failed to build netflow ASN-refresh client");
                                            ASN_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
                                            return;
                                        }
                                    };
                                    match rupu_netflow::asn::refresh(&url, &db, &client).await {
                                        Ok(()) => {
                                            tracing::info!(path = ?db, "netflow ASN table refreshed")
                                        }
                                        Err(e) => tracing::warn!(
                                            error = %e,
                                            "netflow ASN refresh failed; keeping existing table"
                                        ),
                                    }
                                    ASN_REFRESH_IN_FLIGHT.store(false, Ordering::Release);
                                });
                            }
                        }
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

            // Adapter for rupu-cp's TranscriptMutator port: shells
            // `rupu transcript archive|delete <id>` using this same binary.
            let transcript_mutator: Option<
                Arc<dyn rupu_cp::transcript_mutator::TranscriptMutator>,
            > = Some(Arc::new(
                crate::cp_transcript_mutator::SubprocessTranscriptMutator { exe: exe.clone() },
            ));

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

            // Repo lister for the web Run target picker. The same SCM registry
            // and repo lister feed the fleet strip's SCM half below — one
            // credential resolution, not two.
            let scm_resolver = Arc::new(rupu_auth::resolver::KeychainResolver::new());
            let scm_registry = {
                let global_cfg = global_dir.join("config.toml");
                let cfg =
                    rupu_config::layer_files_locked(Some(&global_cfg), None).unwrap_or_default();
                Arc::new(
                    rupu_scm::Registry::discover(
                        scm_resolver.as_ref(),
                        &cfg,
                        Arc::new(rupu_netflow::NullSink),
                    )
                    .await,
                )
            };
            let repo_lister: Arc<dyn rupu_cp::repos::RepoLister> =
                Arc::new(crate::cp_repos::CpRepoLister {
                    registry: Arc::clone(&scm_registry),
                });
            let repos: Option<Arc<dyn rupu_cp::repos::RepoLister>> = Some(Arc::clone(&repo_lister));

            // Fleet inventory for the dashboard's fleet strip. Two halves on
            // very different TTLs — providers are one cheap call each, the SCM
            // half is one issue listing per connected repo — so they refresh on
            // separate tasks against one cache. The dashboard only ever reads
            // that cache, never the network.
            let inventory = Arc::new(crate::cp_inventory::CpFleetInventory::new(
                Arc::clone(&scm_resolver),
                Some(crate::cp_inventory::ScmDeps {
                    global_dir: global_dir.clone(),
                    resolver: Arc::clone(&scm_resolver),
                    repos: repo_lister,
                    scm: scm_registry,
                }),
            ));
            // One immediate refresh per half so the strip is populated by the
            // time the operator's first page load lands.
            let inventory_handle = {
                let inv = Arc::clone(&inventory);
                let mut shutdown = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    loop {
                        inv.refresh_providers().await;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(
                                crate::cp_inventory::PROBE_TTL_SECS,
                            )) => {}
                            _ = shutdown.changed() => break,
                        }
                    }
                })
            };
            let scm_inventory_handle = {
                let inv = Arc::clone(&inventory);
                let mut shutdown = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    loop {
                        inv.refresh_scm().await;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(
                                crate::cp_inventory::SCM_TTL_SECS,
                            )) => {}
                            _ = shutdown.changed() => break,
                        }
                    }
                })
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
                transcript_mutator,
                inventory: Some(inventory),
            })
            .await;

            // Signal every background loop to stop and wait for them to drain.
            let _ = shutdown_tx.send(true);
            let _ = worker_handle.await;
            let _ = poller_handle.await;
            let _ = autoflow_reconcile_handle.await;
            let _ = cron_tick_handle.await;
            let _ = gate_sweep_handle.await;
            let _ = inventory_handle.await;
            let _ = scm_inventory_handle.await;

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
            tokio::spawn(resume_one_run(store, run_id, subcommand, None));
        }
    }
}

/// Build the `rupu workflow <subcommand> <run_id> [--gate <g>] [--mode <m>]`
/// argv the resume worker spawns for a claimed run. Pure + independently
/// testable (T5b-2b-i correctness fix, spec §7): `gate` MUST come from the
/// run's `resume_gate_id` MARKER field, not live `awaiting_step_id` — a
/// second concurrent approve request can reorder the latter before this
/// runs, but the marker is immutable once written (see `resume_gate_id`'s
/// doc on `RunRecord`) — and is only ever appended for the `approve`
/// subcommand: a `Paused` resume (`workflow resume`) has no gate concept.
///
/// Without `--gate` on a genuinely >1-gate run, `workflow approve` hits
/// `AmbiguousGate` and the detached child exits non-zero — but the caller
/// already cleared the marker on a successful SPAWN (not on the child's
/// eventual exit status, which it can't observe — the child is detached),
/// so a missing `--gate` here permanently strands the run
/// `AwaitingApproval` with every gate still parked. This was a real,
/// shipped bug (fixed as part of T5b-2b-i) — see
/// `resume_one_run_passes_the_markers_gate_id_to_the_spawned_approve_child`
/// for the regression test.
fn build_resume_argv<'a>(
    subcommand: &'a str,
    run_id: &'a str,
    gate: Option<&'a str>,
    mode: Option<&'a str>,
    approver: Option<&'a str>,
) -> Vec<&'a str> {
    let mut argv: Vec<&str> = vec!["workflow", subcommand, run_id];
    if subcommand == "approve" {
        if let Some(g) = gate {
            argv.push("--gate");
            argv.push(g);
        }
        // ISSUES.md I-82: carry the web-initiated approve's true actor
        // (persisted on the run's `resume_approver` marker by
        // `request_resume_approval`) across the process boundary so the
        // spawned child records it on the gate decision row instead of
        // falling back to `whoami::username()`. `resume` (the Paused,
        // non-gate case) has no approver/gate-decision concept, so this is
        // approve-only, same as `--gate`.
        if let Some(a) = approver {
            argv.push("--approver");
            argv.push(a);
        }
    }
    if let Some(m) = mode {
        argv.push("--mode");
        argv.push(m);
    }
    argv
}

/// Resolve + spawn the detached `rupu workflow <subcommand> <run_id> [...]`
/// child for ONE already-claimed run (the resume worker's per-run body,
/// extracted so tests can drive it directly instead of waiting out the
/// worker's 4s poll interval), then clear its resume marker on either a
/// successful spawn (the child now owns the run) or a failed one (so a
/// poisoned run isn't retried forever).
///
/// Captures the requested resume mode AND the targeted gate (T5b-2b-i) from
/// the run's marker fields (`resume_mode`/`resume_gate_id`) while the
/// marker is still present, then hands off to
/// [`build_resume_argv`]. `exe_override` lets tests point at a fake
/// executable instead of `std::env::current_exe()` (the production
/// default, used when `None`) — e.g. a capture script that records its
/// argv, so a test can assert on the EXACT argv the real `rupu` binary
/// would have received, rather than just on marker-field plumbing.
async fn resume_one_run(
    store: Arc<RunStore>,
    run_id: String,
    subcommand: &'static str,
    exe_override: Option<std::path::PathBuf>,
) {
    let now2 = chrono::Utc::now();
    let loaded = store.load(&run_id).ok();
    let mode = loaded.as_ref().and_then(|r| r.resume_mode.clone());
    let gate = loaded.as_ref().and_then(|r| r.resume_gate_id.clone());
    let approver = loaded.as_ref().and_then(|r| r.resume_approver.clone());

    let exe = match exe_override {
        Some(p) => p,
        None => match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(run_id = %run_id, error = %e, "resume worker: cannot resolve current exe; clearing marker");
                if let Err(ce) = store.clear_resume(&run_id, now2) {
                    tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                }
                return;
            }
        },
    };

    let argv = build_resume_argv(
        subcommand,
        &run_id,
        gate.as_deref(),
        mode.as_deref(),
        approver.as_deref(),
    );

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
}

/// One pass of the cp-serve gate sweep (Plan 4). For every run in the
/// store it classifies the needed action via [`sweep_decision`] and maps it
/// to store IO:
///
/// * `AwaitingApproval` (non-remote): resolve the gate's `on_timeout`, call
///   `expire_if_overdue` (finalizes the `Fail`/default timeout, or no-ops
///   when not overdue), then — per the decision — claim the resume lease
///   and spawn a detached `rupu workflow approve <id>` (`on_timeout:
///   approve`) or run the `on_reject` cleanup chain (`on_timeout: reject`).
///   The claim (via [`RunStore::claim_resume`]) guards against the SAME run
///   also being picked up by the web-approve `run_resume_worker` — both
///   paths race on the same lease field regardless of which one requested
///   the resume, so whichever claims first wins and the other backs off.
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
    worker_id: String,
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
                // Task 5b-2a (spec §7/§8): iterate the FULL awaiting set —
                // one element for a legacy/single-gate run (see
                // `awaiting_gates`'s normalization), several for a genuine
                // multi-gate DAG run — so each gate's OWN `on_timeout`
                // policy resolves independently rather than assuming "the
                // sole/first gate". Snapshotted before the loop: a `Fail`
                // action on one gate finalizes the WHOLE run
                // (`expire_gate_if_overdue`'s doc), which must stop this
                // loop from touching sibling gates that no longer exist on
                // a terminal record.
                let gates_snapshot = rec.awaiting_gates();
                for gate in gates_snapshot {
                    if rec.status != rupu_orchestrator::RunStatus::AwaitingApproval {
                        break;
                    }
                    let expired = gate.expires_at.is_some_and(|exp| now > exp);
                    // Cheap short-circuit: a gate that isn't overdue yet
                    // needs no snapshot read / expire call (the decision
                    // would be `Skip`).
                    if !expired {
                        continue;
                    }
                    let gate_step_id = gate.step_id.clone();
                    let on_timeout = store.resolve_gate_timeout_for(&rec, &gate_step_id);
                    let decision = sweep_decision(rec.status, on_timeout, expired, None, is_remote);
                    let expire_res =
                        store.expire_gate_if_overdue(&mut rec, &gate_step_id, now, on_timeout);
                    let outcome = match expire_res {
                        Ok(o) => o,
                        Err(e) => {
                            tracing::warn!(run_id = %run_id, gate = %gate_step_id, error = %e, "gate sweep: expire_gate_if_overdue failed");
                            continue;
                        }
                    };
                    match decision {
                        SweepAction::Skip => {
                            // Not overdue, or Fail/default already finalized
                            // inside the expire call.
                            if matches!(outcome, Some(rupu_orchestrator::TimeoutAction::Fail)) {
                                tracing::info!(run_id = %run_id, gate = %gate_step_id, "gate sweep: gate timed out → run failed");
                            }
                        }
                        SweepAction::ExpireApprove => {
                            // Claim the resume lease before spawning: the web
                            // approve path's `run_resume_worker` also spawns
                            // `workflow approve` for AwaitingApproval runs it
                            // finds via `list_pending_resume`, and both paths
                            // race on the SAME `resume_claimed_at` field
                            // regardless of which one requested the resume — so
                            // claiming here stops the sweep from re-spawning a
                            // second approve for a run the resume worker (or a
                            // prior sweep tick) already handed off.
                            let claimed = match store.claim_resume(&run_id, &worker_id, now) {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(run_id = %run_id, error = %e, "gate sweep: claim_resume failed");
                                    continue;
                                }
                            };
                            if !claimed {
                                tracing::info!(run_id = %run_id, "gate-sweep: approve already claimed for run_id, skipping");
                                continue;
                            }
                            // expire left the record AwaitingApproval; hand off
                            // to a detached `rupu workflow approve <id> --gate
                            // <gate_step_id>`, which re-resolves the
                            // on_timeout: approve policy and does the approve +
                            // in-process resume in its own killable process
                            // (mirrors the resume worker's detached spawn).
                            // `--gate` targets exactly the timed-out gate —
                            // required once >1 gate can be parked (a bare
                            // `approve` would hit `AmbiguousGate` if a sibling
                            // is still parked), harmless for the sole-gate
                            // case.
                            let mut argv: Vec<&str> =
                                vec!["workflow", "approve", &run_id, "--gate", &gate_step_id];
                            if let Some(m) = rec.resume_mode.as_deref() {
                                argv.push("--mode");
                                argv.push(m);
                            }
                            match std::process::Command::new(&exe).args(&argv).spawn() {
                                Ok(_child) => {
                                    tracing::info!(run_id = %run_id, gate = %gate_step_id, "gate sweep: on_timeout=approve → spawned detached workflow approve");
                                    // Minor (5b-2a handoff): the detached child
                                    // now independently owns approving THIS
                                    // gate — it will call `store.approve_gate`
                                    // itself, which persists removing
                                    // `gate_step_id` from `awaiting`. Mirror
                                    // that removal in THIS in-memory `rec` too
                                    // so a sibling gate processed later in the
                                    // SAME sweep pass (e.g. an
                                    // `ExpireThenCleanupReject` arm below, via
                                    // `expire_gate_if_overdue`'s
                                    // `self.update(record)`) doesn't clobber
                                    // the child's concurrent write by
                                    // re-persisting a full record that still
                                    // shows this gate parked — a race that
                                    // otherwise transiently flips the run back
                                    // to `AwaitingApproval[gate_step_id]`
                                    // (self-heals once the child's own write
                                    // lands, but must not be reintroduced by a
                                    // sibling's write in this same pass). Only
                                    // done on a successful spawn: on a failed
                                    // spawn no child exists to race against,
                                    // and the gate is still genuinely parked
                                    // on disk for the next sweep tick to retry.
                                    rec.awaiting = rec
                                        .awaiting_gates()
                                        .into_iter()
                                        .filter(|g| g.step_id != gate_step_id)
                                        .collect();
                                    rec.sync_awaiting_compat();
                                    // The spawned child now owns the run —
                                    // clear the marker/claim exactly like the
                                    // resume worker does after its own spawn.
                                    if let Err(ce) = store.clear_resume(&run_id, now) {
                                        tracing::warn!(run_id = %run_id, error = %ce, "gate sweep: clear_resume failed");
                                    }
                                }
                                Err(e) => {
                                    // I-43: deliberately do NOT clear_resume
                                    // here. No child was spawned to own the
                                    // run, so if we cleared the claim, the
                                    // very next sweep tick would see a free
                                    // lease and re-spawn immediately — for a
                                    // permanently-failing spawn (bad exe
                                    // path, exhausted process table, etc.)
                                    // that re-spawns forever at the sweep's
                                    // cadence (default 60s) with no backoff,
                                    // since nothing else ever moves the run
                                    // out of `AwaitingApproval`. Leaving the
                                    // claim in place reuses the SAME
                                    // `RESUME_LEASE` TTL `claim_resume`
                                    // already enforces for the web-approve
                                    // race (5 minutes) as a backoff: the next
                                    // reclaim attempt (this sweep or the
                                    // resume worker) waits for the lease to
                                    // go stale rather than retrying every
                                    // tick. The gate itself stays genuinely
                                    // parked on disk either way.
                                    tracing::error!(run_id = %run_id, gate = %gate_step_id, error = %e, "gate sweep: failed to spawn workflow approve for on_timeout=approve; leaving resume claim in place to back off retries");
                                }
                            }
                        }
                        SweepAction::ExpireThenCleanupReject => {
                            // expire_gate_if_overdue removed THIS gate from
                            // the set (and finalized the run Rejected only if
                            // that emptied it — other parked gates may still
                            // remain). Run the same on_reject cleanup chain
                            // the CLI reject path runs, scoped to this gate.
                            if !matches!(outcome, Some(rupu_orchestrator::TimeoutAction::Reject)) {
                                tracing::warn!(run_id = %run_id, gate = %gate_step_id, "gate sweep: expected reject outcome for on_timeout=reject but got {outcome:?}; skipping cleanup");
                                continue;
                            }
                            let reason = rec.error_message.clone().unwrap_or_else(|| {
                                "gate timed out (on_timeout: reject)".to_string()
                            });
                            tracing::info!(run_id = %run_id, step_id = %gate_step_id, "gate sweep: on_timeout=reject → gate auto-rejected; running on_reject cleanup");
                            // I-36: run this unconditionally — an empty
                            // `on_reject:` chain must still record the
                            // gate's rejected decision (see the identical
                            // fix in the CLI `reject` command). No operator
                            // is involved in this autonomous, timeout-driven
                            // path, so `approver` is `None`.
                            match crate::resume::build_reject_cleanup_opts(
                                &store,
                                &run_id,
                                &gate_step_id,
                                &reason,
                                rec.resume_mode.as_deref(),
                            )
                            .await
                            {
                                Ok((opts, chain_len)) => {
                                    match rupu_orchestrator::runner::run_reject_cleanup(
                                        opts,
                                        &gate_step_id,
                                        &reason,
                                        "timeout",
                                        None,
                                    )
                                    .await
                                    {
                                        Ok(()) => {
                                            tracing::info!(run_id = %run_id, gate = %gate_step_id, chain_len, "gate sweep: on_reject cleanup chain executed");
                                        }
                                        Err(e) => {
                                            tracing::warn!(run_id = %run_id, gate = %gate_step_id, error = %e, "gate sweep: on_reject cleanup chain errored (gate already rejected)");
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(run_id = %run_id, gate = %gate_step_id, error = %e, "gate sweep: could not build on_reject cleanup opts (gate already rejected)");
                                }
                            }
                        }
                        SweepAction::Reap => {
                            tracing::warn!(run_id = %run_id, gate = %gate_step_id, "gate sweep: unexpected Reap decision for AwaitingApproval run; skipping");
                        }
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
            // I-35: a `Rejected` run whose reject/cancel had no workflow
            // runtime of its own to run the chain synchronously (CP web,
            // `LocalHostConnector`/`HttpHostConnector`, the rupu-app
            // desktop executor, and `RunStore::cancel` — see
            // `RunRecord::reject_cleanup_pending`'s doc) leaves the marker
            // set; this arm is the deferred worker for it, mirroring the
            // resume worker's marker-and-sweep shape for the approve side.
            // Matches on the MARKER (via `list_reject_cleanup_pending`'s
            // same filter, re-checked here per-run since this loop already
            // iterates every run), never on `status == Rejected` alone —
            // a run whose chain already ran keeps that status forever but
            // has no marker, so it's a cheap no-op below rather than a
            // repeat of the chain.
            rupu_orchestrator::RunStatus::Rejected => {
                if is_remote {
                    tracing::debug!(run_id = %run_id, "gate sweep: skipping remote-host rejected run");
                    continue;
                }
                let Some(marker) = rec.reject_cleanup_pending.clone() else {
                    continue;
                };
                tracing::info!(run_id = %run_id, step_id = %marker.step_id, "gate sweep: running deferred on_reject cleanup (I-35)");
                match crate::resume::build_reject_cleanup_opts(
                    &store,
                    &run_id,
                    &marker.step_id,
                    &marker.reason,
                    rec.resume_mode.as_deref(),
                )
                .await
                {
                    Ok((opts, chain_len)) => {
                        match rupu_orchestrator::runner::run_reject_cleanup(
                            opts,
                            &marker.step_id,
                            &marker.reason,
                            &marker.via,
                            marker.approver.as_deref(),
                        )
                        .await
                        {
                            Ok(()) => {
                                tracing::info!(run_id = %run_id, step_id = %marker.step_id, chain_len, "gate sweep: deferred on_reject cleanup executed");
                                if let Err(e) = store.clear_reject_cleanup(&run_id) {
                                    tracing::warn!(run_id = %run_id, error = %e, "gate sweep: clear_reject_cleanup failed");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(run_id = %run_id, error = %e, "gate sweep: deferred on_reject cleanup chain errored; marker left set, will retry next tick");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(run_id = %run_id, error = %e, "gate sweep: could not build deferred on_reject cleanup opts; marker left set, will retry next tick");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Whether the ASN table should be refreshed right now.
///
/// Pure so the policy is testable without a network or a timer: `false`
/// when auto-refresh is off in config, otherwise delegates staleness to
/// `rupu_netflow::asn::is_stale` (missing / unreadable / stale mtime / a
/// zero-day interval all count as stale).
pub(crate) fn should_refresh_asn(cfg: &rupu_config::NetflowConfig, path: &std::path::Path) -> bool {
    cfg.asn_auto_refresh && rupu_netflow::asn::is_stale(path, cfg.asn_refresh_interval_days)
}

/// Whether the shared gate-sweep/ASN-refresh background loop should run at
/// all in this process.
///
/// Deliberately NOT `cp.gate_sweep_enabled` alone: the loop drives two
/// independent things (gate-timeout enforcement and ASN auto-refresh), and
/// an operator who disables the former while leaving the latter on — the
/// default for both — must still get refresh ticks. `cp.gate_sweep_enabled`
/// separately gates whether `run_gate_sweep` itself executes inside the
/// tick body; `should_refresh_asn` remains the sole authority over whether
/// an ASN refresh actually fires on a given tick.
pub(crate) fn sweep_loop_enabled(
    cp: &rupu_config::CpConfig,
    netflow: &rupu_config::NetflowConfig,
) -> bool {
    cp.gate_sweep_enabled || netflow.asn_auto_refresh
}

/// Process-wide single-flight guard for the ASN refresh spawned from the
/// gate-sweep tick. `should_refresh_asn` only observes on-disk mtime, so a
/// download that stalls past one tick interval (the netflow HTTP client
/// sets no request timeout) would otherwise look stale again on the next
/// tick and get refreshed a second time concurrently. `AsnTable::write`'s
/// intermediate file is a FIXED `<path>.db.tmp`, not a per-call unique
/// name, so two concurrent refreshes racing to write it is a genuine data
/// race, not merely wasted bandwidth.
static ASN_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Attempts to claim a single-flight guard so at most one ASN refresh is
/// ever in flight at a time. Returns `true` if this call claimed it (the
/// caller may proceed and MUST clear the guard — via
/// `guard.store(false, Ordering::Release)` — when the refresh finishes, on
/// BOTH the success and failure path; clearing only on success would let a
/// failed refresh wedge the guard shut forever). Returns `false` if a
/// refresh is already in flight.
///
/// Generic over the guard so it is unit-testable against a fresh local
/// `AtomicBool`, without touching the process-wide [`ASN_REFRESH_IN_FLIGHT`].
pub(crate) fn try_claim_asn_refresh(guard: &AtomicBool) -> bool {
    guard
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::{
        build_resume_argv, resume_one_run, run_gate_sweep, run_periodic_tick, should_refresh_asn,
        sweep_decision, sweep_loop_enabled, try_claim_asn_refresh, SweepAction,
    };
    use rupu_orchestrator::{RunStatus, TimeoutAction};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // ── T5b-2b-i correctness fix: the resume worker's gate-id round-trip ──

    #[test]
    fn build_resume_argv_includes_gate_only_for_approve_when_present() {
        // The bug: on a >1-gate run, the worker used to omit `--gate`
        // entirely, so the spawned `workflow approve` hit `AmbiguousGate`
        // and exited non-zero while the marker had already been cleared —
        // permanently stranding the run `AwaitingApproval` with every gate
        // still parked. `--gate` must be present whenever a gate id was
        // resolved AND the subcommand is `approve`.
        assert_eq!(
            build_resume_argv("approve", "run_x", Some("gate_b"), None, None),
            vec!["workflow", "approve", "run_x", "--gate", "gate_b"],
        );
        // Legacy/sole-gate marker (no gate id resolved) — back-compat,
        // `--gate` omitted exactly like before this field existed.
        assert_eq!(
            build_resume_argv("approve", "run_x", None, None, None),
            vec!["workflow", "approve", "run_x"],
        );
        // `--mode` composes with `--gate`.
        assert_eq!(
            build_resume_argv("approve", "run_x", Some("gate_b"), Some("bypass"), None),
            vec!["workflow", "approve", "run_x", "--gate", "gate_b", "--mode", "bypass"],
        );
        // `workflow resume` (a cooperative-pause resume) has no gate
        // concept — `--gate` must never appear even if `gate` is `Some`
        // (e.g. a stale marker field left over from a different flow).
        assert_eq!(
            build_resume_argv("resume", "run_x", Some("gate_b"), None, None),
            vec!["workflow", "resume", "run_x"],
        );
    }

    // ── ISSUES.md I-82: the resume worker's approver round-trip ──

    #[test]
    fn build_resume_argv_includes_approver_only_for_approve_when_present() {
        // The bug: `request_resume_approval` learned the web approver but
        // never persisted it anywhere the resume worker could read, so the
        // spawned `workflow approve` always re-derived `whoami::username()`
        // — the identity of whatever account runs `cp serve`, not the real
        // web-initiated actor. `--approver` must be present whenever the
        // marker carried one AND the subcommand is `approve`.
        assert_eq!(
            build_resume_argv("approve", "run_x", None, None, Some("web")),
            vec!["workflow", "approve", "run_x", "--approver", "web"],
        );
        // No approver on the marker (e.g. a record written before this
        // field existed) — `--approver` omitted, falls back to
        // `whoami::username()` exactly like before this field existed.
        assert_eq!(
            build_resume_argv("approve", "run_x", None, None, None),
            vec!["workflow", "approve", "run_x"],
        );
        // Composes with `--gate` and `--mode`.
        assert_eq!(
            build_resume_argv(
                "approve",
                "run_x",
                Some("gate_b"),
                Some("bypass"),
                Some("web"),
            ),
            vec![
                "workflow",
                "approve",
                "run_x",
                "--gate",
                "gate_b",
                "--approver",
                "web",
                "--mode",
                "bypass",
            ],
        );
        // `workflow resume` (a cooperative-pause resume) has no
        // approver/gate-decision concept — `--approver` must never appear
        // even if `approver` is `Some` (e.g. a stale marker field left
        // over from a different flow).
        assert_eq!(
            build_resume_argv("resume", "run_x", None, None, Some("web")),
            vec!["workflow", "resume", "run_x"],
        );
    }

    /// A 2-gate `AwaitingApproval` run for the resume-worker round-trip
    /// tests — both `gate_a` and `gate_b` genuinely parked.
    fn multi_gate_resume_record(id: &str) -> rupu_orchestrator::RunRecord {
        use rupu_orchestrator::runs::AwaitingGate;
        let now = chrono::Utc::now();
        let mut rec = rupu_orchestrator::RunRecord {
            id: id.into(),
            workflow_name: "g".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: std::path::PathBuf::from("/tmp/proj"),
            transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: now,
            finished_at: None,
            final_output: None,
            error_message: None,
            awaiting: vec![
                AwaitingGate {
                    step_id: "gate_a".into(),
                    prompt: Some("approve a?".into()),
                    since: now,
                    expires_at: None,
                },
                AwaitingGate {
                    step_id: "gate_b".into(),
                    prompt: Some("approve b?".into()),
                    since: now,
                    expires_at: None,
                },
            ],
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
        };
        rec.sync_awaiting_compat();
        rec
    }

    /// The full round-trip this bug slipped through on: a web operator
    /// approves gate_b of a 2-gate run (`request_resume_approval` — the
    /// EXACT call `api/runs.rs`'s `approve_run` makes), which sets the
    /// `resume_gate_id` marker; `resume_one_run` (the resume worker's
    /// per-run body) then reads that marker and spawns the child with
    /// `--gate gate_b` in its REAL argv — verified by pointing the spawn at
    /// a capture script instead of a real `rupu` binary, so this asserts on
    /// the literal argv a production spawn would receive, not just on
    /// marker-field plumbing. Must FAIL on the pre-fix code (which had no
    /// `resume_gate_id` field/argv wiring at all — verified via a temporary
    /// revert, see the T5b-2b-i report) and PASS with the fix.
    #[tokio::test]
    async fn resume_one_run_passes_the_markers_gate_id_to_the_spawned_approve_child() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let rec = multi_gate_resume_record("run_resume_worker_gate_b");
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate_a\n    approval: {}\n  - id: gate_b\n    approval: {}\n",
            )
            .unwrap();

        // Simulate the web operator approving gate_b specifically — the
        // exact `RunStore` call `POST /api/runs/:id/approve?gate=gate_b`
        // makes (api/runs.rs's `approve_run`).
        let now = chrono::Utc::now();
        store
            .request_resume_approval(&rec.id, "web", None, now, Some("gate_b"))
            .expect("approving gate_b of a 2-gate run should succeed");
        // Sanity: the marker really does carry the target gate.
        assert_eq!(
            store.load(&rec.id).unwrap().resume_gate_id.as_deref(),
            Some("gate_b")
        );

        // A capture script standing in for the real `rupu` binary: appends
        // its argv to a file instead of actually running anything.
        let capture_path = tmp.path().join("captured_argv.txt");
        let script_path = tmp.path().join("capture.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", capture_path.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        resume_one_run(
            Arc::clone(&store),
            rec.id.clone(),
            "approve",
            Some(script_path),
        )
        .await;

        // The script runs asynchronously (detached, not awaited by
        // `resume_one_run` itself) — poll briefly for its output.
        let mut captured = None;
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&capture_path) {
                captured = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let captured = captured.expect("capture script should have run and recorded its argv");
        assert!(
            captured.contains("--gate gate_b"),
            "spawned child's argv was: {captured:?} — missing --gate gate_b"
        );
        // gate_a must not appear as the target.
        assert!(!captured.contains("--gate gate_a"));
        // ISSUES.md I-82: the web approve's true actor ("web", passed to
        // `request_resume_approval` above) must reach the spawned child so
        // it lands on the gate decision row, instead of the child silently
        // re-deriving `whoami::username()`.
        assert!(
            captured.contains("--approver web"),
            "spawned child's argv was: {captured:?} — missing --approver web"
        );

        // The marker was cleared (spawn succeeded) — the child now owns
        // resolving gate_b for real.
        let reloaded = store.load(&rec.id).unwrap();
        assert!(reloaded.resume_requested_at.is_none());
        assert!(reloaded.resume_gate_id.is_none());
        assert!(reloaded.resume_approver.is_none());
    }

    /// Single-gate parity: a legacy/sole-gate marker (`resume_gate_id:
    /// None`) must still spawn a working `workflow approve` — `--gate`
    /// omitted, exactly like before this field existed.
    #[tokio::test]
    async fn resume_one_run_omits_gate_for_a_sole_gate_marker() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let mut rec = multi_gate_resume_record("run_resume_worker_sole");
        // Collapse to a single parked gate.
        rec.awaiting.truncate(1);
        rec.sync_awaiting_compat();
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate_a\n    approval: {}\n",
            )
            .unwrap();

        let now = chrono::Utc::now();
        store
            .request_resume_approval(&rec.id, "web", None, now, None)
            .expect("approving the sole gate with no explicit id should work as today");
        assert_eq!(store.load(&rec.id).unwrap().resume_gate_id, None);

        let capture_path = tmp.path().join("captured_argv.txt");
        let script_path = tmp.path().join("capture.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", capture_path.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        resume_one_run(
            Arc::clone(&store),
            rec.id.clone(),
            "approve",
            Some(script_path),
        )
        .await;

        let mut captured = None;
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&capture_path) {
                captured = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let captured = captured.expect("capture script should have run and recorded its argv");
        assert!(
            !captured.contains("--gate"),
            "sole-gate resume must not pass --gate: {captured:?}"
        );
    }

    /// gate_a: `timeout_seconds: 10`, `on_timeout: reject`, no `on_reject:`
    /// (an empty chain). gate_b: a plain gate with NO timeout — must never be
    /// touched by timing out gate_a.
    ///
    /// NOTE (I-36): this fixture used to rely on `cheap_on_reject_chain_len`
    /// short-circuiting an empty chain before the heavy
    /// `build_reject_cleanup_opts`/`run_reject_cleanup` path, which needs real
    /// config/resolver/MCP-registry wiring this unit test avoids. That
    /// short-circuit was REMOVED — skipping the chain also skipped the only
    /// `emit_gate_result` call, so an empty-chain reject recorded no gate
    /// decision. The empty chain here is now purely about keeping this test
    /// focused on sibling-gate isolation.
    const GATE_A_REJECT_GATE_B_NONE_YAML: &str =
        "name: g\nsteps:\n  - id: gate_a\n    approval:\n      timeout_seconds: 10\n      on_timeout: reject\n  - id: gate_b\n    approval: {}\n";

    /// Task 5b-2a: the sweep must time out ONLY the overdue gate in a
    /// multi-gate `awaiting` set, leaving every other parked gate — and the
    /// run's `AwaitingApproval` status — untouched.
    #[tokio::test]
    async fn run_gate_sweep_times_out_only_the_overdue_gate_leaves_sibling_parked() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let hosts = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        // Never actually spawned in this scenario: the only overdue gate
        // routes to `on_timeout: reject` with a 0-length `on_reject` chain,
        // which never reaches the `ExpireApprove` spawn branch.
        let exe = std::env::current_exe().unwrap();

        let now = chrono::Utc::now();
        let since = now - chrono::Duration::seconds(120);
        let mut rec = rupu_orchestrator::RunRecord {
            id: "run_sweep_multi_gate".into(),
            workflow_name: "g".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            started_at: since,
            finished_at: None,
            final_output: None,
            error_message: None,
            awaiting: vec![
                rupu_orchestrator::runs::AwaitingGate {
                    step_id: "gate_a".into(),
                    prompt: Some("approve a?".into()),
                    since,
                    expires_at: Some(now - chrono::Duration::seconds(30)), // overdue
                },
                rupu_orchestrator::runs::AwaitingGate {
                    step_id: "gate_b".into(),
                    prompt: Some("approve b?".into()),
                    since,
                    expires_at: None, // no timeout — must never expire
                },
            ],
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
        };
        rec.sync_awaiting_compat();
        store
            .create(rec.clone(), GATE_A_REJECT_GATE_B_NONE_YAML)
            .unwrap();

        run_gate_sweep(Arc::clone(&store), hosts, exe, "sweep-test".to_string()).await;

        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(
            reloaded.status,
            RunStatus::AwaitingApproval,
            "gate_b is still parked — the run must not finalize"
        );
        assert_eq!(reloaded.awaiting.len(), 1);
        assert_eq!(reloaded.awaiting[0].step_id, "gate_b");
        assert_eq!(reloaded.awaiting_step_id.as_deref(), Some("gate_b"));
    }

    /// gate_a: `on_timeout: approve`. gate_b: `on_timeout: reject` with no
    /// `on_reject:` (empty chain, same short-circuit as
    /// `GATE_A_REJECT_GATE_B_NONE_YAML`).
    const GATE_A_APPROVE_GATE_B_REJECT_YAML: &str = "name: g\nsteps:\n  - id: gate_a\n    approval:\n      timeout_seconds: 10\n      on_timeout: approve\n  - id: gate_b\n    approval:\n      timeout_seconds: 10\n      on_timeout: reject\n";

    /// Task 5b-2b-i sweep-race fix: when a SINGLE sweep pass processes two
    /// concurrently-overdue gates with mixed `on_timeout` policies — gate_a:
    /// `approve` (processed first, per `awaiting`'s order), gate_b: `reject`
    /// (processed second) — the approve branch's detached spawn hands gate_a
    /// off to a child process that will independently call
    /// `store.approve_gate`. Before this fix, the in-memory `rec` was never
    /// updated to reflect that hand-off, so gate_b's own
    /// `expire_gate_if_overdue` → `self.update(record)` a moment later
    /// persisted a STALE 2-gate copy (minus only gate_b) — resurrecting
    /// gate_a in `run.json` even though a detached process now owns
    /// resolving it, and leaving the run `AwaitingApproval` instead of
    /// reaching the `Rejected` state gate_b's own timeout earned on its own
    /// path.
    #[tokio::test]
    async fn run_gate_sweep_two_overdue_mixed_policy_gates_does_not_resurrect_the_approved_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let hosts = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        // A trivial, instantly-exiting binary — the sweep's `ExpireApprove`
        // branch really does spawn it (so the fix's "on a *successful*
        // spawn" in-memory removal actually exercises), but it ignores
        // every arg and never touches `run.json`, so it can't itself
        // resolve the race this test is isolating.
        let exe = std::path::PathBuf::from("true");

        let now = chrono::Utc::now();
        let since = now - chrono::Duration::seconds(120);
        let mut rec = rupu_orchestrator::RunRecord {
            id: "run_sweep_mixed_policy".into(),
            workflow_name: "g".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            started_at: since,
            finished_at: None,
            final_output: None,
            error_message: None,
            awaiting: vec![
                rupu_orchestrator::runs::AwaitingGate {
                    step_id: "gate_a".into(),
                    prompt: Some("approve a?".into()),
                    since,
                    expires_at: Some(now - chrono::Duration::seconds(30)), // overdue
                },
                rupu_orchestrator::runs::AwaitingGate {
                    step_id: "gate_b".into(),
                    prompt: Some("approve b?".into()),
                    since,
                    expires_at: Some(now - chrono::Duration::seconds(30)), // overdue
                },
            ],
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
        };
        rec.sync_awaiting_compat();
        store
            .create(rec.clone(), GATE_A_APPROVE_GATE_B_REJECT_YAML)
            .unwrap();

        run_gate_sweep(Arc::clone(&store), hosts, exe, "sweep-test".to_string()).await;

        let reloaded = store.load(&rec.id).unwrap();
        assert!(
            !reloaded.awaiting.iter().any(|g| g.step_id == "gate_a"),
            "gate_a was handed off to a detached approve — it must not be \
             resurrected by gate_b's own reject persisting a stale \
             in-memory copy of the awaiting set: {:?}",
            reloaded.awaiting
        );
        // gate_b's own reject empties what's left of the (correctly fixed)
        // in-memory set, so the run reaches the same terminal `Rejected`
        // state its own timeout earned.
        assert_eq!(reloaded.status, RunStatus::Rejected);
        assert!(reloaded.awaiting.is_empty());
    }

    /// Single-gate parity: a legacy-shaped record (empty `awaiting`, only
    /// the derived-compat fields populated) with exactly ONE overdue gate
    /// must reach the SAME terminal routing the sweep reached before Task
    /// 5b-2a's per-gate loop existed.
    #[tokio::test]
    async fn run_gate_sweep_sole_overdue_gate_reaches_same_terminal_routing_as_before() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let hosts = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        let exe = std::env::current_exe().unwrap();

        let now = chrono::Utc::now();
        let rec = rupu_orchestrator::RunRecord {
            id: "run_sweep_sole_gate".into(),
            workflow_name: "g".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            started_at: now - chrono::Duration::seconds(120),
            finished_at: None,
            final_output: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: Some("gate_a".into()),
            approval_prompt: Some("approve a?".into()),
            awaiting_since: Some(now - chrono::Duration::seconds(120)),
            expires_at: Some(now - chrono::Duration::seconds(30)), // overdue
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
        };
        store
            .create(rec.clone(), GATE_A_REJECT_GATE_B_NONE_YAML)
            .unwrap();

        run_gate_sweep(Arc::clone(&store), hosts, exe, "sweep-test".to_string()).await;

        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(
            reloaded.status,
            RunStatus::Rejected,
            "a sole overdue gate must still finalize the whole run, same as before 5b-2a"
        );
        assert!(reloaded.awaiting.is_empty());
        assert!(reloaded.awaiting_step_id.is_none());
    }

    /// I-43: a spawn failure in the `ExpireApprove` arm must not clear the
    /// resume claim, or the very next sweep tick sees a free lease and
    /// re-spawns immediately — forever, at the sweep's cadence (default
    /// 60s), since nothing else ever flips the run out of
    /// `AwaitingApproval`. `exe` here is a path guaranteed not to exist, so
    /// `std::process::Command::spawn()` itself returns `Err` (the OS-level
    /// launch failure this test targets — not a child that launches and
    /// then exits nonzero).
    ///
    /// The binding assertion is about the SECOND tick: with the fix, the
    /// first failed spawn leaves `resume_claimed_at` set (claim retained),
    /// so the second tick's `claim_resume` sees a live lease and skips
    /// entirely — `resume_claimed_at` is unchanged between the two ticks.
    /// Pre-fix, the first tick clears the claim unconditionally, so the
    /// second tick re-claims (a fresh, later `resume_claimed_at`) and
    /// re-attempts the spawn.
    #[tokio::test]
    async fn run_gate_sweep_does_not_respawn_forever_after_spawn_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let hosts = rupu_workspace::HostStore {
            root: tmp.path().join("hosts"),
        };
        let exe = std::path::PathBuf::from("/nonexistent/rupu-gate-sweep-i43-test-binary");
        assert!(!exe.exists(), "test exe path must not exist");

        let now = chrono::Utc::now();
        let rec = rupu_orchestrator::RunRecord {
            id: "run_sweep_spawn_fail".into(),
            workflow_name: "g".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: tmp.path().to_path_buf(),
            transcript_dir: tmp.path().to_path_buf(),
            started_at: now - chrono::Duration::seconds(120),
            finished_at: None,
            final_output: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: Some("gate_a".into()),
            approval_prompt: Some("approve a?".into()),
            awaiting_since: Some(now - chrono::Duration::seconds(120)),
            expires_at: Some(now - chrono::Duration::seconds(30)), // overdue
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
        };
        // Single gate, `on_timeout: approve` — must route to `ExpireApprove`.
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate_a\n    approval:\n      timeout_seconds: 10\n      on_timeout: approve\n",
            )
            .unwrap();

        run_gate_sweep(
            Arc::clone(&store),
            hosts.clone(),
            exe.clone(),
            "sweep-test".to_string(),
        )
        .await;

        let after_tick1 = store.load(&rec.id).unwrap();
        assert_eq!(
            after_tick1.status,
            RunStatus::AwaitingApproval,
            "the gate is still genuinely parked after a failed spawn — nothing resolved it"
        );
        let claimed_after_tick1 = after_tick1.resume_claimed_at;
        assert!(
            claimed_after_tick1.is_some(),
            "a spawn failure must NOT clear the resume claim — it must leave the lease in \
             place so the existing RESUME_LEASE TTL backs off the next reclaim, instead of \
             clearing it and inviting an immediate re-spawn on the very next tick"
        );

        run_gate_sweep(Arc::clone(&store), hosts, exe, "sweep-test".to_string()).await;

        let after_tick2 = store.load(&rec.id).unwrap();
        assert_eq!(
            after_tick2.resume_claimed_at, claimed_after_tick1,
            "the second tick must see a still-live lease and skip re-claiming/re-spawning \
             entirely — a changed resume_claimed_at means it re-claimed, which means it \
             re-spawned, which is the unbounded-respawn bug"
        );
        assert_eq!(after_tick2.status, RunStatus::AwaitingApproval);
    }

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
                sweep_decision(
                    status,
                    Some(TimeoutAction::Reject),
                    true,
                    Some(false),
                    false
                ),
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

    // ── I-35: every reject/cancel path must run the on_reject chain ──
    //
    // Pre-fix, `RunStore::reject_gate`'s explicit-reject branch (the one
    // CP web, `LocalHostConnector`/`HttpHostConnector`, the rupu-app
    // executor, and `RunStore::cancel` all bottom out in) recorded NO
    // marker at all, and this sweep had no arm looking for one — so a
    // web/app/cancel reject's `on_reject` chain (and the gate decision
    // `run_reject_cleanup` is the only caller of `emit_gate_result` for)
    // never ran, no matter how many sweep ticks fired. These tests drive
    // the REAL `build_reject_cleanup_opts`/`run_reject_cleanup` path (not
    // a test-double factory) via the `RUPU_MOCK_PROVIDER_SCRIPT` seam
    // (`crates/rupu-runtime/src/provider_factory.rs`), mirroring
    // `crates/rupu-cli/tests/workflow_runs_no_side_effects.rs`'s fixture
    // shape: a real `write_file` tool call is the only reliable proof the
    // chain genuinely executed, as opposed to merely flipping a flag.

    /// The on_reject chain's only step: write a marker file via the real
    /// `write_file` tool. Its existence on disk is the proof the chain
    /// really ran (not just that some in-memory flag flipped).
    const I35_WRITE_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "write_file", "tool_input": {"path": "reject_cleanup_marker.txt", "content": "cleanup ran"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "done", "stop": "end_turn" } }
]
"#;

    const I35_WRITER_AGENT: &str = "---\nname: writer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 2\ntools: [write_file]\n---\nyou write files.";

    /// A single gate whose `on_reject` chain runs the `writer` agent above.
    const I35_WORKFLOW_NONEMPTY_CHAIN: &str = "name: g\nsteps:\n  - id: gate\n    approval:\n      prompt: \"Approve?\"\n      on_reject:\n        - id: cleanup\n          agent: writer\n          prompt: \"cleanup after reject\"\n";

    /// Same shape, but no `on_reject:` at all — Test 3's empty-chain,
    /// must-not-spin case.
    const I35_WORKFLOW_EMPTY_CHAIN: &str =
        "name: g\nsteps:\n  - id: gate\n    approval:\n      prompt: \"Approve?\"\n";

    /// `<tmp>/home` (a `RUPU_HOME` with the `writer` agent) + `<tmp>/workspace`
    /// (the run's workspace — must exist on disk for `project_root_for`'s
    /// `canonicalize`, but needs no `.rupu/` of its own: `rebuild_opts_from_disk`
    /// re-parses the workflow from the PERSISTED SNAPSHOT `store.create` was
    /// given, never from a project directory).
    fn i35_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join("agents")).unwrap();
        std::fs::write(home.join("agents/writer.md"), I35_WRITER_AGENT).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        (tmp, home, workspace)
    }

    /// An `AwaitingApproval` run parked at `gate`, workspace-bound to
    /// `workspace`, ready for an explicit reject/cancel. Caller still needs
    /// to `store.create(rec, <yaml>)` with whichever chain (non-empty or
    /// empty) the test needs.
    fn i35_awaiting_record(id: &str, workspace: &std::path::Path) -> rupu_orchestrator::RunRecord {
        use rupu_orchestrator::runs::AwaitingGate;
        let now = chrono::Utc::now();
        let mut rec = rupu_orchestrator::RunRecord {
            id: id.into(),
            workflow_name: "g".into(),
            status: RunStatus::AwaitingApproval,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: workspace.to_path_buf(),
            transcript_dir: workspace.join(".rupu/transcripts"),
            started_at: now,
            finished_at: None,
            final_output: None,
            error_message: None,
            awaiting: vec![AwaitingGate {
                step_id: "gate".into(),
                prompt: Some("Approve?".into()),
                since: now,
                expires_at: None,
            }],
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            loop_progress: Default::default(),
        };
        rec.sync_awaiting_compat();
        rec
    }

    /// Test 1: reject via the WEB API handler (`POST /api/runs/:id/reject`
    /// against a real spawned `rupu-cp` server), not the CLI. The web
    /// handler must leave the chain to the sweep; the sweep must then
    /// actually run it (real filesystem side effect) AND record the gate's
    /// rejected decision. Pre-fix, the marker file never appears no matter
    /// how many sweep ticks run, because `reject_gate` recorded no marker
    /// for the sweep to find.
    #[tokio::test(flavor = "multi_thread")]
    async fn web_reject_leaves_marker_sweep_runs_cleanup_and_records_gate_decision() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let (_tmp, home, workspace) = i35_fixture();
        let rec = i35_awaiting_record("run_i35_web_reject", &workspace);

        let app_state =
            rupu_cp::state::AppState::new(home.clone(), rupu_config::PricingConfig::default());
        app_state
            .run_store
            .create(rec.clone(), I35_WORKFLOW_NONEMPTY_CHAIN)
            .unwrap();

        let app = rupu_cp::server::router(app_state.clone(), None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = rupu_netflow::http::client_with(
            rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Cp),
            reqwest::Client::builder(),
            Arc::new(rupu_netflow::NullSink),
        )
        .expect("test client build");
        let resp = client
            .post(format!("http://{addr}/api/runs/{}/reject", rec.id))
            .json(&serde_json::json!({ "reason": "not today" }))
            .send()
            .await
            .expect("reject request should succeed");
        assert!(
            resp.status().is_success(),
            "POST /api/runs/:id/reject returned {}",
            resp.status()
        );

        // Immediately after the web reject: the run is terminally
        // Rejected, the cleanup-pending marker IS set, but the chain has
        // NOT run yet (no marker file, no gate StepResult) — the web
        // handler itself must not run a workflow runtime.
        let after_reject = app_state.run_store.load(&rec.id).unwrap();
        assert_eq!(after_reject.status, RunStatus::Rejected);
        assert!(
            after_reject.reject_cleanup_pending.is_some(),
            "web reject must leave the on_reject cleanup-pending marker for the sweep"
        );
        assert!(
            !workspace.join("reject_cleanup_marker.txt").exists(),
            "the web reject handler itself must not run the on_reject chain synchronously"
        );

        // Run the deferred worker for the marker above.
        let hosts = rupu_workspace::HostStore {
            root: home.join("hosts"),
        };
        let exe = std::env::current_exe().unwrap();
        std::env::set_var("RUPU_HOME", &home);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", I35_WRITE_SCRIPT);
        run_gate_sweep(
            Arc::clone(&app_state.run_store),
            hosts,
            exe,
            "test-worker".into(),
        )
        .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        assert!(
            workspace.join("reject_cleanup_marker.txt").exists(),
            "the sweep must have run the on_reject chain for real"
        );
        let after_sweep = app_state.run_store.load(&rec.id).unwrap();
        assert!(
            after_sweep.reject_cleanup_pending.is_none(),
            "the sweep must clear the marker once cleanup succeeds"
        );
        let step_results = app_state.run_store.read_step_results(&rec.id).unwrap();
        let gate_record = step_results
            .iter()
            .find(|r| r.step_id == "gate")
            .expect("the gate's rejected decision must be recorded");
        let gate_output: serde_json::Value = serde_json::from_str(&gate_record.output).unwrap();
        assert_eq!(gate_output["decision"], "rejected");
    }

    /// Test 2: `RunStore::cancel` on an `AwaitingApproval` run — the path
    /// behind BOTH `rupu workflow cancel` and the CP web `/cancel` control
    /// — must reach the same deferred-cleanup fate as an explicit reject:
    /// `cancel` rejects internally (`RunStore::reject`), which is the same
    /// `reject_gate` explicit-reject branch the web reject handler above
    /// goes through, so the marker + sweep mechanism covers it "for free".
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_on_awaiting_approval_run_leaves_marker_sweep_runs_cleanup() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let (_tmp, home, workspace) = i35_fixture();
        let store = Arc::new(rupu_orchestrator::RunStore::new(home.join("runs")));
        let rec = i35_awaiting_record("run_i35_cancel", &workspace);
        store
            .create(rec.clone(), I35_WORKFLOW_NONEMPTY_CHAIN)
            .unwrap();

        let outcome = store
            .cancel(
                &rec.id,
                "operator",
                "cancelled by operator",
                chrono::Utc::now(),
            )
            .expect("cancel of an AwaitingApproval run should succeed");
        assert!(matches!(
            outcome,
            rupu_orchestrator::runs::CancelOutcome::RejectedAwaitingApproval
        ));

        let after_cancel = store.load(&rec.id).unwrap();
        assert_eq!(after_cancel.status, RunStatus::Rejected);
        assert!(
            after_cancel.reject_cleanup_pending.is_some(),
            "cancelling a paused run must leave the on_reject cleanup-pending marker for the sweep"
        );
        assert!(!workspace.join("reject_cleanup_marker.txt").exists());

        let hosts = rupu_workspace::HostStore {
            root: home.join("hosts"),
        };
        let exe = std::env::current_exe().unwrap();
        std::env::set_var("RUPU_HOME", &home);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", I35_WRITE_SCRIPT);
        run_gate_sweep(Arc::clone(&store), hosts, exe, "test-worker".into()).await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        assert!(
            workspace.join("reject_cleanup_marker.txt").exists(),
            "the sweep must have run the cancelled run's on_reject chain for real"
        );
        assert!(store
            .load(&rec.id)
            .unwrap()
            .reject_cleanup_pending
            .is_none());
        let step_results = store.read_step_results(&rec.id).unwrap();
        assert!(step_results.iter().any(|r| r.step_id == "gate"));
    }

    /// Test 3: a rejected run with an EMPTY `on_reject` chain must not
    /// spin — `run_reject_cleanup` unconditionally records the gate's
    /// decision even with zero chain steps, so the sweep clears the marker
    /// on its very first tick. A second tick must be a silent no-op: no
    /// error, and — the concrete, checkable proof nothing re-ran — the
    /// gate's `StepResult` is not duplicated in `step_results.jsonl`.
    #[tokio::test(flavor = "multi_thread")]
    async fn empty_on_reject_chain_clears_marker_and_does_not_reprocess_on_next_tick() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let (_tmp, home, workspace) = i35_fixture();
        let store = Arc::new(rupu_orchestrator::RunStore::new(home.join("runs")));
        let rec = i35_awaiting_record("run_i35_empty_chain", &workspace);
        store.create(rec.clone(), I35_WORKFLOW_EMPTY_CHAIN).unwrap();

        store
            .reject_gate(&rec.id, "web", "not needed", chrono::Utc::now(), None)
            .expect("reject should succeed");
        assert!(store
            .load(&rec.id)
            .unwrap()
            .reject_cleanup_pending
            .is_some());

        let hosts = rupu_workspace::HostStore {
            root: home.join("hosts"),
        };
        let exe = std::env::current_exe().unwrap();
        std::env::set_var("RUPU_HOME", &home);

        // Tick 1: clears the marker and records the gate's decision even
        // though the chain has zero steps.
        run_gate_sweep(
            Arc::clone(&store),
            hosts.clone(),
            exe.clone(),
            "test-worker".into(),
        )
        .await;
        let after_first = store.load(&rec.id).unwrap();
        assert!(
            after_first.reject_cleanup_pending.is_none(),
            "an empty on_reject chain must still clear the marker on the first tick"
        );
        let after_first_results = store.read_step_results(&rec.id).unwrap();
        let gate_rows = after_first_results
            .iter()
            .filter(|r| r.step_id == "gate")
            .count();
        assert_eq!(
            gate_rows, 1,
            "the gate's decision must be recorded exactly once"
        );

        // Tick 2: the marker is gone, so this must be a silent no-op — no
        // second "gate" row appended (the pre-fix-shaped failure mode this
        // guards against: a marker that keeps matching and re-running the
        // chain every tick because the sweep matched on `status ==
        // Rejected` instead of the marker).
        run_gate_sweep(Arc::clone(&store), hosts, exe, "test-worker".into()).await;
        std::env::remove_var("RUPU_HOME");

        let after_second_results = store.read_step_results(&rec.id).unwrap();
        let gate_rows_after_second = after_second_results
            .iter()
            .filter(|r| r.step_id == "gate")
            .count();
        assert_eq!(
            gate_rows_after_second, 1,
            "a second sweep tick must not reprocess an already-cleared marker"
        );
    }

    // ── Task 9: automatic ASN table refresh on the sweep tick ──

    #[test]
    fn asn_refresh_is_skipped_when_auto_refresh_is_off() {
        let cfg = rupu_config::NetflowConfig {
            asn_auto_refresh: false,
            ..Default::default()
        };
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!should_refresh_asn(&cfg, &tmp.path().join("asn.db")));
    }

    #[test]
    fn asn_refresh_is_requested_when_the_table_is_missing() {
        let cfg = rupu_config::NetflowConfig::default();
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(should_refresh_asn(&cfg, &tmp.path().join("asn.db")));
    }

    #[test]
    fn asn_refresh_is_skipped_for_a_fresh_table() {
        let cfg = rupu_config::NetflowConfig::default();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("asn.db");
        std::fs::write(&path, b"{}").unwrap();
        assert!(!should_refresh_asn(&cfg, &path));
    }

    // ── Fix round 1, Finding 1: single-flight guard against overlapping
    // downloads racing on AsnTable::write's fixed temp filename ──

    #[test]
    fn asn_refresh_single_flight_guard_admits_only_one_claimant_until_released() {
        // A fresh, local guard — never touches the process-wide static —
        // so this test has no interference with any other test in the
        // binary regardless of parallel test execution.
        let guard = AtomicBool::new(false);
        assert!(
            try_claim_asn_refresh(&guard),
            "first claim while unclaimed must succeed"
        );
        assert!(
            !try_claim_asn_refresh(&guard),
            "a second claim while one refresh is in flight must fail — this is exactly \
             the race that would otherwise let two overlapping downloads corrupt \
             AsnTable::write's shared <path>.db.tmp"
        );

        // Release on the FAILURE path must also free the guard — the
        // production spawn releases on both Ok and Err arms so a failed
        // refresh can never wedge future refreshes shut.
        guard.store(false, Ordering::Release);
        assert!(
            try_claim_asn_refresh(&guard),
            "claiming must succeed again once released"
        );
    }

    // ── Fix round 1, Finding 2: gate_sweep_enabled must not silently gate
    // the ASN refresh check ──

    #[test]
    fn sweep_loop_stays_enabled_for_asn_refresh_when_gate_sweep_is_disabled() {
        let cp_cfg = rupu_config::CpConfig {
            gate_sweep_enabled: false,
            ..Default::default()
        };
        let netflow_cfg = rupu_config::NetflowConfig {
            asn_auto_refresh: true,
            ..Default::default()
        };
        assert!(
            sweep_loop_enabled(&cp_cfg, &netflow_cfg),
            "an operator who disables gate-timeout enforcement but leaves ASN \
             auto-refresh on (the default for both) must still reach the ASN \
             refresh check every tick"
        );
    }

    #[test]
    fn sweep_loop_disables_when_both_gate_sweep_and_asn_auto_refresh_are_off() {
        let cp_cfg = rupu_config::CpConfig {
            gate_sweep_enabled: false,
            ..Default::default()
        };
        let netflow_cfg = rupu_config::NetflowConfig {
            asn_auto_refresh: false,
            ..Default::default()
        };
        assert!(!sweep_loop_enabled(&cp_cfg, &netflow_cfg));
    }
}
