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
///
/// `account` is the configured account name (e.g. `gl-work`, or the bare
/// `"gitlab"` back-compat account) — both the credential-resolver lookup
/// key and the `[scm.<account>]` config-table key.
pub async fn try_build(
    account: &str,
    resolver: &dyn CredentialResolver,
    cfg: &Config,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> Result<
    Option<(
        Arc<dyn RepoConnector>,
        Arc<dyn IssueConnector>,
        Arc<GitlabExtras>,
    )>,
> {
    let creds = match resolver.get(account, None).await {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let opts = crate::client_options::ScmClientOptions::from_platform_config(
        cfg.scm.platforms.get(account),
    );
    let client = GitlabClient::with_options(token, &opts, sink);
    let repo: Arc<dyn RepoConnector> = Arc::new(GitlabRepoConnector::new(client.clone()));
    let issues: Arc<dyn IssueConnector> = Arc::new(GitlabIssueConnector::new(client.clone()));
    let extras = Arc::new(GitlabExtras::new(client));
    Ok(Some((repo, issues, extras)))
}
