//! `rupu cp` — control-plane HTTP server subcommand.

use crate::paths;
use clap::Subcommand;
use rupu_orchestrator::runs::RunStore;
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
            let worker_handle =
                tokio::spawn(run_resume_worker(Arc::clone(&store), worker_id, shutdown_rx));

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

            // Adapter for rupu-cp's SessionSender port: shells
            // `rupu session send <id> "<prompt>" --detach` using this same
            // binary, reusing the launcher's resolved exe.
            let session_sender: Arc<dyn rupu_cp::session_sender::SessionSender> =
                Arc::new(crate::cp_session_sender::SubprocessSessionSender { exe });

            // Repo lister for the web Run target picker.
            let repos: Option<Arc<dyn rupu_cp::repos::RepoLister>> = {
                let resolver = rupu_auth::KeychainResolver::new();
                let global_cfg = global_dir.join("config.toml");
                let cfg =
                    rupu_config::layer_files(Some(&global_cfg), None).unwrap_or_default();
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
            })
            .await;

            // Signal the worker to stop and wait for it to drain.
            let _ = shutdown_tx.send(true);
            let _ = worker_handle.await;

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

        for run in pending {
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
