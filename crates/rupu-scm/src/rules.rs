//! Account selection: which configured SCM account serves a given repo.
//!
//! A pure function with no I/O and no dependencies, so the entire
//! precedence table is unit-testable without a Registry, a network, or a
//! tracing-capture harness. This mirrors `resolve_configured_default` in
//! `crate::registry`, which was extracted for the same reason.
//!
//! Precedence: explicit → owner rule → path rule → sole account → error.
//! The sole-account tier is the back-compat clause: with one account
//! configured nothing can be ambiguous, so an existing single-account
//! user needs no rules and never sees the ambiguity error.

use std::path::Path;

use crate::account::AccountId;
use crate::types::RepoRef;

/// One account-selection rule. Exactly one of `owner` / `path` is
/// meaningful; a rule with neither can never match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub owner: Option<String>,
    pub path: Option<String>,
    pub account: AccountId,
}

impl Rule {
    /// Convert a config-parsed `[[scm.rules]]` entry (`rupu_config::ScmRule`,
    /// wire shape: `account: String`) into the domain `Rule` this module's
    /// `resolve_account` consumes (`account: AccountId`). Lives beside the
    /// type it builds rather than in `rupu-config` — `rupu-config` has no
    /// reason to know about `AccountId`. `Registry::discover` is the sole
    /// caller: `rupu-scm` already depends on `rupu-config`, so no CLI-side
    /// helper and no rule list threaded through call sites.
    pub fn from_config(r: &rupu_config::ScmRule) -> Self {
        Self {
            owner: r.owner.clone(),
            path: r.path.clone(),
            account: AccountId::new(r.account.clone()),
        }
    }
}

/// How an account was chosen — or why it could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The caller named it (`--account`, or an MCP `account` argument).
    Explicit(AccountId),
    /// An owner rule matched. This is the tier daemons rely on: a
    /// webhook payload or cron poll knows the owner but has no cwd.
    Owner(AccountId),
    /// A path rule matched the caller's cwd.
    Path(AccountId),
    /// Nothing matched, but only one account is configured for this
    /// platform, so there is nothing to be ambiguous about.
    SoleAccount(AccountId),
    /// An owner or path rule's pattern matched, and the account it
    /// names is not registered ANYWHERE — not merely absent from this
    /// call's platform/capability-narrowed `candidates`, but unknown to
    /// every `all_accounts` entry too. This is the "dead identity"
    /// case: the credential is missing, revoked, or expired with a
    /// failed refresh, so `discover` never registered it under any
    /// platform. Distinct from [`Self::NoMatch`]: a rule DID fire here,
    /// so falling through to a different candidate (e.g. the
    /// sole-account tier, if exactly one other account happens to be
    /// configured) would silently target the wrong identity — the one
    /// thing this precedence table promises never happens.
    ///
    /// Deliberately NOT returned when the named account merely isn't a
    /// candidate for *this* call but IS registered under a different
    /// platform or capability (a GitHub-named path rule evaluated for a
    /// GitLab repo, say) — that case still skips the rule and falls
    /// through, exactly as it always has, because there is nothing
    /// wrong with that account; it just isn't the one this particular
    /// operation needs. `pattern` is the rule's own owner/path text,
    /// carried through so the caller can build a message pointing at
    /// the specific rule to fix.
    RuleTargetUnavailable { account: AccountId, pattern: String },
    /// Several accounts are configured and nothing selected one.
    NoMatch { candidates: Vec<AccountId> },
    /// No accounts at all — a different failure from ambiguity, and it
    /// deserves a different message (log in, rather than disambiguate).
    NoAccounts,
}

/// Match a glob pattern against a value.
///
/// Deliberately tiny: a trailing `/*` or a bare `*` are the only
/// wildcards, because those are the only shapes the documented rule
/// forms use. A full glob crate would be a dependency and a much larger
/// surface to reason about for no gain.
pub fn glob_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return value == prefix || value.starts_with(&format!("{prefix}/"));
    }
    pattern == value
}

/// Expand a leading `~` against `$HOME`. Returns the pattern unchanged
/// when there is no leading `~` or no resolvable home directory — a
/// rule that cannot be expanded simply never matches, rather than
/// matching the wrong thing.
pub fn expand_tilde(pattern: &str, home: Option<&Path>) -> String {
    match (pattern.strip_prefix("~/"), home) {
        (Some(rest), Some(h)) => format!("{}/{}", h.to_string_lossy(), rest),
        _ => pattern.to_string(),
    }
}

/// Resolve which account serves `repo`, given the configured rules and
/// the accounts that actually exist.
///
/// `candidates` must already be narrowed to accounts of the right
/// platform AND capability for this call — this function does not know
/// about `Platform`, which keeps it pure and keeps that filtering in
/// one place (the Registry). `all_accounts` is the wider set: every
/// account registered anywhere, of any platform/capability. `Rule`
/// carries no platform of its own (a rule's `account` can name an
/// identity of any kind), so the two sets matter for different
/// questions: "is this rule's account usable for *this* call" is
/// `candidates`; "does this rule's account exist at all, under some
/// other platform or capability" is `all_accounts`. A rule whose
/// pattern matches but whose account fails the first check and passes
/// the second is not an error — it just isn't this call's rule, and the
/// engine falls through to the next tier exactly as if the rule had
/// never matched. A rule whose account fails BOTH is the real defect
/// this engine guards against: see [`Resolution::RuleTargetUnavailable`].
pub fn resolve_account(
    rules: &[Rule],
    repo: Option<&RepoRef>,
    cwd: Option<&Path>,
    home: Option<&Path>,
    explicit: Option<&AccountId>,
    candidates: &[AccountId],
    all_accounts: &[AccountId],
) -> Resolution {
    if let Some(a) = explicit {
        return Resolution::Explicit(a.clone());
    }
    if candidates.is_empty() {
        return Resolution::NoAccounts;
    }

    let known = |a: &AccountId| candidates.contains(a);
    let known_anywhere = |a: &AccountId| all_accounts.contains(a);

    if let Some(r) = repo {
        for rule in rules {
            if let Some(pat) = &rule.owner {
                if glob_matches(pat, &r.owner) {
                    if known(&rule.account) {
                        return Resolution::Owner(rule.account.clone());
                    }
                    if !known_anywhere(&rule.account) {
                        // The pattern matched, and the account it names
                        // doesn't exist under ANY platform/capability —
                        // a dead identity, not merely the wrong domain
                        // for this call. Stop here rather than
                        // continuing to the next rule (or the
                        // sole-account tier below) — continuing is
                        // exactly the fall-through that lets a matched
                        // rule silently route to a different identity.
                        return Resolution::RuleTargetUnavailable {
                            account: rule.account.clone(),
                            pattern: pat.clone(),
                        };
                    }
                    // Live account, just of a different
                    // platform/capability than this call needs (e.g. a
                    // GitHub-named rule evaluated for a GitLab repo).
                    // Nothing wrong with it — skip this rule and let
                    // the next rule, or a later tier, have a chance,
                    // same as always.
                }
            }
        }
    }

    if let Some(dir) = cwd {
        let dir_s = dir.to_string_lossy();
        for rule in rules {
            if let Some(pat) = &rule.path {
                let expanded = expand_tilde(pat, home);
                if glob_matches(&expanded, &dir_s) {
                    if known(&rule.account) {
                        return Resolution::Path(rule.account.clone());
                    }
                    if !known_anywhere(&rule.account) {
                        return Resolution::RuleTargetUnavailable {
                            account: rule.account.clone(),
                            pattern: pat.clone(),
                        };
                    }
                }
            }
        }
    }

    if candidates.len() == 1 {
        return Resolution::SoleAccount(candidates[0].clone());
    }

    Resolution::NoMatch {
        candidates: candidates.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::types::RepoRef;
    use std::path::PathBuf;

    fn repo(owner: &str, name: &str) -> RepoRef {
        RepoRef {
            platform: Platform::Github,
            owner: owner.to_string(),
            repo: name.to_string(),
        }
    }

    fn rule_owner(pat: &str, acct: &str) -> Rule {
        Rule {
            owner: Some(pat.into()),
            path: None,
            account: AccountId::new(acct),
        }
    }

    fn rule_path(pat: &str, acct: &str) -> Rule {
        Rule {
            owner: None,
            path: Some(pat.into()),
            account: AccountId::new(acct),
        }
    }

    fn accounts(names: &[&str]) -> Vec<AccountId> {
        names.iter().map(|n| AccountId::new(*n)).collect()
    }

    #[test]
    fn explicit_beats_every_rule() {
        let rules = vec![rule_owner("acme/*", "gh-work")];
        let got = resolve_account(
            &rules,
            Some(&repo("acme", "api")),
            None,
            None,
            Some(&AccountId::new("gh-personal")),
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(got, Resolution::Explicit(AccountId::new("gh-personal")));
    }

    #[test]
    fn owner_rule_beats_path_rule() {
        let rules = vec![
            rule_path("/home/me/work/*", "gh-path"),
            rule_owner("acme/*", "gh-owner"),
        ];
        let got = resolve_account(
            &rules,
            Some(&repo("acme", "api")),
            Some(&PathBuf::from("/home/me/work/api")),
            None,
            None,
            &accounts(&["gh-path", "gh-owner"]),
            &accounts(&["gh-path", "gh-owner"]),
        );
        assert_eq!(got, Resolution::Owner(AccountId::new("gh-owner")));
    }

    #[test]
    fn path_rule_applies_when_no_owner_rule_matches() {
        let rules = vec![
            rule_owner("other/*", "gh-other"),
            rule_path("/home/me/work/*", "gh-work"),
        ];
        let got = resolve_account(
            &rules,
            Some(&repo("acme", "api")),
            Some(&PathBuf::from("/home/me/work/api")),
            None,
            None,
            &accounts(&["gh-other", "gh-work"]),
            &accounts(&["gh-other", "gh-work"]),
        );
        assert_eq!(got, Resolution::Path(AccountId::new("gh-work")));
    }

    /// The back-compat clause, and the most important test in this file:
    /// one account configured means nothing can be ambiguous, so no
    /// rules are needed and nothing errors.
    #[test]
    fn sole_account_wins_when_nothing_matches() {
        let got = resolve_account(
            &[],
            Some(&repo("whoever", "whatever")),
            None,
            None,
            None,
            &accounts(&["gh-only"]),
            &accounts(&["gh-only"]),
        );
        assert_eq!(got, Resolution::SoleAccount(AccountId::new("gh-only")));
    }

    #[test]
    fn ambiguous_with_no_matching_rule_reports_candidates() {
        let got = resolve_account(
            &[rule_owner("acme/*", "gh-work")],
            Some(&repo("other", "thing")),
            None,
            None,
            None,
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::NoMatch {
                candidates: accounts(&["gh-work", "gh-personal"])
            }
        );
    }

    #[test]
    fn no_configured_accounts_is_its_own_outcome() {
        let got = resolve_account(&[], Some(&repo("a", "b")), None, None, None, &[], &[]);
        assert_eq!(got, Resolution::NoAccounts);
    }

    /// Daemons have an owner but no cwd — a path rule must not fire, and
    /// must not panic on the absent cwd.
    #[test]
    fn path_rules_are_skipped_when_there_is_no_cwd() {
        let got = resolve_account(
            &[rule_path("/home/me/work/*", "gh-work")],
            Some(&repo("acme", "api")),
            None,
            None,
            None,
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::NoMatch {
                candidates: accounts(&["gh-work", "gh-personal"])
            }
        );
    }

    /// An owner rule naming an account that is not configured must not
    /// select it — otherwise a typo in config silently routes to nothing.
    /// With two OTHER live candidates still around, the old behavior
    /// (fall through to the next rule, then to ambiguity) landed in
    /// `NoMatch`. The fix makes a matched-but-unavailable rule stop
    /// unconditionally rather than continue, so this now reports
    /// `RuleTargetUnavailable` instead — an error that names the actual
    /// problem (the rule's target has no credential) rather than a
    /// generic "several accounts, none selected" message that doesn't
    /// mention the rule at all.
    #[test]
    fn a_rule_naming_an_unconfigured_account_does_not_match() {
        let got = resolve_account(
            &[rule_owner("acme/*", "gh-typo")],
            Some(&repo("acme", "api")),
            None,
            None,
            None,
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::RuleTargetUnavailable {
                account: AccountId::new("gh-typo"),
                pattern: "acme/*".into(),
            }
        );
    }

    /// The dangerous case item 1 of the Arc 2 final review flagged: only
    /// ONE other candidate remains once the rule's named account is
    /// excluded. Pre-fix, `known(&rule.account)` being false made the
    /// loop fall through past the rule entirely, straight to
    /// `candidates.len() == 1` — which silently returned
    /// `SoleAccount(gh-personal)`, routing a request that an owner rule
    /// explicitly earmarked for `gh-work` onto a completely different
    /// account with no error at all. This is the genuine RED this arc's
    /// fix closes: the two-candidate sibling above never exercised this
    /// branch because `NoMatch` was also its pre-fix outcome.
    #[test]
    fn a_rule_naming_an_unconfigured_account_does_not_fall_through_to_sole_account() {
        let got = resolve_account(
            &[rule_owner("acme/*", "gh-work")],
            Some(&repo("acme", "api")),
            None,
            None,
            None,
            &accounts(&["gh-personal"]),
            &accounts(&["gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::RuleTargetUnavailable {
                account: AccountId::new("gh-work"),
                pattern: "acme/*".into(),
            }
        );
    }

    /// The path-tier mirror of `a_rule_naming_an_unconfigured_account_does_not_match`:
    /// the owner tier's `known()` guard already has this test; the path
    /// tier's identical guard did not. The code is symmetric — this
    /// closes the coverage gap rather than hunting a bug. See that
    /// test's doc for why the expected `Resolution` changed from
    /// `NoMatch` to `RuleTargetUnavailable`.
    #[test]
    fn a_path_rule_naming_an_unconfigured_account_does_not_match() {
        let got = resolve_account(
            &[rule_path("/home/me/work/*", "gh-typo")],
            None,
            Some(&PathBuf::from("/home/me/work/api")),
            None,
            None,
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::RuleTargetUnavailable {
                account: AccountId::new("gh-typo"),
                pattern: "/home/me/work/*".into(),
            }
        );
    }

    /// The path-tier twin of
    /// `a_rule_naming_an_unconfigured_account_does_not_fall_through_to_sole_account`:
    /// one other candidate remains, so the pre-fix code silently landed
    /// on `SoleAccount(gh-personal)` for a path rule that named
    /// `gh-work`. Same defect, same fix, same genuine RED.
    #[test]
    fn a_path_rule_naming_an_unconfigured_account_does_not_fall_through_to_sole_account() {
        let got = resolve_account(
            &[rule_path("/home/me/work/*", "gh-work")],
            None,
            Some(&PathBuf::from("/home/me/work/api")),
            None,
            None,
            &accounts(&["gh-personal"]),
            &accounts(&["gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::RuleTargetUnavailable {
                account: AccountId::new("gh-work"),
                pattern: "/home/me/work/*".into(),
            }
        );
    }

    /// Final Arc 2 review, fix wave 2, finding 1: `Rule` carries no
    /// platform of its own, so a path rule naming a GitHub account
    /// matches on `cwd` alone regardless of what platform the current
    /// call is resolving for. Before this fix, `known(&rule.account)`
    /// being false for `gh-work` here (it isn't a GitLab candidate,
    /// it's a live GitHub account) was indistinguishable from `gh-work`
    /// not existing anywhere, so the fix in the previous review round
    /// over-corrected: it would have hard-errored a perfectly working
    /// GitLab resolution just because a same-directory GitHub rule
    /// happened to match first. `gh-work` here IS registered,
    /// `all_accounts` says so, just not as a GitLab candidate, so the
    /// rule is skipped (not treated as unavailable) and resolution
    /// falls through to the sole remaining GitLab candidate, exactly
    /// as it did before Arc 2's rule engine work existed at all.
    #[test]
    fn a_path_rule_naming_a_live_account_of_a_different_platform_is_skipped_not_unavailable() {
        let got = resolve_account(
            &[rule_path("/home/me/work/*", "gh-work")],
            None,
            Some(&PathBuf::from("/home/me/work/api")),
            None,
            None,
            &accounts(&["gl-work"]),
            &accounts(&["gh-work", "gl-work"]),
        );
        assert_eq!(got, Resolution::SoleAccount(AccountId::new("gl-work")));
    }

    /// Owner-tier twin of the path-tier test above: a `gh-work`-naming
    /// owner rule matched against a GitLab repo op, with `gh-work` live
    /// under GitHub but absent from the GitLab-narrowed `candidates`.
    #[test]
    fn an_owner_rule_naming_a_live_account_of_a_different_platform_is_skipped_not_unavailable() {
        let got = resolve_account(
            &[rule_owner("acme/*", "gh-work")],
            Some(&repo("acme", "api")),
            None,
            None,
            None,
            &accounts(&["gl-work"]),
            &accounts(&["gh-work", "gl-work"]),
        );
        assert_eq!(got, Resolution::SoleAccount(AccountId::new("gl-work")));
    }

    #[test]
    fn from_config_converts_wire_rule_to_domain_rule() {
        let owner_rule = rupu_config::ScmRule {
            owner: Some("acme/*".into()),
            path: None,
            account: "gh-work".into(),
        };
        assert_eq!(
            Rule::from_config(&owner_rule),
            rule_owner("acme/*", "gh-work")
        );

        let path_rule = rupu_config::ScmRule {
            owner: None,
            path: Some("~/Code/work/*".into()),
            account: "gh-work".into(),
        };
        assert_eq!(
            Rule::from_config(&path_rule),
            rule_path("~/Code/work/*", "gh-work")
        );
    }

    #[test]
    fn first_matching_rule_of_a_tier_wins() {
        let rules = vec![
            rule_owner("acme/*", "gh-first"),
            rule_owner("acme/*", "gh-second"),
        ];
        let got = resolve_account(
            &rules,
            Some(&repo("acme", "api")),
            None,
            None,
            None,
            &accounts(&["gh-first", "gh-second"]),
            &accounts(&["gh-first", "gh-second"]),
        );
        assert_eq!(got, Resolution::Owner(AccountId::new("gh-first")));
    }

    #[test]
    fn owner_pattern_matches_bare_owner_and_glob() {
        assert!(glob_matches("acme", "acme"));
        assert!(glob_matches("acme/*", "acme"));
        assert!(!glob_matches("acme/*", "acmecorp"));
        assert!(!glob_matches("acme", "other"));
        assert!(glob_matches("*", "anything"));
    }

    #[test]
    fn path_pattern_matches_prefix_glob() {
        assert!(glob_matches("/home/me/work/*", "/home/me/work/api"));
        assert!(glob_matches("/home/me/work/*", "/home/me/work/deep/nested"));
        assert!(!glob_matches("/home/me/work/*", "/home/me/personal/api"));
    }

    #[test]
    fn glob_matching_is_case_sensitive() {
        assert!(!glob_matches("Acme/*", "acme"));
    }

    #[test]
    fn tilde_path_rule_matches_when_home_is_known() {
        let rules = vec![rule_path("~/Code/work/*", "gh-work")];
        let got = resolve_account(
            &rules,
            None,
            Some(&PathBuf::from("/home/me/Code/work/api")),
            Some(&PathBuf::from("/home/me")),
            None,
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(got, Resolution::Path(AccountId::new("gh-work")));
    }

    /// Without a resolvable home, a `~`-prefixed pattern must not be
    /// expanded into something that happens to match — it should simply
    /// never match, falling through to ambiguity.
    #[test]
    fn tilde_path_rule_does_not_match_when_home_is_unknown() {
        let rules = vec![rule_path("~/Code/work/*", "gh-work")];
        let got = resolve_account(
            &rules,
            None,
            Some(&PathBuf::from("/home/me/Code/work/api")),
            None,
            None,
            &accounts(&["gh-work", "gh-personal"]),
            &accounts(&["gh-work", "gh-personal"]),
        );
        assert_eq!(
            got,
            Resolution::NoMatch {
                candidates: accounts(&["gh-work", "gh-personal"])
            }
        );
    }

    #[test]
    fn expand_tilde_leaves_non_tilde_patterns_unchanged() {
        assert_eq!(
            expand_tilde("/home/me/work/*", Some(&PathBuf::from("/home/me"))),
            "/home/me/work/*"
        );
    }

    #[test]
    fn expand_tilde_substitutes_home_for_leading_tilde() {
        assert_eq!(
            expand_tilde("~/Code/work/*", Some(&PathBuf::from("/home/me"))),
            "/home/me/Code/work/*"
        );
    }

    #[test]
    fn expand_tilde_returns_pattern_unchanged_without_home() {
        assert_eq!(expand_tilde("~/Code/work/*", None), "~/Code/work/*");
    }
}
