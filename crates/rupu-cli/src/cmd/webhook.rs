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

use super::autoflow_wake::{store_webhook_wake_event, wake_event_from_webhook};
use crate::paths;
use async_trait::async_trait;
use clap::Subcommand;
use rupu_orchestrator::Workflow;
use rupu_webhook::{
    serve, DispatchOutcome, WebhookConfig, WebhookEvent, WebhookObserver, WorkflowDispatcher,
};
use rupu_workspace::RepoRegistryStore;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
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
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn serve_cmd(addr: SocketAddr) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
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
        observer: Some(Arc::new(CliWebhookObserver { global })),
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

struct CliWebhookObserver {
    global: PathBuf,
}

#[async_trait]
impl WebhookObserver for CliWebhookObserver {
    async fn observe(&self, event: &WebhookEvent) -> anyhow::Result<()> {
        let Some(stored) = wake_event_from_webhook(event) else {
            return Ok(());
        };
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&self.global),
        };
        if repo_store.load(&stored.repo_ref)?.is_none() {
            return Ok(());
        }
        let root = paths::autoflow_webhook_events_dir(&self.global);
        store_webhook_wake_event(&root, &stored)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_webhook::{WebhookEvent, WebhookSource};
    use serde_json::json;

    #[tokio::test]
    async fn observer_queues_only_tracked_repo_events() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        let repo_path = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &repo_path,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();

        let observer = CliWebhookObserver {
            global: global.clone(),
        };
        observer
            .observe(&WebhookEvent {
                source: WebhookSource::Github,
                event_id: "github.issue.labeled".into(),
                delivery_id: Some("delivery-123".into()),
                payload: json!({
                    "issue": { "number": 42 },
                    "repository": {
                        "name": "rupu",
                        "owner": { "login": "Section9Labs" }
                    }
                }),
            })
            .await
            .unwrap();
        observer
            .observe(&WebhookEvent {
                source: WebhookSource::Github,
                event_id: "github.issue.labeled".into(),
                delivery_id: Some("delivery-456".into()),
                payload: json!({
                    "issue": { "number": 7 },
                    "repository": {
                        "name": "other",
                        "owner": { "login": "Section9Labs" }
                    }
                }),
            })
            .await
            .unwrap();

        let queued = std::fs::read_dir(paths::autoflow_webhook_events_dir(&global))
            .unwrap()
            .flatten()
            .count();
        assert_eq!(queued, 1);
    }
}
