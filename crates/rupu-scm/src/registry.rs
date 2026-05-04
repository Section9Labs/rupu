//! Registry that discovers and manages connectors for configured platforms.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};
use crate::platform::{IssueTracker, Platform};

/// Registry that builds connectors from configured credentials.
/// Connectors are discovered once during [`discover`] and cached
/// in hashmaps keyed by platform/tracker.
///
/// [`discover`]: Self::discover
#[derive(Default)]
pub struct Registry {
    repo_connectors: HashMap<Platform, Arc<dyn RepoConnector>>,
    issue_connectors: HashMap<IssueTracker, Arc<dyn IssueConnector>>,
}

impl Registry {
    /// Discover connectors from configured credentials. Each platform
    /// is probed independently; missing credentials are logged at INFO
    /// level and skipped. Errors during build are logged at WARN level
    /// and also skipped — the registry continues with whatever succeeded.
    pub async fn discover(resolver: &dyn CredentialResolver, cfg: &Config) -> Self {
        let mut registry = Self::default();

        // Try GitHub
        match crate::connectors::github::try_build(resolver, cfg).await {
            Ok(Some((repo, issues))) => {
                registry.repo_connectors.insert(Platform::Github, repo);
                registry
                    .issue_connectors
                    .insert(IssueTracker::Github, issues);
            }
            Ok(None) => {
                info!("GitHub connector: no credentials configured");
            }
            Err(e) => {
                warn!("GitHub connector build failed: {}", e);
            }
        }

        registry
    }

    /// Retrieve the RepoConnector for a given platform, if one is
    /// registered. Clones the Arc so the caller owns a reference.
    pub fn repo(&self, p: Platform) -> Option<Arc<dyn RepoConnector>> {
        self.repo_connectors.get(&p).cloned()
    }

    /// Retrieve the IssueConnector for a given tracker, if one is
    /// registered. Clones the Arc so the caller owns a reference.
    pub fn issues(&self, t: IssueTracker) -> Option<Arc<dyn IssueConnector>> {
        self.issue_connectors.get(&t).cloned()
    }
}
