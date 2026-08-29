//! Registry that discovers and manages connectors for configured accounts.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use tracing::{info, warn};

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::account::AccountId;
use crate::connectors::github::extras::GithubExtras;
use crate::connectors::gitlab::extras::GitlabExtras;
use crate::connectors::{IssueConnector, RepoConnector};
use crate::error::AccountError;
use crate::event_connector::EventConnector;
use crate::platform::{IssueTracker, Platform};
use crate::rules::{self, Resolution, Rule};
use crate::types::{EventSourceRef, RepoRef};

/// One configured SCM account: a name (`AccountId`) plus the connectors
/// built for it. `kind` is the vendor (`Platform`) this account talks
/// to — the account's *name* is its identity, `kind` selects the client
/// implementation, mirroring the model Arc 1 established for LLM
/// providers (see `crate::account`'s module doc).
struct ScmAccount {
    kind: Platform,
    repo: Option<Arc<dyn RepoConnector>>,
    issues: Option<Arc<dyn IssueConnector>>,
    events: Option<Arc<dyn EventConnector>>,
    github_extras: Option<Arc<GithubExtras>>,
    gitlab_extras: Option<Arc<GitlabExtras>>,
}

impl ScmAccount {
    fn empty(kind: Platform) -> Self {
        Self {
            kind,
            repo: None,
            issues: None,
            events: None,
            github_extras: None,
            gitlab_extras: None,
        }
    }
}

/// Registry that builds connectors from configured credentials.
/// Connectors are discovered once during [`discover`] and cached,
/// keyed by [`AccountId`] rather than by [`Platform`] — a user can hold
/// two GitHub accounts (work + personal), or github.com alongside a
/// GitHub Enterprise host. `Platform` stays the vendor; only the map
/// key changed. Which account serves a given request is decided by
/// [`repo_for`]/[`issues_for`], which run [`rules::resolve_account`].
///
/// [`discover`]: Self::discover
/// [`repo_for`]: Self::repo_for
/// [`issues_for`]: Self::issues_for
#[derive(Default)]
pub struct Registry {
    /// GitHub/GitLab accounts — the only kinds `Platform` currently has.
    accounts: BTreeMap<AccountId, ScmAccount>,
    /// Tracker-only accounts (Linear, Jira) have no `Platform`, so they
    /// cannot live in `accounts`. Not (yet) multi-account: exactly one
    /// entry each, under the bare tracker name (`"linear"` / `"jira"`),
    /// matching today's single-account behavior — see the module's
    /// `discover` doc.
    tracker_accounts: BTreeMap<AccountId, (IssueTracker, Arc<dyn IssueConnector>)>,
    tracker_event_connectors: BTreeMap<AccountId, Arc<dyn EventConnector>>,
    /// `[[scm.rules]]`, parsed once at `discover` time.
    rules: Vec<Rule>,
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

/// The `IssueTracker` variant that shares a `Platform` with `t`, or
/// `None` for the tracker-only vendors (Linear, Jira) that have none.
fn platform_for_tracker(t: IssueTracker) -> Option<Platform> {
    match t {
        IssueTracker::Github => Some(Platform::Github),
        IssueTracker::Gitlab => Some(Platform::Gitlab),
        IssueTracker::Linear | IssueTracker::Jira => None,
    }
}

impl Registry {
    /// Discover connectors from configured credentials. Every configured
    /// account (`[scm.<name>]`) is probed independently, plus the two
    /// built-in bare vendor names (`"github"`, `"gitlab"`) when not
    /// already declared — mirroring Arc 1's "built-in vendor names are
    /// always candidates" rule (spec §3.1, §7 acceptance table). Missing
    /// credentials are logged at INFO level and skipped. Errors during
    /// build are logged at WARN level and also skipped — the registry
    /// continues with whatever succeeded.
    ///
    /// Back-compat (load-bearing): with no `[scm.*]` config at all, this
    /// reduces to probing exactly `"github"` and `"gitlab"` as bare
    /// account names, which is byte-for-byte what today's registry
    /// probes — a user with only a `github/sso` credential and no
    /// `[scm.*]` config sees zero change.
    ///
    /// `sink` is stored on every connector this call builds and is reused
    /// for every future call through them — a `Registry` is long-lived
    /// relative to any one run (see the module doc / netflow-per-run plan).
    /// Callers backing a single in-flight run pass that run's
    /// `Arc<dyn FlowSink>`; run-less callers (bare CLI list commands, cron
    /// pollers) pass `Arc::new(rupu_netflow::NullSink)`.
    pub async fn discover(
        resolver: &dyn CredentialResolver,
        cfg: &Config,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Self {
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
            rules: cfg.scm.rules.iter().map(Rule::from_config).collect(),
            ..Self::default()
        };

        // Every declared `[scm.<name>]` table, plus the two built-in
        // bare vendor names when not already declared. `BTreeSet` keeps
        // build order (and therefore log order) deterministic.
        let mut account_names: std::collections::BTreeSet<String> =
            cfg.scm.platforms.keys().cloned().collect();
        account_names.insert("github".to_string());
        account_names.insert("gitlab".to_string());

        for name in account_names {
            let platform_cfg = cfg.scm.platforms.get(&name);
            // §3.1: `kind` defaults to the account name when that name
            // is itself a known vendor. An account whose kind resolves
            // to neither `github` nor `gitlab` (a Linear/Jira config
            // table, or a typo) simply isn't a repo account — skip it
            // here; Linear/Jira are handled by their own segment below.
            let kind = platform_cfg
                .and_then(|p| p.kind.as_deref())
                .and_then(|k| k.parse::<Platform>().ok())
                .or_else(|| name.parse::<Platform>().ok());
            let Some(kind) = kind else { continue };

            let mut acct = ScmAccount::empty(kind);
            match kind {
                Platform::Github => {
                    match crate::connectors::github::try_build(&name, resolver, cfg, sink.clone())
                        .await
                    {
                        Ok(Some((repo, issues, extras))) => {
                            acct.repo = Some(repo);
                            acct.issues = Some(issues);
                            acct.github_extras = Some(extras);
                        }
                        Ok(None) => {
                            info!(account = %name, "github: no credentials configured; skipping connector");
                        }
                        Err(e) => {
                            warn!(account = %name, error = %e, "github: connector build failed; skipping");
                        }
                    }
                    match crate::connectors::github::events::try_build(
                        &name,
                        resolver,
                        cfg,
                        sink.clone(),
                    )
                    .await
                    {
                        Ok(Some(c)) => acct.events = Some(c),
                        Ok(None) => {}
                        Err(e) => {
                            warn!(account = %name, error = %e, "github events: connector build failed; skipping");
                        }
                    }
                }
                Platform::Gitlab => {
                    match crate::connectors::gitlab::try_build(&name, resolver, cfg, sink.clone())
                        .await
                    {
                        Ok(Some((repo, issues, extras))) => {
                            acct.repo = Some(repo);
                            acct.issues = Some(issues);
                            acct.gitlab_extras = Some(extras);
                        }
                        Ok(None) => {
                            info!(account = %name, "gitlab: no credentials configured; skipping connector");
                        }
                        Err(e) => {
                            warn!(account = %name, error = %e, "gitlab: connector build failed; skipping");
                        }
                    }
                    match crate::connectors::gitlab::events::try_build(
                        &name,
                        resolver,
                        cfg,
                        sink.clone(),
                    )
                    .await
                    {
                        Ok(Some(c)) => acct.events = Some(c),
                        Ok(None) => {}
                        Err(e) => {
                            warn!(account = %name, error = %e, "gitlab events: connector build failed; skipping");
                        }
                    }
                }
            }

            // Only register the account if something actually built —
            // keeps `accounts_for`/fan-out honest (no phantom accounts
            // with every field `None`).
            if acct.repo.is_some() || acct.issues.is_some() || acct.events.is_some() {
                reg.accounts.insert(AccountId::new(name), acct);
            }
        }

        // Linear issues. Not (yet) multi-account — see the `tracker_accounts`
        // field doc — so this stays a single bare-name probe, unchanged
        // from before the reshape.
        match crate::connectors::linear::try_build(resolver, cfg, sink.clone()).await {
            Ok(Some(issues)) => {
                reg.tracker_accounts
                    .insert(AccountId::new("linear"), (IssueTracker::Linear, issues));
            }
            Ok(None) => {
                info!("linear: no credentials configured; skipping issue connector");
            }
            Err(e) => {
                warn!(error = %e, "linear: issue connector build failed; skipping");
            }
        }

        // Linear events.
        match crate::connectors::linear::events::try_build(resolver, cfg, sink.clone()).await {
            Ok(Some(c)) => {
                reg.tracker_event_connectors
                    .insert(AccountId::new("linear"), c);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "linear events: connector build failed; skipping");
            }
        }

        // Jira issues.
        match crate::connectors::jira::try_build(resolver, cfg, sink.clone()).await {
            Ok(Some(issues)) => {
                reg.tracker_accounts
                    .insert(AccountId::new("jira"), (IssueTracker::Jira, issues));
            }
            Ok(None) => {
                info!("jira: no credentials configured; skipping issue connector");
            }
            Err(e) => {
                warn!(error = %e, "jira: issue connector build failed; skipping");
            }
        }

        // Jira events.
        match crate::connectors::jira::events::try_build(resolver, cfg, sink.clone()).await {
            Ok(Some(c)) => {
                reg.tracker_event_connectors
                    .insert(AccountId::new("jira"), c);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "jira events: connector build failed; skipping");
            }
        }

        reg
    }

    /// Every account registered for `kind`, in deterministic (`AccountId`)
    /// order — fan-out output is user-visible (`rupu repos list`) and
    /// must not be hash-ordered.
    pub fn accounts_for(&self, kind: Platform) -> Vec<AccountId> {
        self.accounts
            .iter()
            .filter(|(_, a)| a.kind == kind && a.repo.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn accounts_for_tracker(&self, tracker: IssueTracker) -> Vec<AccountId> {
        match platform_for_tracker(tracker) {
            Some(kind) => self
                .accounts
                .iter()
                .filter(|(_, a)| a.kind == kind && a.issues.is_some())
                .map(|(id, _)| id.clone())
                .collect(),
            None => self
                .tracker_accounts
                .iter()
                .filter(|(_, (t, _))| *t == tracker)
                .map(|(id, _)| id.clone())
                .collect(),
        }
    }

    /// Resolve which account's `RepoConnector` serves `repo`, running the
    /// rule engine (spec §6.2/§6.3): explicit `--account` → owner rule →
    /// path rule → sole account of `repo.platform` → error. `cwd` powers
    /// path rules; pass `None` when the caller has no filesystem context
    /// (e.g. a webhook handler).
    pub fn repo_for(
        &self,
        repo: &RepoRef,
        cwd: Option<&Path>,
        explicit: Option<&AccountId>,
    ) -> Result<(AccountId, Arc<dyn RepoConnector>), AccountError> {
        let candidates = self.accounts_for(repo.platform);
        let home = dirs::home_dir();
        let resolution = rules::resolve_account(
            &self.rules,
            Some(repo),
            cwd,
            home.as_deref(),
            explicit,
            &candidates,
        );
        match resolution {
            Resolution::Explicit(id)
            | Resolution::Owner(id)
            | Resolution::Path(id)
            | Resolution::SoleAccount(id) => self.lookup_repo(id, repo.platform, &candidates),
            Resolution::NoMatch { candidates } => Err(AccountError::NoRuleMatched {
                repo: format!("{}/{}", repo.owner, repo.repo),
                platform: repo.platform.to_string(),
                candidates,
            }),
            Resolution::NoAccounts => Err(AccountError::NoAccounts {
                platform: repo.platform.to_string(),
            }),
        }
    }

    /// The existence check `resolve_account` deliberately doesn't do (it
    /// is a pure function; a `Resolution::Explicit(a)` names whatever the
    /// caller typed, unconditionally). This is that check, shared by
    /// every `Resolution` variant that names an account.
    fn lookup_repo(
        &self,
        id: AccountId,
        platform: Platform,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<dyn RepoConnector>), AccountError> {
        if let Some(acct) = self.accounts.get(&id) {
            if acct.kind == platform {
                if let Some(conn) = &acct.repo {
                    return Ok((id, conn.clone()));
                }
            }
        }
        Err(AccountError::UnknownAccount {
            requested: id,
            configured: candidates.to_vec(),
        })
    }

    /// Resolve which account's `IssueConnector` serves `tracker`. Unlike
    /// `repo_for`, there is no `RepoRef`/`cwd` here — `issues_for`'s
    /// callers key off a tracker + project string, not a repo — so only
    /// the explicit and sole-account tiers of the rule engine can ever
    /// fire; owner/path rules need a `RepoRef`/`cwd` this signature
    /// doesn't carry. `project` is not currently consumed by resolution;
    /// it is threaded through purely so ambiguity errors can name what
    /// was being looked up.
    pub fn issues_for(
        &self,
        tracker: IssueTracker,
        project: Option<&str>,
        explicit: Option<&AccountId>,
    ) -> Result<(AccountId, Arc<dyn IssueConnector>), AccountError> {
        let candidates = self.accounts_for_tracker(tracker);
        let resolution =
            rules::resolve_account(&self.rules, None, None, None, explicit, &candidates);
        match resolution {
            Resolution::Explicit(id)
            | Resolution::Owner(id)
            | Resolution::Path(id)
            | Resolution::SoleAccount(id) => self.lookup_issues(id, tracker, &candidates),
            Resolution::NoMatch { candidates } => Err(AccountError::NoRuleMatched {
                repo: project.unwrap_or("(no project given)").to_string(),
                platform: tracker.to_string(),
                candidates,
            }),
            Resolution::NoAccounts => Err(AccountError::NoAccounts {
                platform: tracker.to_string(),
            }),
        }
    }

    fn lookup_issues(
        &self,
        id: AccountId,
        tracker: IssueTracker,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<dyn IssueConnector>), AccountError> {
        match platform_for_tracker(tracker) {
            Some(kind) => {
                if let Some(acct) = self.accounts.get(&id) {
                    if acct.kind == kind {
                        if let Some(c) = &acct.issues {
                            return Ok((id, c.clone()));
                        }
                    }
                }
            }
            None => {
                if let Some((t, c)) = self.tracker_accounts.get(&id) {
                    if *t == tracker {
                        return Ok((id, c.clone()));
                    }
                }
            }
        }
        Err(AccountError::UnknownAccount {
            requested: id,
            configured: candidates.to_vec(),
        })
    }

    /// Direct lookup by account name, with no rule engine involved. Used
    /// by account-scoped operations that already know exactly which
    /// account they want (e.g. resolving `--account` for a non-repo
    /// operation, or `rupu scm bind`'s validation).
    pub fn repo_by_account(&self, id: &AccountId) -> Option<Arc<dyn RepoConnector>> {
        self.accounts.get(id).and_then(|a| a.repo.clone())
    }

    /// Every registered account's `RepoConnector` for `kind`, in
    /// deterministic order — the fan-out accessor for account-scoped
    /// operations that have no repo to key on (`rupu repos list`, spec
    /// §6.2's "Account-scoped, no repo" row).
    pub fn all_repo_connectors(&self, kind: Platform) -> Vec<(AccountId, Arc<dyn RepoConnector>)> {
        self.accounts
            .iter()
            .filter(|(_, a)| a.kind == kind)
            .filter_map(|(id, a)| a.repo.clone().map(|c| (id.clone(), c)))
            .collect()
    }

    /// Retrieve the RepoConnector for a given platform. Back-compat shim
    /// over the account-keyed map: prefers the bare-vendor-named account
    /// (`"github"`/`"gitlab"`) — the account a pre-Arc-2 config always
    /// produces — falling back to the first (lowest `AccountId`)
    /// registered account of that kind so a multi-account setup with no
    /// bare-named account doesn't silently look empty to old call sites.
    /// Clones the Arc so the caller owns a reference.
    pub fn repo(&self, p: Platform) -> Option<Arc<dyn RepoConnector>> {
        self.accounts
            .get(&AccountId::new(p.as_str()))
            .and_then(|a| a.repo.clone())
            .or_else(|| {
                self.accounts
                    .values()
                    .find(|a| a.kind == p)
                    .and_then(|a| a.repo.clone())
            })
    }

    /// Retrieve the IssueConnector for a given tracker. Back-compat shim;
    /// see [`repo`](Self::repo)'s doc for the bare-name-first preference.
    pub fn issues(&self, t: IssueTracker) -> Option<Arc<dyn IssueConnector>> {
        match platform_for_tracker(t) {
            Some(kind) => self
                .accounts
                .get(&AccountId::new(t.as_str()))
                .and_then(|a| a.issues.clone())
                .or_else(|| {
                    self.accounts
                        .values()
                        .find(|a| a.kind == kind)
                        .and_then(|a| a.issues.clone())
                }),
            None => self
                .tracker_accounts
                .get(&AccountId::new(t.as_str()))
                .map(|(_, c)| c.clone()),
        }
    }

    /// Retrieve the EventConnector for a given platform, if one is
    /// registered. Used by `rupu cron tick`'s polled-events tier.
    /// Back-compat shim; see [`repo`](Self::repo)'s doc for the
    /// bare-name-first preference.
    pub fn events(&self, p: Platform) -> Option<Arc<dyn EventConnector>> {
        self.accounts
            .get(&AccountId::new(p.as_str()))
            .and_then(|a| a.events.clone())
            .or_else(|| {
                self.accounts
                    .values()
                    .find(|a| a.kind == p)
                    .and_then(|a| a.events.clone())
            })
    }

    /// Retrieve the EventConnector suitable for a trigger source.
    pub fn events_for_source(&self, source: &EventSourceRef) -> Option<Arc<dyn EventConnector>> {
        match source {
            EventSourceRef::Repo { repo } => self.events(repo.platform),
            EventSourceRef::TrackerProject { tracker, .. } => self
                .tracker_event_connectors
                .get(&AccountId::new(tracker.as_str()))
                .cloned()
                .or_else(|| match tracker {
                    IssueTracker::Github => self.events(Platform::Github),
                    IssueTracker::Gitlab => self.events(Platform::Gitlab),
                    IssueTracker::Linear | IssueTracker::Jira => None,
                }),
        }
    }

    /// Test/internal: register an `EventConnector` directly under the
    /// bare vendor-named account. Discovery wires the GitHub + GitLab
    /// impls from `connectors::github::events::build` /
    /// `connectors::gitlab::events::build`.
    pub fn insert_event_connector(&mut self, p: Platform, c: Arc<dyn EventConnector>) {
        self.accounts
            .entry(AccountId::new(p.as_str()))
            .or_insert_with(|| ScmAccount::empty(p))
            .events = Some(c);
    }

    /// Test/internal: register a `RepoConnector` directly under the bare
    /// vendor-named account, without going through `discover`. Used by
    /// tests that need a fake connector but don't care about multiple
    /// accounts of the same platform — see [`insert_repo_account`] for
    /// the account-aware seam.
    ///
    /// [`insert_repo_account`]: Self::insert_repo_account
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_repo_connector(&mut self, p: Platform, c: Arc<dyn RepoConnector>) {
        self.accounts
            .entry(AccountId::new(p.as_str()))
            .or_insert_with(|| ScmAccount::empty(p))
            .repo = Some(c);
    }

    /// Test/internal: register a `RepoConnector` under an explicit
    /// account name and kind — the seam multi-account tests need.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_repo_account(
        &mut self,
        id: AccountId,
        kind: Platform,
        c: Arc<dyn RepoConnector>,
    ) {
        self.accounts
            .entry(id)
            .or_insert_with(|| ScmAccount::empty(kind))
            .repo = Some(c);
    }

    /// Test/internal: register an `IssueConnector` directly without going
    /// through `discover`. Used by tests that need a fake connector.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_issue_connector(&mut self, t: IssueTracker, c: Arc<dyn IssueConnector>) {
        match platform_for_tracker(t) {
            Some(kind) => {
                self.accounts
                    .entry(AccountId::new(t.as_str()))
                    .or_insert_with(|| ScmAccount::empty(kind))
                    .issues = Some(c);
            }
            None => {
                self.tracker_accounts
                    .insert(AccountId::new(t.as_str()), (t, c));
            }
        }
    }

    /// Test/internal: replace the account-selection rule set directly,
    /// without a `Config` round-trip.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn set_rules(&mut self, rules: Vec<Rule>) {
        self.rules = rules;
    }

    /// Returns the per-platform extras handle for GitHub actions, if
    /// GitHub credentials were present during discovery. Prefers the
    /// bare-vendor-named account, then the first (lowest `AccountId`)
    /// GitHub-kind account with extras — see [`repo`](Self::repo)'s doc.
    pub fn github_extras(&self) -> Option<Arc<GithubExtras>> {
        self.accounts
            .get(&AccountId::new("github"))
            .and_then(|a| a.github_extras.clone())
            .or_else(|| self.accounts.values().find_map(|a| a.github_extras.clone()))
    }

    /// Returns the per-platform extras handle for GitLab pipeline
    /// triggers, if GitLab credentials were present during discovery.
    /// See [`github_extras`](Self::github_extras)'s doc for the
    /// preference order.
    pub fn gitlab_extras(&self) -> Option<Arc<GitlabExtras>> {
        self.accounts
            .get(&AccountId::new("gitlab"))
            .and_then(|a| a.gitlab_extras.clone())
            .or_else(|| self.accounts.values().find_map(|a| a.gitlab_extras.clone()))
    }

    /// Return the default platform for tools that omit the `platform`
    /// argument. Honors `[scm.default].platform` when it names a
    /// platform that has at least one registered account; otherwise
    /// falls back to the registration-order preference (GitHub, then
    /// GitLab) — the same fallback used when no `[scm.default]` is
    /// configured at all. If `[scm.default].platform` names a platform
    /// with no live account (e.g. GitLab configured but no GitLab
    /// credentials), that mismatch is logged at WARN — see
    /// ISSUES.md I-15 — so the fallback isn't indistinguishable from the
    /// "nothing configured" case in the logs. Answers "is any account of
    /// this platform registered?", not "is this platform registered".
    pub fn default_platform(&self) -> Option<Platform> {
        let available = |p: Platform| {
            self.accounts
                .values()
                .any(|a| a.kind == p && a.repo.is_some())
        };
        match resolve_configured_default(self.configured_default_platform, available) {
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
        if available(Platform::Github) {
            Some(Platform::Github)
        } else if available(Platform::Gitlab) {
            Some(Platform::Gitlab)
        } else {
            None
        }
    }

    /// Return the default issue tracker for tools that omit the `tracker`
    /// argument. Honors `[issues.default].tracker` when it names a
    /// tracker that has at least one registered account; otherwise falls
    /// back to the registration-order preference (GitHub, GitLab,
    /// Linear, Jira) — the same fallback used when no `[issues.default]`
    /// is configured at all. If `[issues.default].tracker` names a
    /// tracker with no live account, that mismatch is logged at WARN —
    /// see ISSUES.md I-15. Answers "is any account of this tracker
    /// registered?", not "is this tracker registered".
    pub fn default_tracker(&self) -> Option<IssueTracker> {
        let available = |t: IssueTracker| match platform_for_tracker(t) {
            Some(kind) => self
                .accounts
                .values()
                .any(|a| a.kind == kind && a.issues.is_some()),
            None => self
                .tracker_accounts
                .values()
                .any(|(tracker, _)| *tracker == t),
        };
        match resolve_configured_default(self.configured_default_tracker, available) {
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
        .find(|t| available(*t))
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

    /// Shared fake connector for the account-keying tests below — reuses
    /// the `FakeRepoConnector` already defined in this module rather than
    /// inventing a second fake type. The `Platform` baked into the fake
    /// is irrelevant to resolution: `repo_for`/`accounts_for` key off
    /// `ScmAccount::kind` (set explicitly by `insert_repo_account`), never
    /// off `RepoConnector::platform()`.
    fn fake_repo_connector() -> Arc<dyn RepoConnector> {
        Arc::new(FakeRepoConnector(Platform::Github))
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

    // ── Account-keyed resolution (Arc 2 / Task 3) ──────────────────────

    #[tokio::test]
    async fn single_account_resolves_without_any_rules() {
        let mut reg = Registry::default();
        reg.insert_repo_account(
            AccountId::new("github"),
            Platform::Github,
            fake_repo_connector(),
        );
        let r = RepoRef {
            platform: Platform::Github,
            owner: "anyone".into(),
            repo: "thing".into(),
        };
        let (id, _conn) = reg
            .repo_for(&r, None, None)
            .expect("sole account must resolve");
        assert_eq!(id, AccountId::new("github"));
    }

    #[tokio::test]
    async fn two_accounts_without_a_matching_rule_error_with_candidates() {
        let mut reg = Registry::default();
        reg.insert_repo_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.insert_repo_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_repo_connector(),
        );
        let r = RepoRef {
            platform: Platform::Github,
            owner: "other".into(),
            repo: "thing".into(),
        };
        let err = reg.repo_for(&r, None, None).err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("other/thing"),
            "error must name the repo: {msg}"
        );
        assert!(
            msg.contains("gh-work") && msg.contains("gh-personal"),
            "must list candidates: {msg}"
        );
    }

    #[tokio::test]
    async fn an_owner_rule_selects_between_two_accounts() {
        let mut reg = Registry::default();
        reg.insert_repo_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.insert_repo_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-work"),
        }]);
        let r = RepoRef {
            platform: Platform::Github,
            owner: "acme".into(),
            repo: "api".into(),
        };
        let (id, _) = reg.repo_for(&r, None, None).unwrap();
        assert_eq!(id, AccountId::new("gh-work"));
    }

    #[tokio::test]
    async fn explicit_account_overrides_rules() {
        let mut reg = Registry::default();
        reg.insert_repo_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.insert_repo_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-work"),
        }]);
        let r = RepoRef {
            platform: Platform::Github,
            owner: "acme".into(),
            repo: "api".into(),
        };
        let (id, _) = reg
            .repo_for(&r, None, Some(&AccountId::new("gh-personal")))
            .unwrap();
        assert_eq!(id, AccountId::new("gh-personal"));
    }

    #[tokio::test]
    async fn fan_out_returns_every_account_of_a_platform_sorted() {
        let mut reg = Registry::default();
        reg.insert_repo_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.insert_repo_account(
            AccountId::new("acme-ghe"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.insert_repo_account(
            AccountId::new("gl-work"),
            Platform::Gitlab,
            fake_repo_connector(),
        );
        let ids: Vec<String> = reg
            .all_repo_connectors(Platform::Github)
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(ids, ["acme-ghe", "gh-work"], "github only, sorted");
    }

    #[tokio::test]
    async fn no_accounts_is_a_distinct_error_from_ambiguity() {
        let reg = Registry::default();
        let r = RepoRef {
            platform: Platform::Github,
            owner: "a".into(),
            repo: "b".into(),
        };
        let msg = reg.repo_for(&r, None, None).err().unwrap().to_string();
        assert!(
            msg.contains("auth login"),
            "no-accounts error should point at login: {msg}"
        );
    }

    /// Item 2 from the task brief: `resolve_account` returns
    /// `Resolution::Explicit(a)` unconditionally — it has no way to know
    /// whether `a` is a real account. A typo'd `--account` must not sail
    /// through to an obscure downstream failure; `repo_for`'s lookup must
    /// catch it here with a clear error.
    #[tokio::test]
    async fn explicit_account_typo_is_a_clear_error_not_a_silent_pass_through() {
        let mut reg = Registry::default();
        reg.insert_repo_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_repo_connector(),
        );
        reg.insert_repo_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_repo_connector(),
        );
        let r = RepoRef {
            platform: Platform::Github,
            owner: "acme".into(),
            repo: "api".into(),
        };
        let err = reg
            .repo_for(&r, None, Some(&AccountId::new("gh-wrok")))
            .err()
            .unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("no such account") && msg.contains("gh-wrok"),
            "must name the typo'd account: {msg}"
        );
        assert!(
            msg.contains("gh-work") && msg.contains("gh-personal"),
            "must list the configured accounts: {msg}"
        );
    }

    #[test]
    fn accounts_for_is_empty_and_sorted_with_nothing_registered() {
        let reg = Registry::default();
        assert!(reg.accounts_for(Platform::Github).is_empty());
    }
}
