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

            // Signal both background workers to stop and wait for them to drain.
            let _ = shutdown_tx.send(true);
            let _ = worker_handle.await;
            let _ = poller_handle.await;

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
                        r.worker_id.as_deref() == Some(host.id.as_str())
                            && !r.status.is_terminal()
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

            // Spawn the approve subprocess off-thread so claiming the next
            // run doesn't block on process creation. Move owned data in.
            let store = Arc::clone(&store);
            let run_id = run.id.clone();
            tokio::spawn(async move {
                let now2 = chrono::Utc::now();
                // Capture the requested resume mode while the marker is still
                // present, then hand the run off to a detached
                // `rupu workflow approve <run_id> [--mode <m>]` child. The
                // child does `store.approve` + the in-process resume in ITS
                // OWN process, so the resumed run is independently killable
                // (Cancel) and a resume crash can't take down `cp serve`.
                // The web marker leaves the run AwaitingApproval, so the
                // child's `store.approve` precondition holds.
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

                let mut argv: Vec<&str> = vec!["workflow", "approve", &run_id];
                if let Some(m) = mode.as_deref() {
                    argv.push("--mode");
                    argv.push(m);
                }

                match std::process::Command::new(&exe).args(&argv).spawn() {
                    Ok(_child) => {
                        // Detached: do NOT wait. The child now owns the run;
                        // clear the marker so we don't re-claim it.
                        tracing::info!(run_id = %run_id, "spawned workflow-approve subprocess to resume");
                        if let Err(ce) = store.clear_resume(&run_id, now2) {
                            tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                        } else {
                            tracing::info!(run_id = %run_id, "resume worker: cleared resume marker");
                        }
                    }
                    Err(e) => {
                        // Don't retry a poisoned spawn forever; clear marker.
                        tracing::error!(run_id = %run_id, error = %e, "resume worker: spawn workflow-approve failed; clearing marker");
                        if let Err(ce) = store.clear_resume(&run_id, now2) {
                            tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                        }
                    }
                }
            });
        }
    }
}
