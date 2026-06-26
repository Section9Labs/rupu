//! `rupu cp` — control-plane HTTP server subcommand.

use crate::paths;
use crate::resume;
use clap::Subcommand;
use rupu_orchestrator::runs::{ApprovalDecision, RunStore};
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

            let serve_result = rupu_cp::serve(rupu_cp::ServeOpts {
                bind,
                token,
                global_dir,
                open_browser: !no_open,
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
/// process has no execution runtime). This worker — running inside the same
/// `cp serve` process, which *does* have the full runtime — polls for marked
/// runs, claims each via a 5-minute lease, approves it (phase 1), and
/// re-enters `run_workflow` via [`resume::resume_run`] (phase 2). The marker
/// and claim are cleared afterward regardless of outcome so a poisoned run
/// is not retried forever.
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

            // Spawn the resume so a long-running workflow doesn't block the
            // poll loop. Move owned/clonable data into the task.
            let store = Arc::clone(&store);
            let worker_id = worker_id.clone();
            let run_id = run.id.clone();
            tokio::spawn(async move {
                let now2 = chrono::Utc::now();
                // Capture the requested resume mode while the marker is still
                // present (approve may not preserve `resume_mode`); fall back
                // to None if the record can't be reloaded.
                let mode = store.load(&run_id).ok().and_then(|r| r.resume_mode.clone());
                let decision = match store.approve(&run_id, &worker_id, now2) {
                    Ok(d) => d,
                    Err(e) => {
                        // e.g. the run was rejected concurrently → NotAwaiting.
                        tracing::warn!(run_id = %run_id, error = %e, "resume worker: approve failed; clearing marker");
                        if let Err(ce) = store.clear_resume(&run_id, now2) {
                            tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                        }
                        return;
                    }
                };

                if let ApprovalDecision::Approved { step_id, .. } = decision {
                    tracing::info!(run_id = %run_id, step_id = %step_id, "resume worker: approved; resuming");
                    match resume::resume_run(&store, &run_id, &step_id, mode.as_deref()).await {
                        Ok(_) => {
                            tracing::info!(run_id = %run_id, "resume worker: resume completed");
                        }
                        Err(e) => {
                            // The run is left in whatever state resume
                            // persisted (e.g. Failed). Do NOT retry.
                            tracing::error!(run_id = %run_id, error = %e, "resume worker: resume failed");
                        }
                    }
                } else {
                    tracing::warn!(
                        run_id = %run_id,
                        "resume worker: approve returned non-Approved decision; clearing marker"
                    );
                }

                // Clear marker+claim regardless of resume outcome so a
                // poisoned run isn't retried forever.
                if let Err(ce) = store.clear_resume(&run_id, now2) {
                    tracing::warn!(run_id = %run_id, error = %ce, "resume worker: clear_resume failed");
                } else {
                    tracing::info!(run_id = %run_id, "resume worker: cleared resume marker");
                }
            });
        }
    }
}
