//! GitLab connector — implements RepoConnector + IssueConnector.

pub mod client;
pub mod events;
pub mod extras;
pub mod issues;
pub mod repo;

pub use client::GitlabClient;
pub use events::GitlabEventConnector;
pub use extras::GitlabExtras;
pub use issues::GitlabIssueConnector;
pub use repo::GitlabRepoConnector;

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};

/// Try to build the GitLab Repo + Issue connectors + extras handle from
/// configured credentials. Returns `Ok(None)` if no GitLab credential
/// is stored — that's a normal "user hasn't logged in" path.
pub async fn try_build(
    resolver: &dyn CredentialResolver,
    cfg: &Config,
) -> Result<
    Option<(
        Arc<dyn RepoConnector>,
        Arc<dyn IssueConnector>,
        Arc<GitlabExtras>,
    )>,
> {
    let creds = match resolver.get("gitlab", None).await {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let platform_cfg = cfg.scm.platforms.get("gitlab");
    let base_url = platform_cfg.and_then(|p| p.base_url.clone());
    let max_conc = platform_cfg.and_then(|p| p.max_concurrency);
    let client = GitlabClient::new(token, base_url, max_conc);
    let repo: Arc<dyn RepoConnector> = Arc::new(GitlabRepoConnector::new(client.clone()));
    let issues: Arc<dyn IssueConnector> = Arc::new(GitlabIssueConnector::new(client.clone()));
    let extras = Arc::new(GitlabExtras::new(client));
    Ok(Some((repo, issues, extras)))
}
