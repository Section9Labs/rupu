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
    tracker_event_connectors: HashMap<IssueTracker, Arc<dyn EventConnector>>,
    github_extras: Option<Arc<GithubExtras>>,
    gitlab_extras: Option<Arc<GitlabExtras>>,
    /// Parsed `[scm.default].platform`, captured at `discover` time.
    configured_default_platform: Option<Platform>,
    /// Parsed `[issues.default].tracker`, captured at `discover` time.
    configured_default_tracker: Option<IssueTracker>,
}

/// Outcome of resolving a configured default (`[scm.default].platform` /
/// `[issues.default].tracker`) against the connectors actually
/// registered in a `Registry`. Kept as a pure, dependency-free helper
/// (see [`resolve_configured_default`]) so the "configured but no live
/// connector" case — which must log a WARN per ISSUES.md I-15 — can be
/// unit-tested without a tracing-capture harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultResolution<T> {
    /// A default was configured and a matching connector is registered.
    ConfiguredAndAvailable(T),
    /// A default was configured but no matching connector is registered
    /// (e.g. `platform = "gitlab"` with no GitLab credentials). Callers
    /// must warn here — silently falling back would be indistinguishable
    /// from the "nothing configured" case.
    ConfiguredButUnavailable(T),
    /// No default was configured; fall back silently.
    Unset,
}

/// Classify a configured default (`Some`/`None`) against an
/// availability check, without any logging side effect.
fn resolve_configured_default<T: Copy>(
    configured: Option<T>,
    is_available: impl FnOnce(T) -> bool,
) -> DefaultResolution<T> {
    match configured {
        Some(v) if is_available(v) => DefaultResolution::ConfiguredAndAvailable(v),
        Some(v) => DefaultResolution::ConfiguredButUnavailable(v),
        None => DefaultResolution::Unset,
    }
}

impl Registry {
    /// Discover connectors from configured credentials. Each platform
    /// is probed independently; missing credentials are logged at INFO
    /// level and skipped. Errors during build are logged at WARN level
    /// and also skipped — the registry continues with whatever succeeded.
    pub async fn discover(resolver: &dyn CredentialResolver, cfg: &Config) -> Self {
        // Capture `[scm.default]` / `[issues.default]` up front so
        // `default_platform`/`default_tracker` can consult them later
        // without needing the `Config` back in scope. An unset or
        // unparsable value just leaves the field `None`, which falls
        // back to the registration-order preference below.
        let configured_default_platform = cfg
            .scm
            .default
            .as_ref()
            .and_then(|d| d.platform.as_deref())
            .and_then(|s| s.parse::<Platform>().ok());
        let configured_default_tracker = cfg
            .issues
            .default
            .as_ref()
            .and_then(|d| d.tracker.as_deref())
            .and_then(|s| s.parse::<IssueTracker>().ok());

        let mut reg = Self {
            configured_default_platform,
            configured_default_tracker,
            ..Self::default()
        };

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

        // Linear events.
        match crate::connectors::linear::try_build(resolver, cfg).await {
            Ok(Some(issues)) => {
                reg.issue_connectors.insert(IssueTracker::Linear, issues);
            }
            Ok(None) => {
                info!("linear: no credentials configured; skipping issue connector");
            }
            Err(e) => {
                warn!(error = %e, "linear: issue connector build failed; skipping");
            }
        }

        // Linear events.
        match crate::connectors::linear::events::try_build(resolver, cfg).await {
            Ok(Some(c)) => {
                reg.tracker_event_connectors.insert(IssueTracker::Linear, c);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "linear events: connector build failed; skipping");
            }
        }

        // Jira issues.
        match crate::connectors::jira::try_build(resolver, cfg).await {
            Ok(Some(issues)) => {
                reg.issue_connectors.insert(IssueTracker::Jira, issues);
            }
            Ok(None) => {
                info!("jira: no credentials configured; skipping issue connector");
            }
            Err(e) => {
                warn!(error = %e, "jira: issue connector build failed; skipping");
            }
        }

        // Jira events.
        match crate::connectors::jira::events::try_build(resolver, cfg).await {
            Ok(Some(c)) => {
                reg.tracker_event_connectors.insert(IssueTracker::Jira, c);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "jira events: connector build failed; skipping");
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
            EventSourceRef::TrackerProject { tracker, .. } => self
                .tracker_event_connectors
                .get(tracker)
                .cloned()
                .or_else(|| match tracker {
                    IssueTracker::Github => self.events(Platform::Github),
                    IssueTracker::Gitlab => self.events(Platform::Gitlab),
                    IssueTracker::Linear | IssueTracker::Jira => None,
                }),
        }
    }

    /// Test/internal: register an `EventConnector` directly.
    /// Discovery wires the GitHub + GitLab impls from
    /// `connectors::github::events::build` /
    /// `connectors::gitlab::events::build` once those land.
    pub fn insert_event_connector(&mut self, p: Platform, c: Arc<dyn EventConnector>) {
        self.event_connectors.insert(p, c);
    }

    /// Test/internal: register a `RepoConnector` directly without going
    /// through `discover`. Used by tests that need a fake connector.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_repo_connector(&mut self, p: Platform, c: Arc<dyn RepoConnector>) {
        self.repo_connectors.insert(p, c);
    }

    /// Test/internal: register an `IssueConnector` directly without going
    /// through `discover`. Used by tests that need a fake connector.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_issue_connector(&mut self, t: IssueTracker, c: Arc<dyn IssueConnector>) {
        self.issue_connectors.insert(t, c);
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
    /// argument. Honors `[scm.default].platform` when it names a
    /// platform that actually has a registered connector; otherwise
    /// falls back to the registration-order preference (GitHub, then
    /// GitLab) — the same fallback used when no `[scm.default]` is
    /// configured at all. If `[scm.default].platform` names a platform
    /// with no live connector (e.g. GitLab configured but no GitLab
    /// credentials), that mismatch is logged at WARN — see
    /// ISSUES.md I-15 — so the fallback isn't indistinguishable from the
    /// "nothing configured" case in the logs.
    pub fn default_platform(&self) -> Option<Platform> {
        match resolve_configured_default(self.configured_default_platform, |p| {
            self.repo_connectors.contains_key(&p)
        }) {
            DefaultResolution::ConfiguredAndAvailable(p) => return Some(p),
            DefaultResolution::ConfiguredButUnavailable(p) => {
                warn!(
                    configured = %p,
                    "scm.default.platform is configured but has no registered connector; \
                     falling back to registration-order preference"
                );
            }
            DefaultResolution::Unset => {}
        }
        if self.repo_connectors.contains_key(&Platform::Github) {
            Some(Platform::Github)
        } else if self.repo_connectors.contains_key(&Platform::Gitlab) {
            Some(Platform::Gitlab)
        } else {
            None
        }
    }

    /// Return the default issue tracker for tools that omit the `tracker`
    /// argument. Honors `[issues.default].tracker` when it names a
    /// tracker that actually has a registered connector; otherwise falls
    /// back to the registration-order preference (GitHub, GitLab,
    /// Linear, Jira) — the same fallback used when no `[issues.default]`
    /// is configured at all. If `[issues.default].tracker` names a
    /// tracker with no live connector, that mismatch is logged at WARN —
    /// see ISSUES.md I-15.
    pub fn default_tracker(&self) -> Option<IssueTracker> {
        match resolve_configured_default(self.configured_default_tracker, |t| {
            self.issue_connectors.contains_key(&t)
        }) {
            DefaultResolution::ConfiguredAndAvailable(t) => return Some(t),
            DefaultResolution::ConfiguredButUnavailable(t) => {
                warn!(
                    configured = %t,
                    "issues.default.tracker is configured but has no registered connector; \
                     falling back to registration-order preference"
                );
            }
            DefaultResolution::Unset => {}
        }
        [
            IssueTracker::Github,
            IssueTracker::Gitlab,
            IssueTracker::Linear,
            IssueTracker::Jira,
        ]
        .into_iter()
        .find(|t| self.issue_connectors.contains_key(t))
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

#[cfg(test)]
mod tests {
    //! ISSUES.md I-15: `default_platform`/`default_tracker` used to
    //! hardcode a GitHub-then-GitLab preference and never read
    //! `[scm.default]`/`[issues.default]`. These tests build a Registry
    //! via the `empty()` + `insert_*_connector` test seam (no real
    //! credentials/network needed) and assert the config-first, then
    //! registration-order-fallback behavior at the `default_platform()`/
    //! `default_tracker()` consumer surface.
    use std::path::Path;

    use async_trait::async_trait;

    use super::*;
    use crate::error::ScmError;
    use crate::types::{
        Branch, Comment, CreateIssue, CreatePr, Diff, FileContent, Issue, IssueFilter, IssueRef,
        IssueState, Pr, PrFilter, PrRef, Repo, RepoRef,
    };

    struct FakeRepoConnector(Platform);

    #[async_trait]
    impl RepoConnector for FakeRepoConnector {
        fn platform(&self) -> Platform {
            self.0
        }
        async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
            unimplemented!()
        }
        async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> {
            unimplemented!()
        }
        async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
            unimplemented!()
        }
        async fn create_branch(
            &self,
            _r: &RepoRef,
            _name: &str,
            _from_sha: &str,
        ) -> Result<Branch, ScmError> {
            unimplemented!()
        }
        async fn read_file(
            &self,
            _r: &RepoRef,
            _path: &str,
            _ref_: Option<&str>,
        ) -> Result<FileContent, ScmError> {
            unimplemented!()
        }
        async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
            unimplemented!()
        }
        async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
            unimplemented!()
        }
        async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
            unimplemented!()
        }
        async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
            unimplemented!()
        }
        async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
            unimplemented!()
        }
        async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
            unimplemented!()
        }
    }

    struct FakeIssueConnector(IssueTracker);

    #[async_trait]
    impl IssueConnector for FakeIssueConnector {
        fn tracker(&self) -> IssueTracker {
            self.0
        }
        async fn list_issues(
            &self,
            _project: &str,
            _filter: IssueFilter,
        ) -> Result<Vec<Issue>, ScmError> {
            unimplemented!()
        }
        async fn get_issue(&self, _i: &IssueRef) -> Result<Issue, ScmError> {
            unimplemented!()
        }
        async fn comment_issue(&self, _i: &IssueRef, _body: &str) -> Result<Comment, ScmError> {
            unimplemented!()
        }
        async fn create_issue(
            &self,
            _project: &str,
            _opts: CreateIssue,
        ) -> Result<Issue, ScmError> {
            unimplemented!()
        }
        async fn update_issue_state(
            &self,
            _i: &IssueRef,
            _state: IssueState,
        ) -> Result<(), ScmError> {
            unimplemented!()
        }
    }

    #[test]
    fn resolve_configured_default_distinguishes_unset_available_and_unavailable() {
        // I-15 follow-up: `default_platform`/`default_tracker` warn when a
        // configured default names something with no live connector, so
        // that case must be distinguishable from "nothing configured" —
        // this is exactly what `resolve_configured_default` exists to
        // make unit-testable without a tracing-capture harness (the crate
        // has none). This test is the "warn path is taken" assertion:
        // `ConfiguredButUnavailable` is the variant `default_platform`/
        // `default_tracker` match on to emit `tracing::warn!`.
        assert_eq!(
            resolve_configured_default(None::<Platform>, |_| true),
            DefaultResolution::Unset
        );
        assert_eq!(
            resolve_configured_default(Some(Platform::Gitlab), |p| p == Platform::Gitlab),
            DefaultResolution::ConfiguredAndAvailable(Platform::Gitlab)
        );
        assert_eq!(
            resolve_configured_default(Some(Platform::Gitlab), |p| p == Platform::Github),
            DefaultResolution::ConfiguredButUnavailable(Platform::Gitlab)
        );
    }

    #[test]
    fn default_platform_prefers_the_configured_value() {
        // A GitLab-primary shop that sets [scm.default] platform = "gitlab"
        // must get GitLab back even though GitHub is also registered and
        // would otherwise win under the registration-order fallback.
        let mut reg = Registry::empty();
        reg.insert_repo_connector(
            Platform::Github,
            Arc::new(FakeRepoConnector(Platform::Github)),
        );
        reg.insert_repo_connector(
            Platform::Gitlab,
            Arc::new(FakeRepoConnector(Platform::Gitlab)),
        );
        reg.configured_default_platform = Some(Platform::Gitlab);

        assert_eq!(reg.default_platform(), Some(Platform::Gitlab));
    }

    #[test]
    fn default_platform_falls_back_when_config_is_unset() {
        // With no [scm.default], preserve today's registration-order
        // preference (GitHub before GitLab).
        let mut reg = Registry::empty();
        reg.insert_repo_connector(
            Platform::Github,
            Arc::new(FakeRepoConnector(Platform::Github)),
        );
        reg.insert_repo_connector(
            Platform::Gitlab,
            Arc::new(FakeRepoConnector(Platform::Gitlab)),
        );

        assert_eq!(reg.default_platform(), Some(Platform::Github));
    }

    #[test]
    fn default_platform_ignores_configured_value_with_no_matching_connector() {
        // A stale/misconfigured [scm.default] naming a platform that
        // has no registered connector must not black-hole the default;
        // fall back to whatever is actually registered.
        let mut reg = Registry::empty();
        reg.insert_repo_connector(
            Platform::Github,
            Arc::new(FakeRepoConnector(Platform::Github)),
        );
        reg.configured_default_platform = Some(Platform::Gitlab);

        assert_eq!(reg.default_platform(), Some(Platform::Github));
    }

    #[test]
    fn default_tracker_includes_linear() {
        // default_tracker() used to only consider Github/Gitlab, so a
        // Linear-only setup got None even with Linear wired.
        let mut reg = Registry::empty();
        reg.insert_issue_connector(
            IssueTracker::Linear,
            Arc::new(FakeIssueConnector(IssueTracker::Linear)),
        );

        assert_eq!(reg.default_tracker(), Some(IssueTracker::Linear));
    }

    #[test]
    fn default_tracker_prefers_the_configured_value() {
        let mut reg = Registry::empty();
        reg.insert_issue_connector(
            IssueTracker::Github,
            Arc::new(FakeIssueConnector(IssueTracker::Github)),
        );
        reg.insert_issue_connector(
            IssueTracker::Gitlab,
            Arc::new(FakeIssueConnector(IssueTracker::Gitlab)),
        );
        reg.configured_default_tracker = Some(IssueTracker::Gitlab);

        assert_eq!(reg.default_tracker(), Some(IssueTracker::Gitlab));
    }

    #[test]
    fn default_tracker_falls_back_when_config_is_unset() {
        let mut reg = Registry::empty();
        reg.insert_issue_connector(
            IssueTracker::Github,
            Arc::new(FakeIssueConnector(IssueTracker::Github)),
        );
        reg.insert_issue_connector(
            IssueTracker::Gitlab,
            Arc::new(FakeIssueConnector(IssueTracker::Gitlab)),
        );

        assert_eq!(reg.default_tracker(), Some(IssueTracker::Github));
    }
}
