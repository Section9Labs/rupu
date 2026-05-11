//! Linear event connector.

pub mod events;
pub mod issues;

pub use events::LinearEventConnector;
pub use issues::LinearIssueConnector;

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::IssueConnector;

/// Try to build the Linear Issue connector from configured credentials.
/// Returns `Ok(None)` when no Linear credential is present.
pub async fn try_build(
    resolver: &dyn CredentialResolver,
    cfg: &Config,
) -> Result<Option<Arc<dyn IssueConnector>>> {
    let creds = match resolver
        .get("linear", Some(rupu_providers::AuthMode::ApiKey))
        .await
    {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let base_url = cfg
        .scm
        .platforms
        .get("linear")
        .and_then(|platform| platform.base_url.clone());
    Ok(Some(Arc::new(LinearIssueConnector::new(token, base_url))))
}
