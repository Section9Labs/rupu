//! `rupu webhook serve`. Long-running HTTP receiver for SCM events.
//!
//! Secrets come from env vars (not config files, not the keychain —
//! webhook secrets are operational secrets that ought to be in
//! whatever process supervisor / systemd-unit / cron environment is
//! running this command):
//!
//!   RUPU_GITHUB_WEBHOOK_SECRET   (HMAC-SHA256 secret for GitHub)
//!   RUPU_GITLAB_WEBHOOK_TOKEN    (shared-secret token for GitLab)
//!
//! Either may be unset; the corresponding endpoint then returns
//! 503 (service-unavailable) so the operator knows the route is
//! intentionally disabled rather than misconfigured.

use crate::paths;
use async_trait::async_trait;
use clap::Subcommand;
use rupu_orchestrator::Workflow;
use rupu_webhook::{serve, DispatchOutcome, WebhookConfig, WorkflowDispatcher};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::warn;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Run the webhook HTTP receiver in the foreground.
    Serve {
        /// Address to bind. Defaults to 127.0.0.1:8080. Use 0.0.0.0
        /// to expose externally (only do this behind a reverse proxy
        /// that terminates TLS — rupu does not).
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: SocketAddr,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Serve { addr } => serve_cmd(addr).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu webhook: {e}");
            ExitCode::from(1)
        }
    }
}

async fn serve_cmd(addr: SocketAddr) -> anyhow::Result<()> {
    let github_secret = std::env::var("RUPU_GITHUB_WEBHOOK_SECRET")
        .ok()
        .map(|s| s.into_bytes());
    let gitlab_token = std::env::var("RUPU_GITLAB_WEBHOOK_TOKEN")
        .ok()
        .map(|s| s.into_bytes());
    if github_secret.is_none() && gitlab_token.is_none() {
        anyhow::bail!(
            "neither RUPU_GITHUB_WEBHOOK_SECRET nor RUPU_GITLAB_WEBHOOK_TOKEN is set; \
             at least one webhook endpoint must be configured"
        );
    }

    let config = WebhookConfig {
        addr,
        github_secret,
        gitlab_token,
        workflow_loader: Arc::new(load_workflows),
        dispatcher: Arc::new(CliDispatcher),
    };
    serve(config).await?;
    Ok(())
}

/// Production [`WorkflowDispatcher`] — invokes the same code path as
/// `rupu workflow run <name>` for matched workflows. Failures are
/// returned to the receiver, which records them in the JSON
/// response so operators can see what went wrong without tailing
/// logs.
struct CliDispatcher;

#[async_trait]
impl WorkflowDispatcher for CliDispatcher {
    async fn dispatch(
        &self,
        workflow_name: &str,
        event: &serde_json::Value,
    ) -> anyhow::Result<DispatchOutcome> {
        let summary =
            super::workflow::run_by_name(workflow_name, Vec::new(), None, Some(event.clone()))
                .await?;
        Ok(DispatchOutcome {
            run_id: summary.run_id,
            awaiting_step_id: summary.awaiting_step_id,
        })
    }
}

/// Walk global + project workflow directories and return every
/// successfully-parsed workflow. Called fresh per request so authors
/// can edit workflow files without restarting the receiver.
fn load_workflows() -> Vec<(String, Workflow)> {
    let global = match paths::global_dir() {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "global dir lookup failed");
            return Vec::new();
        }
    };
    let pwd = std::env::current_dir().ok();
    let project_root = pwd
        .as_deref()
        .and_then(|p| paths::project_root_for(p).ok().flatten());

    let mut by_name: BTreeMap<String, Workflow> = BTreeMap::new();
    push_workflows(&global.join("workflows"), &mut by_name);
    if let Some(p) = &project_root {
        push_workflows(&p.join(".rupu/workflows"), &mut by_name);
    }
    by_name.into_iter().collect()
}

fn push_workflows(dir: &Path, into: &mut BTreeMap<String, Workflow>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(wf) = Workflow::parse(&body) else {
            continue;
        };
        into.insert(stem.to_string(), wf);
    }
}
