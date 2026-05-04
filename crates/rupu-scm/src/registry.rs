//! Registry that discovers and manages connectors for configured platforms.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::github::extras::GithubExtras;
use crate::connectors::gitlab::extras::GitlabExtras;
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
    github_extras: Option<Arc<GithubExtras>>,
    gitlab_extras: Option<Arc<GitlabExtras>>,
}

impl Registry {
    /// Discover connectors from configured credentials. Each platform
    /// is probed independently; missing credentials are logged at INFO
    /// level and skipped. Errors during build are logged at WARN level
    /// and also skipped — the registry continues with whatever succeeded.
    pub async fn discover(resolver: &dyn CredentialResolver, cfg: &Config) -> Self {
        let mut reg = Self::default();

        // GitHub
        match crate::connectors::github::try_build(resolver, cfg).await {
            Ok(Some((repo, issues, extras))) => {
                reg.repo_connectors.insert(Platform::Github, repo);
                reg.issue_connectors.insert(IssueTracker::Github, issues);
                reg.github_extras = Some(extras);
            }
            Ok(None) => {
                info!("github: no credentials configured; skipping connector");
            }
            Err(e) => {
                warn!(error = %e, "github: connector build failed; skipping");
            }
        }

        // GitLab
        match crate::connectors::gitlab::try_build(resolver, cfg).await {
            Ok(Some((repo, issues, extras))) => {
                reg.repo_connectors.insert(Platform::Gitlab, repo);
                reg.issue_connectors.insert(IssueTracker::Gitlab, issues);
                reg.gitlab_extras = Some(extras);
            }
            Ok(None) => {
                info!("gitlab: no credentials configured; skipping connector");
            }
            Err(e) => {
                warn!(error = %e, "gitlab: connector build failed; skipping");
            }
        }

        reg
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

    /// Returns the per-platform extras handle for GitHub actions, if
    /// GitHub credentials were present during discovery.
    pub fn github_extras(&self) -> Option<Arc<GithubExtras>> {
        self.github_extras.clone()
    }

    /// Returns the per-platform extras handle for GitLab pipeline
    /// triggers, if GitLab credentials were present during discovery.
    pub fn gitlab_extras(&self) -> Option<Arc<GitlabExtras>> {
        self.gitlab_extras.clone()
    }
}
