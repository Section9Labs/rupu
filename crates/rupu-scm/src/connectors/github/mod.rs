//! GitHub connectors.

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};

mod client;
mod issues;
mod repo;

pub use client::{classify_octocrab_error, GithubClient};

/// Try to build the GitHub Repo + Issue connectors from configured
/// credentials. Returns `Ok(None)` when no GitHub credential is
/// stored — that's a normal "user hasn't logged in" case.
pub async fn try_build(
    resolver: &dyn CredentialResolver,
    cfg: &Config,
) -> Result<Option<(Arc<dyn RepoConnector>, Arc<dyn IssueConnector>)>> {
    let creds = match resolver.get("github", None).await {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let platform_cfg = cfg.scm.platforms.get("github");
    let base_url = platform_cfg.and_then(|p| p.base_url.clone());
    let max_conc = platform_cfg.and_then(|p| p.max_concurrency);
    let client = GithubClient::new(token, base_url, max_conc);
    let repo: Arc<dyn RepoConnector> = Arc::new(repo::GithubRepoConnector::new(client.clone()));
    let issues: Arc<dyn IssueConnector> = Arc::new(issues::GithubIssueConnector::new(client));
    Ok(Some((repo, issues)))
}
