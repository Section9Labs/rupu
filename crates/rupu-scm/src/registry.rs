//! Registry that discovers and manages connectors for configured platforms.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, warn};

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::github::extras::GithubExtras;
use crate::connectors::gitlab::extras::GitlabExtras;
use crate::connectors::{IssueConnector, RepoConnector};
use crate::event_connector::EventConnector;
use crate::platform::{IssueTracker, Platform};
use crate::types::EventSourceRef;

/// Registry that builds connectors from configured credentials.
/// Connectors are discovered once during [`discover`] and cached
/// in hashmaps keyed by platform/tracker.
///
/// [`discover`]: Self::discover
#[derive(Default)]
pub struct Registry {
    repo_connectors: HashMap<Platform, Arc<dyn RepoConnector>>,
    issue_connectors: HashMap<IssueTracker, Arc<dyn IssueConnector>>,
    event_connectors: HashMap<Platform, Arc<dyn EventConnector>>,
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

        // GitHub events (separate path because it uses reqwest directly
        // rather than octocrab; same credential resolver). No-op when
        // GitHub credentials are absent.
        match crate::connectors::github::events::try_build(resolver, cfg).await {
            Ok(Some(c)) => {
                reg.event_connectors.insert(Platform::Github, c);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "github events: connector build failed; skipping");
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

        // GitLab events.
        match crate::connectors::gitlab::events::try_build(resolver, cfg).await {
            Ok(Some(c)) => {
                reg.event_connectors.insert(Platform::Gitlab, c);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "gitlab events: connector build failed; skipping");
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

    /// Retrieve the EventConnector for a given platform, if one is
    /// registered. Used by `rupu cron tick`'s polled-events tier.
    pub fn events(&self, p: Platform) -> Option<Arc<dyn EventConnector>> {
        self.event_connectors.get(&p).cloned()
    }

    /// Retrieve the EventConnector suitable for a trigger source.
    pub fn events_for_source(&self, source: &EventSourceRef) -> Option<Arc<dyn EventConnector>> {
        match source {
            EventSourceRef::Repo { repo } => self.events(repo.platform),
            EventSourceRef::TrackerProject { tracker, .. } => match tracker {
                IssueTracker::Github => self.events(Platform::Github),
                IssueTracker::Gitlab => self.events(Platform::Gitlab),
                IssueTracker::Linear | IssueTracker::Jira => None,
            },
        }
    }

    /// Test/internal: register an `EventConnector` directly.
    /// Discovery wires the GitHub + GitLab impls from
    /// `connectors::github::events::build` /
    /// `connectors::gitlab::events::build` once those land.
    pub fn insert_event_connector(&mut self, p: Platform, c: Arc<dyn EventConnector>) {
        self.event_connectors.insert(p, c);
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

    /// Return the default platform for tools that omit the `platform`
    /// argument. Prefers GitHub, then GitLab. Wiring to `[scm.default]`
    /// config lands in Task 19; this is the v0 "first registered" fallback.
    pub fn default_platform(&self) -> Option<Platform> {
        if self.repo_connectors.contains_key(&Platform::Github) {
            Some(Platform::Github)
        } else if self.repo_connectors.contains_key(&Platform::Gitlab) {
            Some(Platform::Gitlab)
        } else {
            None
        }
    }

    /// Return the default issue tracker for tools that omit the `tracker`
    /// argument. Prefers GitHub, then GitLab.
    pub fn default_tracker(&self) -> Option<IssueTracker> {
        if self.issue_connectors.contains_key(&IssueTracker::Github) {
            Some(IssueTracker::Github)
        } else if self.issue_connectors.contains_key(&IssueTracker::Gitlab) {
            Some(IssueTracker::Gitlab)
        } else {
            None
        }
    }

    /// Test-only: build a Registry with no connectors. Tools that
    /// require a connector return McpError::NotWiredInV0 — they do
    /// NOT panic. Honors the "no mock features" rule: the absence
    /// of a connector is reported, not silently ignored.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn empty() -> Self {
        Self::default()
    }
}
