//! GitHub connectors.

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};

/// Try to build the GitHub Repo + Issue connectors from configured
/// credentials. Returns `Ok(None)` when no GitHub credential is
/// stored — that's a normal "user hasn't logged in" case, not an
/// error. Real implementation lands in Tasks 11-12; for now this
/// always returns `Ok(None)`.
pub async fn try_build(
    _resolver: &dyn CredentialResolver,
    _cfg: &Config,
) -> Result<Option<(Arc<dyn RepoConnector>, Arc<dyn IssueConnector>)>> {
    Ok(None)
}
