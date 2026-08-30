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

/// Whether `discover` should warn about a declared `[scm.<name>]` table
/// whose `kind` never resolved to a `Platform` (see the call site in
/// `discover`'s account loop).
///
/// Pure and unit-tested directly — the crate has no tracing-capture
/// harness, so this is the coverage seam for the decision instead of the
/// `warn!` call itself.
///
/// `declared` is `platform_cfg.is_some()`: only a real `[scm.<name>]`
/// table can be a mistake worth flagging. The two implicit bare-name
/// probes (`"github"`, `"gitlab"` with no config table) are the normal
/// "no config for this built-in vendor" case, not a mistake — never warn
/// for those.
///
/// Among declared tables, `"linear"`/`"jira"` are excluded on purpose,
/// not by omission: they are legitimate declared `[scm.<name>]` tables
/// (carrying e.g. a self-hosted `base_url` — see `docs/using-rupu.md`'s
/// `[scm.jira]` example and `registry_discover.rs`'s
/// `jira_event_connector_built_when_credential_present`), and for them
/// `kind` can *never* resolve to a `Platform` — `Platform` only has
/// `Github`/`Gitlab`, so neither a declared `kind` nor the name-is-the-
/// vendor fallback can ever succeed. That is correct, not a config
/// mistake: Linear/Jira are handled by `discover`'s separate tracker
/// segment. Warning here on every `discover()` call for a documented,
/// working Jira/Linear config would be worse than the silent drop this
/// warn exists to replace — noise on a correct setup erodes trust in the
/// warning faster than it helps anyone. The principled test is "does
/// this name parse as an `IssueTracker` with no matching `Platform`" —
/// i.e. exactly Linear/Jira — rather than hardcoding the two strings, so
/// a future tracker-only vendor added to `IssueTracker` is excluded for
/// free instead of silently starting to warn.
fn should_warn_unresolvable_kind(name: &str, declared: bool) -> bool {
    if !declared {
        return false;
    }
    !matches!(name.parse::<IssueTracker>(), Ok(t) if platform_for_tracker(t).is_none())
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
            let Some(kind) = kind else {
                // A declared `[scm.<name>]` table that never resolves to
                // a repo `kind` is almost always a mistake — a missing
                // or misspelled `kind` (spec §3.1 requires it for any
                // non-vendor account name), since `Config::validate`
                // never inspects `cfg.scm.platforms`. Warn so the account
                // doesn't just vanish with no trace. But see
                // `should_warn_unresolvable_kind`'s doc: stay silent for
                // the implicit bare-name probes AND for legitimate
                // declared Linear/Jira tables, neither of which is a
                // mistake.
                if should_warn_unresolvable_kind(&name, platform_cfg.is_some()) {
                    warn!(
                        account = %name,
                        "scm: account declares no resolvable kind; skipping"
                    );
                }
                continue;
            };

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
            Resolution::RuleTargetUnavailable { account, pattern } => {
                Err(AccountError::RuleTargetUnavailable {
                    account,
                    pattern,
                    platform: repo.platform.to_string(),
                })
            }
            Resolution::NoMatch { candidates } => Err(AccountError::NoRuleMatched {
                repo: format!("{}/{}", repo.owner, repo.repo),
                owner: repo.owner.clone(),
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
    ///
    /// `resolve_account` returns `Explicit` before it ever looks at
    /// `candidates.is_empty()` — a pure function has no "log in" concept
    /// to distinguish from ambiguity. So `--account gh-work` with zero
    /// accounts configured at all must not fall through to
    /// `UnknownAccount` with a dangling empty candidate list (which reads
    /// as a typo); it is the same "log in" failure as the no-explicit
    /// case, and gets the same `NoAccounts` error.
    fn lookup_repo(
        &self,
        id: AccountId,
        platform: Platform,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<dyn RepoConnector>), AccountError> {
        if candidates.is_empty() {
            return Err(AccountError::NoAccounts {
                platform: platform.to_string(),
            });
        }
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

    /// Resolve which account's `IssueConnector` serves `tracker`, running
    /// the same rule engine `repo_for` runs whenever a `RepoRef` is known
    /// — owner and path rules need the caller's `RepoRef`/`cwd` exactly
    /// the way `repo_for` does, and dropping them here would reproduce
    /// the very defect this arc exists to fix (spec §1: "the owner is
    /// already in scope at these call sites and is currently
    /// discarded"). `rupu issues list --repo acme/api` builds a full
    /// `RepoRef` before calling in — passing it through is what lets an
    /// `acme/* -> gh-work` rule actually select an account for it. `home`
    /// is not a parameter, matching `repo_for`: it resolves the same way
    /// `repo_for` does (`dirs::home_dir()`), internally — unlike `cwd`,
    /// which genuinely differs per caller (an agent's working directory
    /// is not the process's cwd), the user's home directory is one
    /// process-global value with nothing for a caller to meaningfully
    /// supply, so making every future call site resolve and thread it
    /// through would be boilerplate with no corresponding benefit.
    ///
    /// **Linear and Jira genuinely have no `RepoRef`** — a tracker
    /// project string like `"ENG"` is not an owner/repo pair. Their
    /// callers pass `repo: None`, and only the explicit and sole-account
    /// tiers can ever resolve for them. That is correct, not a gap to
    /// close: it's the same reason `EventSourceRef::TrackerProject`
    /// needs an explicit `account` field on the trigger config (Task 6) —
    /// there is no owner/path to infer one from.
    pub fn issues_for(
        &self,
        tracker: IssueTracker,
        repo: Option<&RepoRef>,
        cwd: Option<&Path>,
        explicit: Option<&AccountId>,
    ) -> Result<(AccountId, Arc<dyn IssueConnector>), AccountError> {
        let candidates = self.accounts_for_tracker(tracker);
        let home = dirs::home_dir();
        let resolution = rules::resolve_account(
            &self.rules,
            repo,
            cwd,
            home.as_deref(),
            explicit,
            &candidates,
        );
        match resolution {
            Resolution::Explicit(id)
            | Resolution::Owner(id)
            | Resolution::Path(id)
            | Resolution::SoleAccount(id) => self.lookup_issues(id, tracker, &candidates),
            Resolution::RuleTargetUnavailable { account, pattern } => {
                Err(AccountError::RuleTargetUnavailable {
                    account,
                    pattern,
                    platform: tracker.to_string(),
                })
            }
            Resolution::NoMatch { candidates } => Err(AccountError::NoRuleMatched {
                repo: repo
                    .map(|r| format!("{}/{}", r.owner, r.repo))
                    .unwrap_or_else(|| format!("({tracker} lookup)")),
                owner: repo.map(|r| r.owner.clone()).unwrap_or_default(),
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
        if candidates.is_empty() {
            return Err(AccountError::NoAccounts {
                platform: tracker.to_string(),
            });
        }
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
    /// operation).
    ///
    /// NOT what powers `rupu scm bind`'s validation (Arc 2 final review
    /// item 4) — that check is a lighter-weight, synchronous read of
    /// layered `[scm.*]` config (`cmd/scm.rs`'s `warn_if_account_unknown`,
    /// the same shape `scm accounts` already reads config with),
    /// deliberately not a full `Registry::discover` + `repo_by_account`
    /// round trip: `bind` is a fast local-only "did I type this right"
    /// check, and building a live registry would mean network calls and
    /// credential resolution just to validate a string.
    pub fn repo_by_account(&self, id: &AccountId) -> Option<Arc<dyn RepoConnector>> {
        self.accounts.get(id).and_then(|a| a.repo.clone())
    }

    /// `github_extras_by_account`'s counterpart for when there is
    /// nothing to run the rule engine against at all — no `RepoRef`,
    /// not even an owner (Arc 2 Task 6 review item 1: a `projects_v2_item`
    /// webhook payload with no top-level `organization` object). The
    /// caller is expected to have already narrowed to a single candidate
    /// via `accounts_for(Platform::Github)` (sole-account tier) before
    /// calling this — this method itself does no disambiguation, same
    /// as `repo_by_account`.
    pub fn github_extras_by_account(&self, id: &AccountId) -> Option<Arc<GithubExtras>> {
        self.accounts.get(id).and_then(|a| a.github_extras.clone())
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

    /// Resolve which account's `EventConnector` serves a polled trigger
    /// source, running the same rule engine `repo_for`/`issues_for` run
    /// (spec §6.2/§6.3, Task 6): explicit → owner rule → path rule →
    /// sole account → error. Same three-argument shape as
    /// `repo_for`/`issues_for`/`github_extras_for` (Arc 2 Task 6 review
    /// item 2) — `explicit` is a real caller-supplied parameter, not
    /// something only `EventSourceRef::TrackerProject`'s embedded
    /// `account` field can carry. Before this fix, `Repo` sources always
    /// passed `None` as their explicit tier: an `account = "..."` on a
    /// repo-backed `poll_sources` entry (`github:owner/repo`,
    /// `gitlab:group/project`) parsed cleanly (a known field, so
    /// `deny_unknown_fields` never caught it) and then did nothing —
    /// exactly the silent-config-no-op this project rejects everywhere
    /// else. `cwd` powers path rules exactly like `repo_for`; `rupu cron
    /// tick` has no meaningful cwd of its own — it polls a remote
    /// source on a schedule, not from inside a checkout — so its caller
    /// passes `None`, same as the webhook receiver (spec §6.5's daemon
    /// note: "a webhook payload or cron poll knows the owner but has no
    /// cwd"). Only the explicit/owner/sole-account tiers can ever fire
    /// for a daemon caller as a result — that is by design, not a gap.
    ///
    /// `EventSourceRef::Repo` has an owner to key on and resolves
    /// exactly like `repo_for`, with `explicit` as its explicit tier.
    /// `EventSourceRef::TrackerProject` splits its `project` string into
    /// a `RepoRef` via
    /// [`tracker_project_repo`](crate::types::tracker_project_repo) for
    /// the repo-backed trackers (`github`/`gitlab` issues, where
    /// `project` IS `owner/repo`) so the same owner-rule tier applies
    /// there too. Its explicit tier is `explicit.or(account)` — the
    /// caller-supplied parameter wins when given (so a caller that
    /// threads a trigger-config `account` through the parameter, same
    /// as it would for a `Repo` source, behaves identically for either
    /// variant); the variant's own embedded `account` field is the
    /// fallback, so a value constructed directly with that field set
    /// (no separate parameter available at the call site) still
    /// resolves — this is the field's real purpose for Linear/Jira,
    /// whose `project` has no owner/path to infer an account from at
    /// all (spec §6.5: "the one place multi-account cannot be
    /// inferred").
    ///
    /// Was `events_for_source`'s pre-Task-6 shape: `Option<Arc<dyn
    /// EventConnector>>`, `Repo` routed through the account-arbitrary
    /// `events(Platform)` shim, and `TrackerProject` had no lever at
    /// all besides that same shim. The `Result<(AccountId, ...)>`
    /// return shape matches `repo_for`/`issues_for` so callers can
    /// distinguish "no account configured for this vendor" (skip,
    /// unremarkable) from "several accounts and nothing selected one"
    /// (a real misconfiguration worth a warning).
    pub fn events_for_source(
        &self,
        source: &EventSourceRef,
        cwd: Option<&Path>,
        explicit: Option<&AccountId>,
    ) -> Result<(AccountId, Arc<dyn EventConnector>), AccountError> {
        let home = dirs::home_dir();
        match source {
            EventSourceRef::Repo { repo } => {
                let candidates = self.accounts_for_events(repo.platform);
                let resolution = rules::resolve_account(
                    &self.rules,
                    Some(repo),
                    cwd,
                    home.as_deref(),
                    explicit,
                    &candidates,
                );
                let id = Self::resolution_to_id(resolution, || {
                    (
                        format!("{}/{}", repo.owner, repo.repo),
                        repo.owner.clone(),
                        repo.platform.to_string(),
                    )
                })?;
                self.lookup_events_platform(id, repo.platform, &candidates)
            }
            EventSourceRef::TrackerProject {
                tracker,
                project,
                account,
            } => {
                let explicit = explicit.or(account.as_ref());
                if let Some(repo) = crate::types::tracker_project_repo(*tracker, project) {
                    let candidates = self.accounts_for_events(repo.platform);
                    let resolution = rules::resolve_account(
                        &self.rules,
                        Some(&repo),
                        cwd,
                        home.as_deref(),
                        explicit,
                        &candidates,
                    );
                    let id = Self::resolution_to_id(resolution, || {
                        (project.clone(), repo.owner.clone(), tracker.to_string())
                    })?;
                    self.lookup_events_platform(id, repo.platform, &candidates)
                } else {
                    let candidates = self.accounts_for_tracker_events(*tracker);
                    let resolution = rules::resolve_account(
                        &self.rules,
                        None,
                        cwd,
                        home.as_deref(),
                        explicit,
                        &candidates,
                    );
                    let id = Self::resolution_to_id(resolution, || {
                        (project.clone(), String::new(), tracker.to_string())
                    })?;
                    self.lookup_events_tracker(id, *tracker, &candidates)
                }
            }
        }
    }

    /// Shared `Resolution` → `Result<AccountId, AccountError>` step for
    /// `events_for_source`'s three branches — same mapping `repo_for`
    /// inlines once, factored out here because `events_for_source` has
    /// three call sites for it. `describe` is called only on the three
    /// error paths (`RuleTargetUnavailable`/`NoMatch`/`NoAccounts`), so
    /// it's a closure rather than three eagerly-computed strings.
    fn resolution_to_id(
        resolution: Resolution,
        describe: impl FnOnce() -> (String, String, String),
    ) -> Result<AccountId, AccountError> {
        match resolution {
            Resolution::Explicit(id)
            | Resolution::Owner(id)
            | Resolution::Path(id)
            | Resolution::SoleAccount(id) => Ok(id),
            Resolution::RuleTargetUnavailable { account, pattern } => {
                let (_, _, platform) = describe();
                Err(AccountError::RuleTargetUnavailable {
                    account,
                    pattern,
                    platform,
                })
            }
            Resolution::NoMatch { candidates } => {
                let (repo, owner, platform) = describe();
                Err(AccountError::NoRuleMatched {
                    repo,
                    owner,
                    platform,
                    candidates,
                })
            }
            Resolution::NoAccounts => {
                let (_, _, platform) = describe();
                Err(AccountError::NoAccounts { platform })
            }
        }
    }

    /// Candidate accounts for `events_for_source`'s `Repo` tier and its
    /// repo-backed-tracker branch: every account of `kind` that actually
    /// has an `EventConnector` built (mirrors `accounts_for`'s
    /// `a.repo.is_some()` filter, but for `a.events`).
    fn accounts_for_events(&self, kind: Platform) -> Vec<AccountId> {
        self.accounts
            .iter()
            .filter(|(_, a)| a.kind == kind && a.events.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Candidate accounts for `events_for_source`'s tracker-native
    /// branch (Linear/Jira, or a repo-backed tracker whose `project`
    /// didn't split). Repo-backed trackers still live in `self.accounts`
    /// (there is no separate "github issues" account map), so this
    /// delegates to `accounts_for_events`; Linear/Jira live in
    /// `tracker_event_connectors`, which — per that field's doc — holds
    /// at most one entry per tracker today (not yet multi-account), so
    /// this returns a 0- or 1-element list.
    fn accounts_for_tracker_events(&self, tracker: IssueTracker) -> Vec<AccountId> {
        match platform_for_tracker(tracker) {
            Some(kind) => self.accounts_for_events(kind),
            None => {
                let id = AccountId::new(tracker.as_str());
                if self.tracker_event_connectors.contains_key(&id) {
                    vec![id]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn lookup_events_platform(
        &self,
        id: AccountId,
        platform: Platform,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<dyn EventConnector>), AccountError> {
        if candidates.is_empty() {
            return Err(AccountError::NoAccounts {
                platform: platform.to_string(),
            });
        }
        if let Some(acct) = self.accounts.get(&id) {
            if acct.kind == platform {
                if let Some(c) = &acct.events {
                    return Ok((id, c.clone()));
                }
            }
        }
        Err(AccountError::UnknownAccount {
            requested: id,
            configured: candidates.to_vec(),
        })
    }

    /// Mirrors `lookup_issues`'s dual-branch dispatch: a repo-backed
    /// tracker's accounts live in `self.accounts` (keyed by `Platform`,
    /// same map `lookup_events_platform` reads), Linear/Jira's live in
    /// `tracker_event_connectors`. Needed because
    /// `accounts_for_tracker_events` — this function's candidate-list
    /// counterpart — returns candidates from `self.accounts` for a
    /// repo-backed tracker (`events_for_source`'s call site reaches
    /// this branch when a `TrackerProject { tracker: Github/Gitlab,
    /// .. }`'s `project` doesn't split into `owner/repo`, e.g. no `/`)
    /// — looking only in `tracker_event_connectors` for those ids would
    /// always report `UnknownAccount`, even for an account that is
    /// genuinely configured and has an events connector.
    fn lookup_events_tracker(
        &self,
        id: AccountId,
        tracker: IssueTracker,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<dyn EventConnector>), AccountError> {
        if candidates.is_empty() {
            return Err(AccountError::NoAccounts {
                platform: tracker.to_string(),
            });
        }
        match platform_for_tracker(tracker) {
            Some(kind) => {
                if let Some(acct) = self.accounts.get(&id) {
                    if acct.kind == kind {
                        if let Some(c) = &acct.events {
                            return Ok((id, c.clone()));
                        }
                    }
                }
            }
            None => {
                if let Some(c) = self.tracker_event_connectors.get(&id) {
                    return Ok((id, c.clone()));
                }
            }
        }
        Err(AccountError::UnknownAccount {
            requested: id,
            configured: candidates.to_vec(),
        })
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

    /// Test/internal: register an `IssueConnector` under an explicit
    /// account name and kind — the `issues_for` counterpart to
    /// `insert_repo_account`.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_issue_account(
        &mut self,
        id: AccountId,
        kind: Platform,
        c: Arc<dyn IssueConnector>,
    ) {
        self.accounts
            .entry(id)
            .or_insert_with(|| ScmAccount::empty(kind))
            .issues = Some(c);
    }

    /// Test/internal: register an `EventConnector` under an explicit
    /// account name and kind — the `events()` counterpart to
    /// `insert_repo_account`/`insert_issue_account`.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_event_account(
        &mut self,
        id: AccountId,
        kind: Platform,
        c: Arc<dyn EventConnector>,
    ) {
        self.accounts
            .entry(id)
            .or_insert_with(|| ScmAccount::empty(kind))
            .events = Some(c);
    }

    /// Test/internal: register a `GithubExtras` handle under an explicit
    /// account name — the `github_extras_for()` counterpart to
    /// `insert_repo_account`. Lets a test prove account routing for
    /// `github.workflows_dispatch` without a real octocrab HTTP round
    /// trip: `github_extras_for` returning the same `Arc` this test
    /// inserted is a strong enough signal (Arc identity), since
    /// `lookup_github_extras`'s own logic is otherwise a straight
    /// `Option` read with no further branching to miss.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_github_extras_account(&mut self, id: AccountId, extras: Arc<GithubExtras>) {
        self.accounts
            .entry(id)
            .or_insert_with(|| ScmAccount::empty(Platform::Github))
            .github_extras = Some(extras);
    }

    /// Test/internal: register a `GitlabExtras` handle under an explicit
    /// account name — see
    /// [`insert_github_extras_account`](Self::insert_github_extras_account)'s
    /// doc for why this is enough to exercise `gitlab_extras_for`'s
    /// routing without a real HTTP call.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn insert_gitlab_extras_account(&mut self, id: AccountId, extras: Arc<GitlabExtras>) {
        self.accounts
            .entry(id)
            .or_insert_with(|| ScmAccount::empty(Platform::Gitlab))
            .gitlab_extras = Some(extras);
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

    /// Resolve which account's `GithubExtras` handle serves `repo`,
    /// running the same rule engine `repo_for` runs (spec §6.2/§6.3):
    /// explicit `account` → owner rule → path rule → sole account of
    /// GitHub → error. This is the account-aware counterpart to
    /// [`github_extras`](Self::github_extras): that method is a
    /// back-compat shim that picks an account arbitrarily (bare-name
    /// first, else lexicographically-first), which is exactly the
    /// "invisible until a second account exists" defect this arc
    /// closes — a `workflows_dispatch` call actually mutates state on
    /// whichever GitHub account it resolves to, so guessing wrong here
    /// is worse than for a read-only diagnostic.
    ///
    /// `discover` always sets `github_extras` in the same branch as
    /// `repo`/`issues` (see the `Platform::Github` arm above), so an
    /// account can only be a candidate here if it also has extras —
    /// [`lookup_github_extras`](Self::lookup_github_extras) still
    /// checks, rather than assuming, in case a test seam registers the
    /// two independently.
    pub fn github_extras_for(
        &self,
        repo: &RepoRef,
        cwd: Option<&Path>,
        explicit: Option<&AccountId>,
    ) -> Result<(AccountId, Arc<GithubExtras>), AccountError> {
        let candidates = self.accounts_for_github_extras();
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
            | Resolution::SoleAccount(id) => self.lookup_github_extras(id, &candidates),
            Resolution::RuleTargetUnavailable { account, pattern } => {
                Err(AccountError::RuleTargetUnavailable {
                    account,
                    pattern,
                    platform: repo.platform.to_string(),
                })
            }
            Resolution::NoMatch { candidates } => Err(AccountError::NoRuleMatched {
                repo: format!("{}/{}", repo.owner, repo.repo),
                owner: repo.owner.clone(),
                platform: repo.platform.to_string(),
                candidates,
            }),
            Resolution::NoAccounts => Err(AccountError::NoAccounts {
                platform: repo.platform.to_string(),
            }),
        }
    }

    /// Every registered account with a built `GithubExtras` handle, in
    /// deterministic order. `github_extras_for`'s own candidate list —
    /// made `pub` (Arc 2 Task 6 review item 1) so a caller with no
    /// `RepoRef` at all (no owner, so no rule engine to run) can still
    /// implement the sole-account tier by hand: check this list has
    /// exactly one entry, then `github_extras_by_account` it directly.
    /// Deliberately NOT `accounts_for(Platform::Github)` (filtered on
    /// `a.repo.is_some()`) — `discover()`'s Github arm always builds
    /// `repo`/`issues`/`github_extras` together, so the two lists agree
    /// for every account `discover()` itself produces, but a test
    /// double built via `insert_github_extras_account` alone (no
    /// matching `insert_repo_account`) makes them diverge, and this is
    /// the field this candidate list is actually about.
    pub fn accounts_for_github_extras(&self) -> Vec<AccountId> {
        self.accounts
            .iter()
            .filter(|(_, a)| a.kind == Platform::Github && a.github_extras.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn lookup_github_extras(
        &self,
        id: AccountId,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<GithubExtras>), AccountError> {
        if candidates.is_empty() {
            return Err(AccountError::NoAccounts {
                platform: Platform::Github.to_string(),
            });
        }
        if let Some(acct) = self.accounts.get(&id) {
            if acct.kind == Platform::Github {
                if let Some(extras) = &acct.github_extras {
                    return Ok((id, extras.clone()));
                }
            }
        }
        Err(AccountError::UnknownAccount {
            requested: id,
            configured: candidates.to_vec(),
        })
    }

    /// Resolve which account's `GitlabExtras` handle serves `repo`. The
    /// `gitlab.pipeline_trigger` counterpart to
    /// [`github_extras_for`](Self::github_extras_for) — same rule
    /// engine, same reasoning for why the account-arbitrary
    /// [`gitlab_extras`](Self::gitlab_extras) shim isn't good enough for
    /// a mutating call.
    pub fn gitlab_extras_for(
        &self,
        repo: &RepoRef,
        cwd: Option<&Path>,
        explicit: Option<&AccountId>,
    ) -> Result<(AccountId, Arc<GitlabExtras>), AccountError> {
        let candidates = self.accounts_for_gitlab_extras();
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
            | Resolution::SoleAccount(id) => self.lookup_gitlab_extras(id, &candidates),
            Resolution::RuleTargetUnavailable { account, pattern } => {
                Err(AccountError::RuleTargetUnavailable {
                    account,
                    pattern,
                    platform: repo.platform.to_string(),
                })
            }
            Resolution::NoMatch { candidates } => Err(AccountError::NoRuleMatched {
                repo: format!("{}/{}", repo.owner, repo.repo),
                owner: repo.owner.clone(),
                platform: repo.platform.to_string(),
                candidates,
            }),
            Resolution::NoAccounts => Err(AccountError::NoAccounts {
                platform: repo.platform.to_string(),
            }),
        }
    }

    fn accounts_for_gitlab_extras(&self) -> Vec<AccountId> {
        self.accounts
            .iter()
            .filter(|(_, a)| a.kind == Platform::Gitlab && a.gitlab_extras.is_some())
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn lookup_gitlab_extras(
        &self,
        id: AccountId,
        candidates: &[AccountId],
    ) -> Result<(AccountId, Arc<GitlabExtras>), AccountError> {
        if candidates.is_empty() {
            return Err(AccountError::NoAccounts {
                platform: Platform::Gitlab.to_string(),
            });
        }
        if let Some(acct) = self.accounts.get(&id) {
            if acct.kind == Platform::Gitlab {
                if let Some(extras) = &acct.gitlab_extras {
                    return Ok((id, extras.clone()));
                }
            }
        }
        Err(AccountError::UnknownAccount {
            requested: id,
            configured: candidates.to_vec(),
        })
    }

    /// Returns the per-platform extras handle for GitHub actions, if
    /// GitHub credentials were present during discovery. Prefers the
    /// bare-vendor-named account, then the first (lowest `AccountId`)
    /// GitHub-kind account with extras (`find_map`, not
    /// `find(..).and_then(..)`: independent accounts can each have a
    /// distinct subset of a kind's fields populated, so stopping at the
    /// first *matching-kind* account rather than the first
    /// *matching-kind-with-this-field* account could return `None` past
    /// a real one — the same reasoning the deleted `repo`/`issues`/
    /// `events` shims documented before Arc 2 Task 6 removed them as
    /// dead code).
    /// Back-compat shim: `rupu-mcp`'s `github.workflows_dispatch` (a
    /// mutating call with a `RepoRef` in hand) migrated to
    /// [`github_extras_for`](Self::github_extras_for) in Arc 2 Task 5.
    /// `rupu-cli`'s `repos list` private-repo scope hint still calls
    /// this shim directly, deliberately gated to the single-GitHub-
    /// account case where "arbitrary" and "correct" are the same
    /// account (Arc 2 Task 4) — a read-only diagnostic, not a mutation.
    pub fn github_extras(&self) -> Option<Arc<GithubExtras>> {
        self.accounts
            .get(&AccountId::new("github"))
            .and_then(|a| a.github_extras.clone())
            .or_else(|| {
                self.accounts
                    .values()
                    .filter(|a| a.kind == Platform::Github)
                    .find_map(|a| a.github_extras.clone())
            })
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

    struct FakeEventConnector;

    #[async_trait]
    impl EventConnector for FakeEventConnector {
        async fn poll_events(
            &self,
            _source: &EventSourceRef,
            _cursor: Option<&str>,
            _limit: u32,
        ) -> Result<crate::event_connector::EventPollResult, ScmError> {
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

    /// `issues_for` counterpart to `fake_repo_connector` — same reasoning:
    /// the `IssueTracker` baked into the fake is irrelevant to resolution.
    fn fake_issue_connector() -> Arc<dyn IssueConnector> {
        Arc::new(FakeIssueConnector(IssueTracker::Github))
    }

    /// `events()` counterpart to `fake_repo_connector`/`fake_issue_connector`.
    fn fake_event_connector() -> Arc<dyn EventConnector> {
        Arc::new(FakeEventConnector)
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

    /// Final Arc 2 review item 1, exercised end-to-end through
    /// `Registry` rather than the pure `resolve_account` unit tests in
    /// `rules.rs`: `gh-work`'s credential never built (missing,
    /// revoked, or an expired refresh that failed), so it was never
    /// registered — `accounts_for(Github)` sees only `gh-personal`. The
    /// owner rule still names `gh-work`. Pre-fix, `repo_for` returned
    /// `Ok((gh-personal, ..))` here — a silent cross-identity misroute
    /// (this scenario, applied to `scm.prs.create`/`issues.comment`,
    /// is a write into the wrong org with no error at all). Post-fix it
    /// must error, and the error must name both the pattern and the
    /// unavailable account so the fix is actionable.
    #[tokio::test]
    async fn an_owner_rule_naming_an_account_with_no_live_connector_errors_instead_of_silently_switching(
    ) {
        let mut reg = Registry::default();
        // Only `gh-personal` actually built a connector — `gh-work` is
        // named by the rule but was never registered, exactly what
        // `discover` produces when `github::try_build` swallows a
        // credential-read failure for one of two configured accounts.
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
        let err = reg
            .repo_for(&r, None, None)
            .err()
            .expect("must error rather than silently resolve to gh-personal");
        assert!(
            matches!(err, AccountError::RuleTargetUnavailable { .. }),
            "{err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("acme/*"), "must name the rule: {msg}");
        assert!(
            msg.contains("gh-work"),
            "must name the target account: {msg}"
        );
        assert!(
            !msg.contains("gh-personal"),
            "must not suggest the account it did NOT route to: {msg}"
        );
    }

    /// Companion assertion to `an_owner_rule_selects_between_two_accounts`:
    /// the same rule must resolve through `issues_for`, not just
    /// `repo_for` — `rupu issues list --repo acme/api` builds a full
    /// `RepoRef` and must not discard the owner the way the pre-Arc-2
    /// call sites did (spec §1's original defect). Fails against a
    /// version of `issues_for` that calls `resolve_account` with
    /// `None, None, None` regardless of what `repo`/`cwd` it was handed.
    #[tokio::test]
    async fn an_owner_rule_selects_between_two_accounts_through_issues_for() {
        let mut reg = Registry::default();
        reg.insert_issue_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_issue_connector(),
        );
        reg.insert_issue_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_issue_connector(),
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
            .issues_for(IssueTracker::Github, Some(&r), None, None)
            .expect("owner rule must resolve through issues_for");
        assert_eq!(id, AccountId::new("gh-work"));
    }

    /// A dummy client never makes a network call in these tests — only
    /// construction and Registry lookup are exercised, matching
    /// `insert_github_extras_account`'s doc on why Arc/AccountId
    /// identity is a sufficient signal here.
    fn dummy_github_extras() -> Arc<GithubExtras> {
        use crate::connectors::github::GithubClient;
        Arc::new(GithubExtras::new(GithubClient::new(
            "unused-token".into(),
            None,
            None,
            Arc::new(rupu_netflow::NullSink),
        )))
    }

    fn dummy_gitlab_extras() -> Arc<GitlabExtras> {
        use crate::connectors::gitlab::GitlabClient;
        Arc::new(GitlabExtras::new(GitlabClient::new(
            "unused-token".into(),
            None,
            None,
            Arc::new(rupu_netflow::NullSink),
        )))
    }

    /// `github.workflows_dispatch`'s account-routing counterpart to
    /// `an_owner_rule_selects_between_two_accounts` — proves
    /// `github_extras_for` runs the same rule engine `repo_for` does,
    /// rather than falling back to `github_extras()`'s account-arbitrary
    /// bare-name-or-first-found shim (the defect `rupu-mcp`'s
    /// `github.workflows_dispatch` had before Arc 2 Task 5: a mutating
    /// call that could silently fire on the wrong GitHub account).
    #[tokio::test]
    async fn an_owner_rule_selects_between_two_accounts_through_github_extras_for() {
        crate::install_default_crypto_provider();
        let mut reg = Registry::default();
        reg.insert_github_extras_account(AccountId::new("gh-work"), dummy_github_extras());
        reg.insert_github_extras_account(AccountId::new("gh-personal"), dummy_github_extras());
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
            .github_extras_for(&r, None, None)
            .expect("owner rule must resolve through github_extras_for");
        assert_eq!(id, AccountId::new("gh-work"));

        // Explicit beats the rule, same precedence as repo_for/issues_for.
        let (id, _) = reg
            .github_extras_for(&r, None, Some(&AccountId::new("gh-personal")))
            .expect("explicit account must resolve through github_extras_for");
        assert_eq!(id, AccountId::new("gh-personal"));
    }

    /// `gitlab.pipeline_trigger`'s counterpart to the GitHub extras test
    /// above.
    #[tokio::test]
    async fn an_owner_rule_selects_between_two_accounts_through_gitlab_extras_for() {
        let mut reg = Registry::default();
        reg.insert_gitlab_extras_account(AccountId::new("gl-work"), dummy_gitlab_extras());
        reg.insert_gitlab_extras_account(AccountId::new("gl-personal"), dummy_gitlab_extras());
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gl-personal"),
        }]);
        let r = RepoRef {
            platform: Platform::Gitlab,
            owner: "acme".into(),
            repo: "api".into(),
        };
        let (id, _) = reg
            .gitlab_extras_for(&r, None, None)
            .expect("owner rule must resolve through gitlab_extras_for");
        assert_eq!(id, AccountId::new("gl-personal"));

        let (id, _) = reg
            .gitlab_extras_for(&r, None, Some(&AccountId::new("gl-work")))
            .expect("explicit account must resolve through gitlab_extras_for");
        assert_eq!(id, AccountId::new("gl-work"));
    }

    /// `events_for_source`'s `Repo` tier: same owner-rule resolution as
    /// `repo_for` (Task 6 migrated it off the account-arbitrary
    /// `events(Platform)` shim).
    #[tokio::test]
    async fn an_owner_rule_selects_between_two_accounts_through_events_for_source() {
        let mut reg = Registry::default();
        reg.insert_event_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.insert_event_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-personal"),
        }]);
        let source = EventSourceRef::Repo {
            repo: RepoRef {
                platform: Platform::Github,
                owner: "acme".into(),
                repo: "api".into(),
            },
        };
        let (id, _) = reg
            .events_for_source(&source, None, None)
            .expect("owner rule must resolve through events_for_source");
        assert_eq!(id, AccountId::new("gh-personal"));
    }

    /// Arc 2 Task 6 review item 2's regression: an explicit `account`
    /// override must work for a `Repo` source too, not just
    /// `TrackerProject`. Before the fix, `events_for_source` always
    /// passed `None` as the explicit tier for `Repo` sources — an
    /// `account = "..."` on a `github:owner/repo` poll_sources entry
    /// parsed cleanly (a known `PollSourceSpec` field) and then did
    /// nothing. Here the configured owner rule points at `gh-personal`;
    /// the explicit override names `gh-work` instead, and must win —
    /// explicit beats owner rule in `rules::resolve_account`'s
    /// precedence regardless of source kind.
    #[tokio::test]
    async fn explicit_account_overrides_an_owner_rule_through_events_for_source() {
        let mut reg = Registry::default();
        reg.insert_event_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.insert_event_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-personal"),
        }]);
        let source = EventSourceRef::Repo {
            repo: RepoRef {
                platform: Platform::Github,
                owner: "acme".into(),
                repo: "api".into(),
            },
        };
        let (id, _) = reg
            .events_for_source(&source, None, Some(&AccountId::new("gh-work")))
            .expect("explicit account must resolve through events_for_source");
        assert_eq!(id, AccountId::new("gh-work"));
    }

    /// A repo-backed `TrackerProject` (`project` = `owner/repo`) gets the
    /// owner split back out via `tracker_project_repo` and resolves
    /// through the SAME owner-rule tier as `Repo` — proving Task 6's
    /// investigation answer for item 3: splitting genuinely does make
    /// the explicit `account` field unnecessary for a repo-backed
    /// tracker, so this passes with `account: None`.
    #[tokio::test]
    async fn tracker_project_owner_rule_applies_when_project_splits() {
        let mut reg = Registry::default();
        reg.insert_event_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.insert_event_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-work"),
        }]);
        let source = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Github,
            project: "acme/api".into(),
            account: None,
        };
        let (id, _) = reg
            .events_for_source(&source, None, None)
            .expect("owner rule must apply to a split repo-backed tracker project");
        assert_eq!(id, AccountId::new("gh-work"));
    }

    /// Linear/Jira have no owner to split out of `project` (`"team-123"`
    /// isn't `owner/repo`) — the variant's own `account` field is the
    /// only lever (spec §6.5). Inserts directly into the private
    /// `tracker_event_connectors` map (this test lives inside the
    /// `registry` module via `use super::*`, so it has the same access
    /// discovery's own Linear/Jira segment does) since there is no
    /// public multi-account test seam for it. Genuinely single-account
    /// (`tracker_event_connectors` has no per-entry `IssueTracker` tag —
    /// unlike `tracker_accounts`, which pairs one in — so its candidate
    /// list, `accounts_for_tracker_events`, can only recognize the one
    /// bare-name key `discover` ever writes; that's `tracker_accounts`'s
    /// "not yet multi-account" constraint, not a new one introduced
    /// here). The claim this proves is narrower than the two-account
    /// owner-rule tests above: the `account` field is read and actually
    /// gates resolution (a wrong name is `UnknownAccount`, not a silent
    /// pass-through) — see the next test.
    #[tokio::test]
    async fn tracker_project_explicit_account_resolves_a_tracker_native_source() {
        let mut reg = Registry::default();
        reg.tracker_event_connectors
            .insert(AccountId::new("linear"), fake_event_connector());
        let source = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Linear,
            project: "team-123".into(),
            account: Some(AccountId::new("linear")),
        };
        let (id, _) = reg
            .events_for_source(&source, None, None)
            .expect("explicit account field must resolve a tracker-native source");
        assert_eq!(id, AccountId::new("linear"));
    }

    /// Companion to the test above: a typo'd `account` field must be
    /// `UnknownAccount`, not silently ignored or silently matched to
    /// the one real entry — the `account` field has teeth.
    #[tokio::test]
    async fn tracker_project_explicit_account_typo_is_unknown_account() {
        let mut reg = Registry::default();
        reg.tracker_event_connectors
            .insert(AccountId::new("linear"), fake_event_connector());
        let source = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Linear,
            project: "team-123".into(),
            account: Some(AccountId::new("linear-typo")),
        };
        let err = match reg.events_for_source(&source, None, None) {
            Err(e) => e,
            Ok(_) => panic!("expected UnknownAccount"),
        };
        assert!(
            matches!(err, AccountError::UnknownAccount { .. }),
            "{err:?}"
        );
    }

    /// Regression coverage for a bug caught in self-review: a
    /// `TrackerProject { tracker: Github, .. }` whose `project` does
    /// NOT split (no `/`) falls to the tracker-native branch of
    /// `events_for_source`, whose candidate list
    /// (`accounts_for_tracker_events`) correctly draws from
    /// `self.accounts` for a repo-backed tracker — but an earlier
    /// version of `lookup_events_tracker` looked ONLY in
    /// `tracker_event_connectors` regardless of tracker kind, so it
    /// always reported `UnknownAccount` for a github/gitlab id even
    /// though the account genuinely exists and has an events connector.
    /// This must resolve via the explicit `account` field exactly like
    /// the split-project case above.
    #[tokio::test]
    async fn tracker_project_explicit_account_resolves_repo_backed_tracker_with_unsplittable_project(
    ) {
        let mut reg = Registry::default();
        reg.insert_event_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_event_connector(),
        );
        let source = EventSourceRef::TrackerProject {
            tracker: IssueTracker::Github,
            project: "no-slash-here".into(),
            account: Some(AccountId::new("gh-work")),
        };
        let (id, _) = reg
            .events_for_source(&source, None, None)
            .expect("explicit account must resolve a repo-backed tracker with no owner/repo split");
        assert_eq!(id, AccountId::new("gh-work"));
    }

    /// Back-compat clause (spec §6.3's fourth tier): one GitHub account
    /// and no `[[scm.rules]]` resolves via `SoleAccount`, matching
    /// today's single-account behavior with zero config changes.
    #[tokio::test]
    async fn sole_account_resolves_events_for_source_with_no_rules() {
        let mut reg = Registry::default();
        reg.insert_event_account(
            AccountId::new("github"),
            Platform::Github,
            fake_event_connector(),
        );
        let source = EventSourceRef::Repo {
            repo: RepoRef {
                platform: Platform::Github,
                owner: "Section9Labs".into(),
                repo: "rupu".into(),
            },
        };
        let (id, _) = reg
            .events_for_source(&source, None, None)
            .expect("sole account must resolve with no rules configured");
        assert_eq!(id, AccountId::new("github"));
    }

    /// No GitHub account configured at all is a distinct failure
    /// (`NoAccounts`) from ambiguity (`NoRuleMatched`) — `cron.rs`'s
    /// caller treats these differently (silent skip vs. a warning).
    #[tokio::test]
    async fn events_for_source_reports_no_accounts_when_none_configured() {
        let reg = Registry::default();
        let source = EventSourceRef::Repo {
            repo: RepoRef {
                platform: Platform::Github,
                owner: "Section9Labs".into(),
                repo: "rupu".into(),
            },
        };
        let err = match reg.events_for_source(&source, None, None) {
            Err(e) => e,
            Ok(_) => panic!("expected NoAccounts"),
        };
        assert!(matches!(err, AccountError::NoAccounts { .. }), "{err:?}");
    }

    /// Two accounts and no rule that matches is `NoRuleMatched`, not a
    /// silent pick — the whole reason spec §6.4 exists.
    #[tokio::test]
    async fn events_for_source_reports_no_rule_matched_when_ambiguous() {
        let mut reg = Registry::default();
        reg.insert_event_account(
            AccountId::new("gh-work"),
            Platform::Github,
            fake_event_connector(),
        );
        reg.insert_event_account(
            AccountId::new("gh-personal"),
            Platform::Github,
            fake_event_connector(),
        );
        let source = EventSourceRef::Repo {
            repo: RepoRef {
                platform: Platform::Github,
                owner: "other".into(),
                repo: "thing".into(),
            },
        };
        let err = match reg.events_for_source(&source, None, None) {
            Err(e) => e,
            Ok(_) => panic!("expected NoRuleMatched"),
        };
        assert!(matches!(err, AccountError::NoRuleMatched { .. }), "{err:?}");
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

    /// Review item 5: `--account gh-work` with *zero* accounts configured
    /// at all must not fall through to `UnknownAccount` with a dangling
    /// empty candidate list — `resolve_account` returns `Explicit`
    /// before it ever looks at `candidates.is_empty()` (it's a pure
    /// function with no "log in" concept), so the existence check in
    /// `lookup_repo`/`lookup_issues` has to catch this itself. This is
    /// the "log in" failure, not the "typo" failure, and must get
    /// `NoAccounts`'s message, not `UnknownAccount`'s.
    #[tokio::test]
    async fn explicit_account_with_no_accounts_configured_is_no_accounts_not_unknown_account() {
        let reg = Registry::default();
        let r = RepoRef {
            platform: Platform::Github,
            owner: "acme".into(),
            repo: "api".into(),
        };
        let err = reg
            .repo_for(&r, None, Some(&AccountId::new("gh-work")))
            .err()
            .unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("auth login"),
            "must be the log-in error, not a dangling-list typo error: {msg}"
        );
        assert!(
            !msg.contains("no such account"),
            "must not present a log-in gap as an unknown-account typo: {msg}"
        );
    }

    /// Review item 4: the §6.4 fix line must be copy-pasteable — the
    /// suggested `--owner` glob is derived from the repo that failed to
    /// resolve, not a literal placeholder.
    #[tokio::test]
    async fn no_rule_matched_error_suggests_a_copy_pasteable_owner_glob() {
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
        let msg = reg.repo_for(&r, None, None).err().unwrap().to_string();
        assert!(
            msg.contains("--owner 'other/*'"),
            "fix line must be copy-pasteable from the failed repo's owner, not a placeholder: {msg}"
        );
    }

    // ── should_warn_unresolvable_kind (review round 3, finding 1) ──────

    /// A declared `[scm.gh-work]` (or any name that is neither a known
    /// `Platform` nor a known `IssueTracker`) with a missing/misspelled
    /// `kind` is the mistake this warn exists to catch.
    #[test]
    fn warns_on_a_declared_account_with_an_unresolvable_kind() {
        assert!(should_warn_unresolvable_kind("gh-work", true));
        assert!(should_warn_unresolvable_kind("gihub", true));
    }

    /// `[scm.jira]`/`[scm.linear]` are legitimate declared tables (e.g.
    /// carrying a self-hosted `base_url` — see `docs/using-rupu.md`'s
    /// `[scm.jira]` example) whose `kind` can never resolve to a
    /// `Platform`, because `Platform` only has `Github`/`Gitlab`. Must
    /// stay silent — this is the exact regression the coordinator caught
    /// in round 2's fix.
    #[test]
    fn does_not_warn_on_a_declared_tracker_only_account() {
        assert!(!should_warn_unresolvable_kind("jira", true));
        assert!(!should_warn_unresolvable_kind("linear", true));
    }

    /// The two implicit bare-name probes (`platform_cfg.is_none()`, i.e.
    /// `declared: false`) are the normal "no config for this built-in
    /// vendor" case — never a mistake, regardless of name.
    #[test]
    fn does_not_warn_when_the_account_was_not_declared() {
        assert!(!should_warn_unresolvable_kind("github", false));
        assert!(!should_warn_unresolvable_kind("gh-work", false));
        assert!(!should_warn_unresolvable_kind("anything", false));
    }
}
