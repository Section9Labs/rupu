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
/// platform — this function does not know about `Platform`, which keeps
/// it pure and keeps platform filtering in one place (the Registry).
pub fn resolve_account(
    rules: &[Rule],
    repo: Option<&RepoRef>,
    cwd: Option<&Path>,
    home: Option<&Path>,
    explicit: Option<&AccountId>,
    candidates: &[AccountId],
) -> Resolution {
    if let Some(a) = explicit {
        return Resolution::Explicit(a.clone());
    }
    if candidates.is_empty() {
        return Resolution::NoAccounts;
    }

    let known = |a: &AccountId| candidates.contains(a);

    if let Some(r) = repo {
        for rule in rules {
            if let Some(pat) = &rule.owner {
                if glob_matches(pat, &r.owner) && known(&rule.account) {
                    return Resolution::Owner(rule.account.clone());
                }
            }
        }
    }

    if let Some(dir) = cwd {
        let dir_s = dir.to_string_lossy();
        for rule in rules {
            if let Some(pat) = &rule.path {
                let expanded = expand_tilde(pat, home);
                if glob_matches(&expanded, &dir_s) && known(&rule.account) {
                    return Resolution::Path(rule.account.clone());
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
        let got = resolve_account(&[], Some(&repo("a", "b")), None, None, None, &[]);
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
    #[test]
    fn a_rule_naming_an_unconfigured_account_does_not_match() {
        let got = resolve_account(
            &[rule_owner("acme/*", "gh-typo")],
            Some(&repo("acme", "api")),
            None,
            None,
            None,
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
