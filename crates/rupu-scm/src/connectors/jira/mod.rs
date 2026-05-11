//! Jira connectors.

pub mod events;
pub mod issues;

pub use events::JiraEventConnector;
pub use issues::JiraIssueConnector;

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::IssueConnector;

/// Try to build the Jira Issue connector from configured credentials.
/// Returns `Ok(None)` when no Jira credential is present.
pub async fn try_build(
    resolver: &dyn CredentialResolver,
    cfg: &Config,
) -> Result<Option<Arc<dyn IssueConnector>>> {
    let creds = match resolver
        .get("jira", Some(rupu_providers::AuthMode::ApiKey))
        .await
    {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let base_url = cfg
        .scm
        .platforms
        .get("jira")
        .and_then(|platform| platform.base_url.clone());
    Ok(Some(Arc::new(JiraIssueConnector::new(creds, base_url)?)))
}
