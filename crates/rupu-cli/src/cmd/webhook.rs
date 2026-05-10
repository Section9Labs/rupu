//! `rupu webhook serve`. Long-running HTTP receiver for SCM events.
//!
//! Secrets come from env vars (not config files, not the keychain —
//! webhook secrets are operational secrets that ought to be in
//! whatever process supervisor / systemd-unit / cron environment is
//! running this command):
//!
//!   RUPU_GITHUB_WEBHOOK_SECRET   (HMAC-SHA256 secret for GitHub)
//!   RUPU_GITLAB_WEBHOOK_TOKEN    (shared-secret token for GitLab)
//!   RUPU_LINEAR_WEBHOOK_SECRET   (HMAC-SHA256 secret for Linear)
//!
//! Any may be unset; the corresponding endpoint then returns
//! 503 (service-unavailable) so the operator knows the route is
//! intentionally disabled rather than misconfigured.

use super::autoflow_wake::wake_requests_from_webhook;
use crate::paths;
use async_trait::async_trait;
use clap::Subcommand;
use rupu_orchestrator::Workflow;
use rupu_runtime::{WakeStore, WakeStoreError};
use rupu_webhook::{
    serve, DispatchOutcome, WebhookConfig, WebhookEvent, WebhookObserver, WorkflowDispatcher,
};
use rupu_workspace::RepoRegistryStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
    let linear_secret = std::env::var("RUPU_LINEAR_WEBHOOK_SECRET")
        .ok()
        .map(|s| s.into_bytes());
    if github_secret.is_none() && gitlab_token.is_none() && linear_secret.is_none() {
        anyhow::bail!(
            "none of RUPU_GITHUB_WEBHOOK_SECRET, RUPU_GITLAB_WEBHOOK_TOKEN, or \
             RUPU_LINEAR_WEBHOOK_SECRET is set; \
             at least one webhook endpoint must be configured"
        );
    }

    let config = WebhookConfig {
        addr,
        github_secret,
        gitlab_token,
        linear_secret,
        workflow_loader: Arc::new(load_workflows),
        dispatcher: Arc::new(CliDispatcher),
        observer: Some(Arc::new(CliWebhookObserver { global })),
    };
    serve(config).await?;
    Ok(())
}

/// Production [`WorkflowDispatcher`] — dispatches the exact resolved
/// workflow candidate for the incoming event. Failures are returned to
/// the receiver, which records them in the JSON response so operators
/// can see what went wrong without tailing logs.
struct CliDispatcher;

#[async_trait]
impl WorkflowDispatcher for CliDispatcher {
    async fn dispatch(
        &self,
        workflow_key: &str,
        event: &serde_json::Value,
    ) -> anyhow::Result<DispatchOutcome> {
        let dispatch = decode_dispatch_key(workflow_key)?;
        let summary = super::workflow::run_by_path(
            dispatch.workflow_path,
            dispatch.project_root,
            dispatch.workspace_path,
            Vec::new(),
            None,
            Some(event.clone()),
        )
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
        let Some(requests) = wake_requests_from_webhook(event) else {
            return Ok(());
        };
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&self.global),
        };
        let Some(first_request) = requests.first() else {
            return Ok(());
        };
        if repo_store.load(&first_request.repo_ref)?.is_none() {
            return Ok(());
        }
        let store = WakeStore::new(paths::autoflow_wakes_dir(&self.global));
        for request in requests {
            match store.enqueue(request) {
                Ok(_) => {}
                Err(WakeStoreError::DuplicateDedupeKey(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }
}

/// Walk global + visible tracked-repo workflow directories and return
/// every successfully-parsed workflow candidate. Called fresh per
/// request so authors can edit workflow files without restarting the
/// receiver.
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
    let default_workspace = pwd.clone().unwrap_or_else(|| global.clone());
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };

    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    push_workflows(
        &global.join("workflows"),
        None,
        default_workspace.clone(),
        &mut seen,
        &mut candidates,
    );
    if let Some(p) = &project_root {
        push_workflows(
            &p.join(".rupu/workflows"),
            Some(p.clone()),
            p.clone(),
            &mut seen,
            &mut candidates,
        );
    }
    if let Ok(tracked) = repo_store.list() {
        for repo in tracked {
            let preferred_checkout = PathBuf::from(&repo.preferred_path);
            if !preferred_checkout.exists() {
                continue;
            }
            let tracked_project_root = paths::project_root_for(&preferred_checkout)
                .ok()
                .flatten()
                .or_else(|| {
                    preferred_checkout
                        .join(".rupu")
                        .is_dir()
                        .then_some(preferred_checkout.clone())
                });
            let Some(tracked_project_root) = tracked_project_root else {
                continue;
            };
            push_workflows(
                &tracked_project_root.join(".rupu/workflows"),
                Some(tracked_project_root.clone()),
                preferred_checkout,
                &mut seen,
                &mut candidates,
            );
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    candidates
}

fn push_workflows(
    dir: &Path,
    project_root: Option<PathBuf>,
    workspace_path: PathBuf,
    seen: &mut BTreeSet<PathBuf>,
    into: &mut Vec<(String, Workflow)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let ext = p.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let canonical = match p.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let body = match std::fs::read_to_string(&canonical) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(wf) = Workflow::parse(&body) else {
            continue;
        };
        let Ok(key) = encode_dispatch_key(DispatchKey {
            workflow_path: canonical,
            project_root: project_root.clone(),
            workspace_path: workspace_path.clone(),
        }) else {
            continue;
        };
        into.push((key, wf));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DispatchKey {
    workflow_path: PathBuf,
    #[serde(default)]
    project_root: Option<PathBuf>,
    workspace_path: PathBuf,
}

fn encode_dispatch_key(key: DispatchKey) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&key)?)
}

fn decode_dispatch_key(value: &str) -> anyhow::Result<DispatchKey> {
    Ok(serde_json::from_str(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
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
        let tracked_event = WebhookEvent {
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
        };
        observer.observe(&tracked_event).await.unwrap();
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

        let queued = WakeStore::new(paths::autoflow_wakes_dir(&global))
            .list_due(chrono::Utc::now())
            .unwrap();
        let expected = crate::cmd::autoflow_wake::wake_requests_from_webhook(&tracked_event)
            .unwrap()
            .len();
        assert_eq!(queued.len(), expected);
        assert!(queued
            .iter()
            .all(|wake| wake.repo_ref == "github:Section9Labs/rupu"));
    }

    #[tokio::test]
    async fn observer_dedupes_replayed_webhook_deliveries() {
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
        let replay = WebhookEvent {
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
        };

        observer.observe(&replay).await.unwrap();
        observer.observe(&replay).await.unwrap();

        let queued = WakeStore::new(paths::autoflow_wakes_dir(&global))
            .list_due(chrono::Utc::now())
            .unwrap();
        let expected = crate::cmd::autoflow_wake::wake_requests_from_webhook(&replay)
            .unwrap()
            .len();
        assert_eq!(queued.len(), expected);
        assert!(queued
            .iter()
            .all(|wake| wake.event.delivery_id.as_deref() == Some("delivery-123")));
    }

    #[test]
    fn dispatch_key_round_trips() {
        let key = DispatchKey {
            workflow_path: PathBuf::from("/tmp/rupu/.rupu/workflows/review.yaml"),
            project_root: Some(PathBuf::from("/tmp/rupu")),
            workspace_path: PathBuf::from("/tmp/rupu"),
        };
        let encoded = encode_dispatch_key(key.clone()).unwrap();
        assert_eq!(decode_dispatch_key(&encoded).unwrap(), key);
    }

    #[test]
    fn load_workflows_includes_tracked_repo_project_workflows() {
        let _guard = ENV_LOCK.blocking_lock();
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let tracked_repo = tmp.path().join("tracked");
        std::fs::create_dir_all(tracked_repo.join(".rupu/workflows")).unwrap();
        std::fs::write(
            tracked_repo.join(".rupu/workflows/review.yaml"),
            r#"name: repo-review
trigger:
  on: event
  event: github.pr.opened
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
"#,
        )
        .unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        std::fs::create_dir_all(&global).unwrap();
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &tracked_repo,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();

        let old_home = std::env::var_os("RUPU_HOME");
        let old_cwd = std::env::current_dir().unwrap();
        std::env::set_var("RUPU_HOME", &global);
        std::env::set_current_dir(tmp.path()).unwrap();

        let loaded = load_workflows();

        std::env::set_current_dir(old_cwd).unwrap();
        match old_home {
            Some(value) => std::env::set_var("RUPU_HOME", value),
            None => std::env::remove_var("RUPU_HOME"),
        }

        let (_, workflow) = loaded
            .iter()
            .find(|(_, workflow)| workflow.name == "repo-review")
            .expect("tracked repo workflow");
        assert_eq!(workflow.name, "repo-review");
    }
}
