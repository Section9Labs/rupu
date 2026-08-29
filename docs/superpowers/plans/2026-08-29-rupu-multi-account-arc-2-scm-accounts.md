# Multi-Account Providers — Arc 2 (SCM Named Accounts) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user hold several GitHub (or GitLab) accounts at once — work and personal, or github.com alongside a GitHub Enterprise host — and have every command, workflow, and daemon reach the right one without ever silently targeting the wrong identity.

**Architecture:** Arc 1 established the identity model for LLM providers: the account name is the identity, `kind` is the vendor. Arc 2 applies the same split to `rupu-scm`. `Platform` stays exactly as it is — it *is* the vendor, and `RepoRef.platform` is correctly typed — so only the `Registry`'s map key changes, from `Platform` to a new `AccountId`. Selection, which does not exist today, comes from a pure rule engine keyed on repo owner and filesystem path.

**Tech Stack:** Rust 2021, tokio, thiserror (libs) / anyhow (CLI), clap, toml + toml_edit, tracing, octocrab (GitHub), reqwest.

**Spec:** `docs/superpowers/specs/2026-08-28-rupu-multi-account-providers-design.md` — §6 is Arc 2's binding design. §3.1 and §4 (the back-compat rule and the credential key format) bind this arc too.

**Depends on:** Arc 1 (PR #627). This plan branches from Arc 1's tip and rebases onto `main` once that lands.

## Global Constraints

- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- Workspace deps only — versions pinned in root `Cargo.toml`, never in a crate `Cargo.toml`.
- Hexagonal rule 1 applies to `rupu-auth`, which must not depend on `rupu-config`. **It does NOT apply to `rupu-scm`**, which has always depended on `rupu-config` — `Registry::discover` takes `&Config` today. An earlier draft of this plan asserted otherwise and was wrong; see Ruling 3 in the ledger. The invariant that does hold and must be preserved: `rupu-config` does not depend on `rupu-scm` (verified), so `AccountId` stays out of `rupu-config` and `ScmRule.account` stays a `String`.
- `rupu-cli` is thin: arg parsing + delegation.
- Errors: `thiserror` in libraries, `anyhow` in `rupu-cli`.
- **Never run `cargo fmt`, even with explicit paths.** `cargo fmt -- <file>` does NOT filter by path — the `--` args are rustfmt flags and cargo still enumerates every workspace target; `main` is fmt-dirty under the pinned toolchain, so this produces a huge bogus diff (it reformatted 81 unrelated files during Arc 1). Use `rustfmt --edition 2021 <file>`, and **never on a crate root** (`lib.rs`/`main.rs`) — rustfmt follows `mod` declarations and recurses the whole tree. `git diff --name-only` before staging.
- **Do not modify `crates/rupu-auth/src/account_key.rs`.** Credential keys stay `<account>/<mode>`.
- **The back-compat clause is load-bearing in every task:** with exactly one account configured for a platform, nothing can be ambiguous, so no rules are needed and nothing errors. Every existing single-account user must see zero behavior change and write no config.
- Never `git stash` / `git stash pop` — the stash is shared across sessions.
- Do not commit to `main`.

## Vendor / platform strings

`Platform` is `Github | Gitlab`. `IssueTracker` is `Github | Gitlab | Linear | Jira`. Both stay as-is. `[scm.<account>].kind` accepts `github` and `gitlab`; `[issues.<account>].kind` additionally accepts `linear` and `jira`.

## Known call-site surface (measured, not estimated)

**A note on Tasks 4–6's specificity.** Tasks 1–3 carry complete code, because they invent new types and logic. Tasks 4–6 are migrations across ~39 call sites, and they deliberately give the before/after *pattern* with real code plus an exact file-and-count list, rather than 39 hand-written hunks. That is a considered trade: enumerating every site from this plan would mean transcribing code I have not read at each one, which produces confident-looking text that is wrong at the third site. The implementer must open each site. What the plan owes them — which shape applies, what the migrated call looks like, what must not change, and what to do when a site does not fit — is specified. **If a site does not match any of the three shapes, stop and report it rather than forcing a fit.**

Production `Registry` accessor sites outside `rupu-scm` itself: **`rupu-mcp` 19** (`issues.rs` 6, `scm_prs.rs` 5, `scm_repos.rs` 3, `scm_branches.rs` 2, `scm_files.rs` 1, `github_extras.rs` 1, `gitlab_extras.rs` 1) and **`rupu-cli` 20** (`autoflow.rs` 6, `autoflow_wake.rs` 3, `workflow.rs` 2, `repos.rs` 2, `issues.rs` 2, `cp_repos.rs` 1, `cp_inventory.rs` 1, `session.rs` 1, `run.rs` 1, `cron.rs` 1). Plus 16 accessor uses and 19 `insert_*_connector` seams in tests.

---

### Task 1: `AccountId`, `ScmAccount`, and account-keyed config

Foundation. Adds types and config shape; changes no behavior. `Registry` still keys on `Platform` after this task.

**Files:**
- Create: `crates/rupu-scm/src/account.rs`
- Modify: `crates/rupu-scm/src/lib.rs` (declare + re-export)
- Modify: `crates/rupu-config/src/scm_config.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `rupu_scm::account::AccountId(pub String)` with `AccountId::new(impl Into<String>)`, `as_str()`, `Display`, `PartialEq`/`Eq`/`Hash`/`Clone`/`Debug`, `Ord` (so listings are deterministic).
  - `rupu_config::ScmPlatformConfig` gains `pub kind: Option<String>`.
  - `rupu_config::ScmSection` gains `pub rules: Vec<ScmRule>`; `ScmRule { owner: Option<String>, path: Option<String>, account: String }`.

- [ ] **Step 1: Write the failing test for `AccountId`**

Create `crates/rupu-scm/src/account.rs` with:

```rust
//! Account identity for SCM connectors.
//!
//! Mirrors the model Arc 1 established for LLM providers: the account
//! name is the identity, `kind` is the vendor. `Platform` stays the
//! vendor — it is correctly typed as such on `RepoRef` — so only the
//! `Registry`'s map key becomes an `AccountId`.

use std::fmt;

/// The name of one configured SCM account, e.g. `gh-work`.
///
/// Ordered so account listings and fan-out results are deterministic
/// rather than hash-ordered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(pub String);

impl AccountId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for AccountId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_round_trips_and_displays() {
        let a = AccountId::new("gh-work");
        assert_eq!(a.as_str(), "gh-work");
        assert_eq!(a.to_string(), "gh-work");
        assert_eq!(AccountId::from("gh-work"), a);
    }

    #[test]
    fn account_ids_sort_deterministically() {
        let mut v = vec![
            AccountId::new("gh-work"),
            AccountId::new("acme-ghe"),
            AccountId::new("gh-personal"),
        ];
        v.sort();
        assert_eq!(
            v.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
            ["acme-ghe", "gh-personal", "gh-work"]
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-scm account_id`
Expected: FAIL — module not declared, does not compile.

- [ ] **Step 3: Declare and export the module**

In `crates/rupu-scm/src/lib.rs`, add beside the other `pub mod` lines (hand-write this line; do NOT rustfmt the crate root):

```rust
pub mod account;
pub use account::AccountId;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-scm account`
Expected: PASS (2 tests)

- [ ] **Step 5: Write the failing config tests**

Append to `mod tests` in `crates/rupu-config/src/scm_config.rs`:

```rust
    #[test]
    fn scm_account_declares_a_kind() {
        let toml = r#"
[gh-work]
kind = "github"

[acme-ghe]
kind = "github"
base_url = "https://git.acme.internal/api/v3"
"#;
        let sec: ScmSection = toml::from_str(toml).unwrap();
        assert_eq!(
            sec.platforms.get("gh-work").and_then(|p| p.kind.as_deref()),
            Some("github")
        );
        assert_eq!(
            sec.platforms
                .get("acme-ghe")
                .and_then(|p| p.base_url.as_deref()),
            Some("https://git.acme.internal/api/v3")
        );
    }

    /// Back-compat: `[scm.github]` with no `kind` is still valid — the
    /// account name IS the platform, exactly as spec §3.1 does for LLM
    /// providers.
    #[test]
    fn bare_platform_table_still_parses_without_kind() {
        let sec: ScmSection = toml::from_str("[github]\ntimeout_ms = 5000\n").unwrap();
        let gh = sec.platforms.get("github").unwrap();
        assert_eq!(gh.kind, None);
        assert_eq!(gh.timeout_ms, Some(5000));
    }

    #[test]
    fn rules_parse_owner_and_path_forms() {
        let toml = r#"
[[rules]]
owner = "acme/*"
account = "gh-work"

[[rules]]
path = "~/Code/work/*"
account = "gh-work"
"#;
        let sec: ScmSection = toml::from_str(toml).unwrap();
        assert_eq!(sec.rules.len(), 2);
        assert_eq!(sec.rules[0].owner.as_deref(), Some("acme/*"));
        assert_eq!(sec.rules[0].account, "gh-work");
        assert_eq!(sec.rules[1].path.as_deref(), Some("~/Code/work/*"));
        assert!(sec.rules[1].owner.is_none());
    }

    /// `rules` is a reserved key like `default` — it must not be
    /// swallowed by the flattened per-account map.
    #[test]
    fn rules_is_not_treated_as_an_account() {
        let sec: ScmSection =
            toml::from_str("[[rules]]\nowner = \"a/*\"\naccount = \"x\"\n").unwrap();
        assert!(!sec.platforms.contains_key("rules"));
    }
```

- [ ] **Step 6: Run to verify they fail**

Run: `cargo test -p rupu-config scm_config`
Expected: FAIL — `ScmPlatformConfig` has no `kind`; `ScmSection` has no `rules`.

- [ ] **Step 7: Add `kind` and `rules` to the config types**

In `crates/rupu-config/src/scm_config.rs`, add to `ScmPlatformConfig` (before `base_url`):

```rust
    /// The platform this account talks to (`github` / `gitlab`). `None`
    /// means the account name IS the platform — the back-compat rule
    /// from design spec §3.1, which is what keeps a pre-existing
    /// `[scm.github]` table working with no edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
```

Add the rule type:

```rust
/// One account-selection rule. Exactly one of `owner` / `path` is set;
/// a rule with both, or neither, is a config error (validated in
/// `Config::validate`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmRule {
    /// Owner glob, e.g. `acme/*` or `acme`. Matched against
    /// `RepoRef.owner`. This is the form that works for daemons, which
    /// know the owner but have no cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Filesystem path glob, e.g. `~/Code/work/*`. Matched against the
    /// caller's cwd. Covers the interactive case before a remote is
    /// known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The account this rule selects.
    pub account: String,
}
```

Add to `ScmSection`:

```rust
    /// Ordered account-selection rules. First match wins within a tier;
    /// see `rupu_scm::rules::resolve_account` for the precedence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ScmRule>,
```

In the `platforms_serde` module's `deserialize`, drop the reserved `rules` key exactly as `default` is already dropped:

```rust
        raw.remove("default");
        raw.remove("rules");
```

Note: `rules` deserializes as an array of tables, which will not coerce into `ScmPlatformConfig`. Removing it from the flattened map after the fact is not sufficient on its own — implement `platforms_serde::deserialize` to work over a `toml::Value`-shaped intermediate (or `serde_json::Value` equivalent) so a non-table value under `rules` is skipped rather than erroring. Verify with the `rules_is_not_treated_as_an_account` test; if the simple `remove` suffices in practice, keep it simple and say so in your report.

- [ ] **Step 8: Run to verify they pass**

Run: `cargo test -p rupu-config`
Expected: PASS, including the pre-existing `ScmSection` round-trip tests.

- [ ] **Step 9: Lint, format, commit**

```bash
cargo clippy -p rupu-scm -p rupu-config --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-scm/src/account.rs crates/rupu-config/src/scm_config.rs
git diff --name-only
git add crates/rupu-scm/src/account.rs crates/rupu-scm/src/lib.rs crates/rupu-config/src/scm_config.rs
git commit -m "feat(scm): AccountId + account-keyed [scm.*] config with kind and rules

Foundation for SCM multi-account. No behavior change: the Registry
still keys on Platform after this commit."
```

---

### Task 2: The rule engine

A pure function with no I/O and no dependencies, so the whole precedence table is unit-testable. Modelled on `resolve_configured_default` in `crates/rupu-scm/src/registry.rs`, which was extracted for exactly this reason.

**Files:**
- Create: `crates/rupu-scm/src/rules.rs`
- Modify: `crates/rupu-scm/src/lib.rs`

**Interfaces:**
- Consumes: `AccountId` (Task 1), `RepoRef` and `Platform` (existing, `crates/rupu-scm/src/types.rs`).
- Produces:
  - `rupu_scm::rules::Rule { owner: Option<String>, path: Option<String>, account: AccountId }`
  - `rupu_scm::rules::Resolution` — `Explicit(AccountId) | Owner(AccountId) | Path(AccountId) | SoleAccount(AccountId) | NoMatch { candidates: Vec<AccountId> } | NoAccounts`
  - `rupu_scm::rules::glob_matches(pattern: &str, value: &str) -> bool`
  - `rupu_scm::rules::expand_tilde(pattern: &str, home: Option<&Path>) -> String`
  - **Final signature** (Step 3 writes it without `home`; Step 5 adds that parameter — this is the shape Task 3 consumes, so do not stop at Step 3's version):

```rust
pub fn resolve_account(
    rules: &[Rule],
    repo: Option<&RepoRef>,
    cwd: Option<&Path>,
    home: Option<&Path>,
    explicit: Option<&AccountId>,
    candidates: &[AccountId],
) -> Resolution
```

**Two `Rule` types exist, deliberately — but not for the reason an earlier draft gave.** `rupu_config::ScmRule` is the wire/TOML shape (`account: String`); `rupu_scm::rules::Rule` is the domain shape (`account: AccountId`). The real reasons are (a) `AccountId` must not leak into `rupu-config`, since `rupu-config` is the upstream crate and does not depend on `rupu-scm`, and (b) the rule engine stays pure and constructible in tests without building a whole `Config`.

`rupu-scm` **does** depend on `rupu-config` (it always has — `Registry::discover` takes `&Config`), so the conversion belongs in `rupu-scm`, not the CLI: add `rules::Rule::from_config(&[rupu_config::ScmRule]) -> Vec<Rule>` and have `Registry::discover` read `cfg.scm.rules` itself. No CLI-side helper is needed, and no rule list has to be threaded through call sites.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-scm/src/rules.rs`. Write the tests first (the whole precedence table plus the globbing edge cases):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RepoRef;
    use crate::platform::Platform;
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
        let got = resolve_account(&[], Some(&repo("a", "b")), None, None, &[]);
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
        let rules = vec![rule_owner("acme/*", "gh-first"), rule_owner("acme/*", "gh-second")];
        let got = resolve_account(
            &rules,
            Some(&repo("acme", "api")),
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
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-scm rules`
Expected: FAIL — module not declared.

- [ ] **Step 3: Implement the engine**

At the top of `crates/rupu-scm/src/rules.rs`:

```rust
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
                if glob_matches(pat, &dir_s) && known(&rule.account) {
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
```

Declare it in `crates/rupu-scm/src/lib.rs` (hand-write; do not rustfmt the crate root):

```rust
pub mod rules;
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rupu-scm rules`
Expected: PASS (12 tests)

- [ ] **Step 5: Add tilde expansion for path rules**

`~/Code/work/*` is the documented form, so a rule must expand `~` before matching. Add to `rules.rs`:

```rust
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
```

Apply it in the path tier (`glob_matches(&expand_tilde(pat, home), &dir_s)`), threading a `home: Option<&Path>` parameter through `resolve_account`. Add tests: a `~/Code/work/*` rule matches `/home/me/Code/work/api` when home is `/home/me`, and does not match when `home` is `None`.

- [ ] **Step 6: Run, lint, format, commit**

```bash
cargo test -p rupu-scm
cargo clippy -p rupu-scm --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-scm/src/rules.rs
git diff --name-only
git add crates/rupu-scm/src/rules.rs crates/rupu-scm/src/lib.rs
git commit -m "feat(scm): pure account-selection rule engine

Precedence: explicit -> owner rule -> path rule -> sole account -> error.
The sole-account tier is the back-compat clause: a single-account user
needs no rules and never sees the ambiguity error. Owner rules are the
tier daemons rely on, since a webhook payload knows the owner but has
no cwd."
```

---

### Task 3: Reshape `Registry` onto account keys

The structural change. `Platform` stays exactly as it is.

**Files:**
- Modify: `crates/rupu-scm/src/registry.rs`
- Modify: `crates/rupu-scm/src/error.rs` (new error variant)

**Interfaces:**
- Consumes: `AccountId` (Task 1), `rules::{Rule, Resolution, resolve_account}` (Task 2).
- Produces:
  - Internal `struct ScmAccount { kind: Platform, repo, issues, events, extras }`
  - `Registry::accounts_for(&self, kind: Platform) -> Vec<AccountId>` — sorted.
  - `Registry::repo_for(&self, repo: &RepoRef, cwd: Option<&Path>, explicit: Option<&AccountId>) -> Result<(AccountId, Arc<dyn RepoConnector>), AccountError>`
  - `Registry::issues_for(&self, tracker: IssueTracker, project: Option<&str>, explicit: Option<&AccountId>) -> Result<(AccountId, Arc<dyn IssueConnector>), AccountError>`
  - `Registry::repo_by_account(&self, id: &AccountId) -> Option<Arc<dyn RepoConnector>>`
  - `Registry::all_repo_connectors(&self, kind: Platform) -> Vec<(AccountId, Arc<dyn RepoConnector>)>` — the fan-out accessor.
  - `AccountError::NoRuleMatched { repo: String, candidates: Vec<AccountId> }`, `AccountError::NoAccounts { platform: Platform }`, `AccountError::UnknownAccount(AccountId)`.

- [ ] **Step 1: Write the failing tests**

In `crates/rupu-scm/src/registry.rs`'s test module (using the existing `insert_repo_connector` test seam, extended to take an `AccountId`):

```rust
    #[tokio::test]
    async fn single_account_resolves_without_any_rules() {
        let mut reg = Registry::default();
        reg.insert_repo_account(AccountId::new("github"), Platform::Github, fake_repo_connector());
        let r = RepoRef { platform: Platform::Github, owner: "anyone".into(), repo: "thing".into() };
        let (id, _conn) = reg.repo_for(&r, None, None).expect("sole account must resolve");
        assert_eq!(id, AccountId::new("github"));
    }

    #[tokio::test]
    async fn two_accounts_without_a_matching_rule_error_with_candidates() {
        let mut reg = Registry::default();
        reg.insert_repo_account(AccountId::new("gh-work"), Platform::Github, fake_repo_connector());
        reg.insert_repo_account(AccountId::new("gh-personal"), Platform::Github, fake_repo_connector());
        let r = RepoRef { platform: Platform::Github, owner: "other".into(), repo: "thing".into() };
        let err = reg.repo_for(&r, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("other/thing"), "error must name the repo: {msg}");
        assert!(msg.contains("gh-work") && msg.contains("gh-personal"), "must list candidates: {msg}");
    }

    #[tokio::test]
    async fn an_owner_rule_selects_between_two_accounts() {
        let mut reg = Registry::default();
        reg.insert_repo_account(AccountId::new("gh-work"), Platform::Github, fake_repo_connector());
        reg.insert_repo_account(AccountId::new("gh-personal"), Platform::Github, fake_repo_connector());
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-work"),
        }]);
        let r = RepoRef { platform: Platform::Github, owner: "acme".into(), repo: "api".into() };
        let (id, _) = reg.repo_for(&r, None, None).unwrap();
        assert_eq!(id, AccountId::new("gh-work"));
    }

    #[tokio::test]
    async fn explicit_account_overrides_rules() {
        let mut reg = Registry::default();
        reg.insert_repo_account(AccountId::new("gh-work"), Platform::Github, fake_repo_connector());
        reg.insert_repo_account(AccountId::new("gh-personal"), Platform::Github, fake_repo_connector());
        reg.set_rules(vec![Rule {
            owner: Some("acme/*".into()),
            path: None,
            account: AccountId::new("gh-work"),
        }]);
        let r = RepoRef { platform: Platform::Github, owner: "acme".into(), repo: "api".into() };
        let (id, _) = reg.repo_for(&r, None, Some(&AccountId::new("gh-personal"))).unwrap();
        assert_eq!(id, AccountId::new("gh-personal"));
    }

    #[tokio::test]
    async fn fan_out_returns_every_account_of_a_platform_sorted() {
        let mut reg = Registry::default();
        reg.insert_repo_account(AccountId::new("gh-work"), Platform::Github, fake_repo_connector());
        reg.insert_repo_account(AccountId::new("acme-ghe"), Platform::Github, fake_repo_connector());
        reg.insert_repo_account(AccountId::new("gl-work"), Platform::Gitlab, fake_repo_connector());
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
        let r = RepoRef { platform: Platform::Github, owner: "a".into(), repo: "b".into() };
        let msg = reg.repo_for(&r, None, None).unwrap_err().to_string();
        assert!(msg.contains("auth login"), "no-accounts error should point at login: {msg}");
    }
```

You will need a `fake_repo_connector()` helper — the crate already has fake connectors behind the `test-helpers` feature (`insert_repo_connector` is `#[cfg(any(test, feature = "test-helpers"))]`); reuse whatever `crates/rupu-scm/tests/registry_discover.rs` already constructs rather than writing a new fake.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-scm registry`
Expected: FAIL — `insert_repo_account`, `repo_for`, `set_rules`, `all_repo_connectors` do not exist.

- [ ] **Step 3: Reshape the struct**

Replace the four `HashMap<Platform|IssueTracker, …>` fields with one account map:

```rust
struct ScmAccount {
    kind: Platform,
    repo: Option<Arc<dyn RepoConnector>>,
    issues: Option<Arc<dyn IssueConnector>>,
    events: Option<Arc<dyn EventConnector>>,
    github_extras: Option<Arc<GithubExtras>>,
    gitlab_extras: Option<Arc<GitlabExtras>>,
}

#[derive(Default)]
pub struct Registry {
    accounts: BTreeMap<AccountId, ScmAccount>,
    /// Tracker-only accounts (Linear, Jira) have no `Platform`.
    tracker_accounts: BTreeMap<AccountId, (IssueTracker, Arc<dyn IssueConnector>)>,
    tracker_event_connectors: BTreeMap<AccountId, Arc<dyn EventConnector>>,
    rules: Vec<Rule>,
    configured_default_platform: Option<Platform>,
    configured_default_tracker: Option<IssueTracker>,
}
```

`BTreeMap` rather than `HashMap` so listings and fan-out are deterministic — this matters because fan-out output is user-visible in `rupu repos list`.

- [ ] **Step 4: Implement `discover` over accounts**

`discover` currently probes each platform once. It must now iterate `cfg.scm.platforms` (the account map), resolve each account's `kind` (declared `kind`, else the account name if it names a platform — the §3.1 rule), and build connectors per account with that account's own `base_url`/timeout/concurrency options. Credentials come from `resolver.get(account_name, None)` — Arc 1 made the resolver account-aware, so `gh-work` resolves `gh-work/sso`.

Preserve the existing behavior exactly for the zero-config case: when `cfg.scm.platforms` is empty, probe `github` and `gitlab` as bare account names, so a user with only a `github/sso` credential and no `[scm.*]` config gets exactly the registry they get today. Keep the existing INFO/WARN logging per account.

- [ ] **Step 5: Implement the accessors**

`repo_for` narrows candidates with `accounts_for(repo.platform)`, calls `rules::resolve_account`, and maps `Resolution` to either a connector or an `AccountError`. Keep `Resolution::SoleAccount` silent (it is the back-compat path and must not warn on every call).

Error text, per spec §6.4:

```
no account rule matches acme/foo
  configured github accounts: gh-personal, gh-work
  fix: rupu scm bind --owner 'acme/*' --account <name>
  or:  pass --account <name>
```

- [ ] **Step 6: Keep `default_platform` / `default_tracker` working**

Both are consumed by MCP tools that omit the `platform` argument (ISSUES.md I-15). They now answer "is any account of this platform registered?" rather than "is this platform registered". Preserve the WARN on configured-but-unavailable.

- [ ] **Step 7: Run, lint, format, commit**

```bash
cargo test -p rupu-scm
cargo clippy -p rupu-scm --all-targets -- -D warnings
rustfmt --edition 2021 crates/rupu-scm/src/registry.rs crates/rupu-scm/src/error.rs
git diff --name-only
git add -A
git commit -m "feat(scm): key Registry on AccountId, add rule-based resolution

Platform stays the vendor; only the map key changes. repo_for/issues_for
run the rule engine, all_repo_connectors fans out for account-scoped
operations that have no repo to key on."
```

---

### Task 4: Migrate `rupu-cli` call sites (20 sites)

**Files:** `crates/rupu-cli/src/cmd/{issues,repos,run,session,workflow,cron,autoflow,autoflow_wake}.rs`, `crates/rupu-cli/src/{cp_repos,cp_inventory}.rs`

Three shapes, per spec §6.2. Most sites already have a `RepoRef` in hand and simply discard the owner — those are mechanical:

```rust
// before — owner discarded
let conn = registry.repo(repo.platform).ok_or_else(|| ...)?;
// after — owner is already in scope
let (_account, conn) = registry.repo_for(&repo, cwd.as_deref(), args.account.as_ref())?;
```

- [ ] **Step 1:** Add `--account <name>` to `rupu issues` and `rupu repos` subcommands that target a specific repo. Do not add it to `list` subcommands that fan out.
- [ ] **Step 2:** Migrate the targeted sites. For each, the `RepoRef` is already constructed nearby — pass it rather than re-deriving it.
- [ ] **Step 3:** Migrate `cp_repos.rs` and `rupu repos list` to `all_repo_connectors`, and add an `ACCOUNT` column to the output so the union is legible. This is a UX improvement, not just a port: a user with two accounts sees both sets of repos in one table, tagged.
- [ ] **Step 4:** Nothing to wire here — `Registry::discover` already receives `&Config` and reads `cfg.scm.rules` itself via `Rule::from_config` (Task 3). Confirm no call site needs a rule list threaded through it, and say so in your report. If you find one that does, stop and report rather than adding a CLI-side helper.
- [ ] **Step 5:** Add an integration test: two GitHub accounts configured, an owner rule, and `rupu issues list --repo acme/api` reaching the right one. Assert on which account's credential was used, not merely that the command exited 0.
- [ ] **Step 6:** `cargo test -p rupu-cli`, clippy, `rustfmt` each touched leaf file, `git diff --name-only`, commit.

---

### Task 5: Migrate `rupu-mcp` call sites (19 sites) and add the `account` argument

**Files:** `crates/rupu-mcp/src/tools/{issues,scm_prs,scm_repos,scm_branches,scm_files,github_extras,gitlab_extras}.rs`

- [ ] **Step 1:** Add an optional `account: Option<String>` field to every tool args struct that already has `platform`/`tracker`, with a schemars doc string explaining it selects among configured accounts and is only needed when rules do not disambiguate.
- [ ] **Step 2:** Migrate the targeted dispatches to `repo_for` / `issues_for`, passing `account` as the explicit tier. Sites like `scm_repos::dispatch_get` already build a `RepoRef` and then ignore it — pass it.
- [ ] **Step 3:** `scm.repos.list` has no repo to key on. Fan out with `all_repo_connectors` and tag each returned repo with its account in the JSON payload, so an agent can tell them apart. Document the added field.
- [ ] **Step 4:** `resolve_platform`'s "first registered platform" fallback becomes "first registered platform that has any account" — keep the behavior, update the implementation.
- [ ] **Step 5:** Update the MCP tool catalog tests for the new argument and the tagged list payload.
- [ ] **Step 6:** `cargo test -p rupu-mcp`, clippy, `rustfmt`, commit.

---

### Task 6: Daemons — webhook and cron

The tier that owner rules exist for.

**Files:** `crates/rupu-cli/src/cmd/webhook.rs`, `crates/rupu-cli/src/cmd/cron.rs`, `crates/rupu-scm/src/types.rs`

- [ ] **Step 1:** `maybe_build_github_projects_hydrator` builds one hydrator at daemon startup and reuses it for every inbound payload. Replace with a per-account cache resolved per payload: the payload carries the owner, so `resolve_account` works with `cwd: None`. Cache by `AccountId` so a busy daemon does not rebuild clients per event.
- [ ] **Step 2:** `EventSourceRef::Repo` carries a `RepoRef` and resolves normally — migrate `events_for_source`.
- [ ] **Step 3:** `EventSourceRef::TrackerProject { tracker, project }` carries only a project string: no owner, no path. Add `account: Option<AccountId>` to that variant and thread it from the trigger config. This is the one place multi-account cannot be inferred, and it must be explicit rather than guessed. Update the trigger schema and its docs.
- [ ] **Step 4:** Add a test that two webhook payloads for different owners reach different accounts.
- [ ] **Step 5:** `cargo test -p rupu-cli -p rupu-scm`, clippy, `rustfmt`, commit.

---

### Task 7: `rupu scm bind`, docs, and the acceptance walkthrough

**Files:** `crates/rupu-cli/src/cmd/scm.rs` (new), `crates/rupu-cli/src/cli.rs`, `README.md`, `docs/scm.md`, `docs/providers.md`

- [ ] **Step 1:** Add `rupu scm bind --owner <glob> --account <name>` and `--path <glob> --account <name>`, appending a `[[scm.rules]]` entry. **Write via `toml_edit::DocumentMut` + `rupu_cp::config_write::write_atomic`**, exactly as Arc 1's `declare_account_in_config` does — that preserves comments and is atomic. Refuse to write when the existing config does not parse.
- [ ] **Step 2:** Add `rupu scm accounts` listing configured accounts, their kind, base_url, and which rules point at them. A user who hits the ambiguity error needs one command that shows the current state.
- [ ] **Step 3:** Validate rules in `Config::validate`: a rule with both `owner` and `path`, or neither, is an error naming the offending entry. A rule naming an account with no `[scm.<name>]` table is a warning, not an error (the account may be credential-only).
- [ ] **Step 4:** Document the whole surface in `docs/scm.md` — accounts, kinds, rules, precedence, the ambiguity error and how to fix it, and the `--account` flag. Update `README.md`'s SCM section. Cross-link from `docs/providers.md`'s accounts section, which Arc 1 added.
- [ ] **Step 5:** Run the spec §7 Arc 2 acceptance walkthrough end to end against a scratch `RUPU_HOME` and paste the **real** output into the report:

```bash
rupu auth login --account gh-work     --kind github --mode sso
rupu auth login --account gh-personal --kind github --mode sso
rupu scm bind --owner 'acme/*'        --account gh-work
rupu scm bind --owner 'MrBrutti/*'    --account gh-personal
rupu repos list                       # both accounts, tagged
rupu issues list --repo acme/api      # -> gh-work
```

- [ ] **Step 6:** Full workspace test, clippy, `rustfmt` each touched leaf file, commit.

---

## Acceptance

Arc 2 is done when the spec §7 Arc 2 block reproduces verbatim, and when:

- A user with exactly one GitHub account and no `[scm.rules]` sees **zero** behavior change and writes no config.
- Two accounts plus owner rules route `rupu issues list --repo acme/api` and `--repo MrBrutti/dots` to different identities.
- `rupu repos list` shows both accounts' repos in one table, tagged by account.
- A targeted command against an unmatched repo errors with the candidates and the fix, and never silently picks one.
- A webhook payload resolves its account from the owner, with no cwd.
- github.com and a GitHub Enterprise host coexist as two accounts with different `base_url`s.

## Deliberately out of scope

- Per-account permission or policy models. Accounts are identities only.
- The `autoflow` entrypoint resolver (`crates/rupu-cli/src/cmd/autoflow.rs`) — it builds one resolver before any repo is known, and autoflow is multi-repo. Arc 2 may make this tractable by giving `Registry` account-awareness, but changing it is a separate decision, not a side effect of this plan.
- `AnthropicClient`'s internal semaphore keyed on the literal `"anthropic"` (an Arc 1 deferral in a different crate).
