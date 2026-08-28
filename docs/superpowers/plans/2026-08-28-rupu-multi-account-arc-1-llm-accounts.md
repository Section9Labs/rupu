# Multi-Account Providers — Arc 1 (LLM Named Accounts) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user hold several credentials for the same LLM vendor (two Anthropic accounts, two OpenAI accounts) by splitting the *account name* (identity) from the *vendor kind* (which client to build).

**Architecture:** The provider name currently means both "who" and "what vendor". We split them: the account name stays the map key everywhere (config table name, credential key prefix), and a `kind` field — which already exists on `ProviderConfig` but only accepts `"openai-compatible"` — becomes the vendor selector. Credential keys stay `<account>/<mode>`, so `anthropic/api-key` keeps working as the account that happens to be *named* `anthropic`. Zero migration.

**Tech Stack:** Rust 2021, tokio, thiserror (libs) / anyhow (CLI), clap, toml + toml_edit, tracing.

**Spec:** `docs/superpowers/specs/2026-08-28-rupu-multi-account-providers-design.md`

## Global Constraints

- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- Workspace deps only — versions pinned in root `Cargo.toml`, never in crate `Cargo.toml` files.
- Hexagonal rule 1: `rupu-auth` MUST NOT depend on `rupu-config`. Account data crosses that boundary as `AccountSpec`, a type owned by `rupu-auth`.
- `rupu-cli` is thin: arg parsing + delegation only.
- Errors: `thiserror` in libraries, `anyhow` in `rupu-cli`.
- **Never run package-wide `cargo fmt`** — `main` is fmt-dirty under the pinned toolchain. Format only the files you touched: `cargo fmt -- <file>`.
- **`crates/rupu-auth/src/account_key.rs` must not change.** Its test `key_format_is_stable_across_providers` must pass unchanged. Changing the key format strands every user's credentials.
- Back-compat rule (spec §3.1), load-bearing in every task: **`kind` defaults to the account name when that name is a known vendor.** A user with `anthropic/api-key` and no `[providers.*]` config must behave exactly as before.
- Never `git stash pop` — the stash is shared across sessions and popping it can clobber another session's work.
- Do not commit to `main`. All work lands on the current worktree branch.

## Vendor kind strings (used verbatim in several tasks)

```
anthropic, openai, openai_codex, codex, gemini, google_gemini,
copilot, github_copilot, local
```

Plus `openai-compatible`, which is not a built-in vendor and keeps its existing separate handling.

---

### Task 1: `ProviderId::from_vendor_str` + `AccountSpec`

Foundation types. No behavior change anywhere yet — this task only adds new, unused API.

**Files:**
- Modify: `crates/rupu-auth/src/backend.rs` (add an impl method + tests)
- Create: `crates/rupu-auth/src/account.rs`
- Modify: `crates/rupu-auth/src/lib.rs` (export the new module)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `ProviderId::from_vendor_str(s: &str) -> Option<ProviderId>`
  - `rupu_auth::account::AccountSpec { pub name: String, pub kind: String }`
  - `AccountSpec::new(name: impl Into<String>, kind: impl Into<String>) -> AccountSpec`
  - `AccountSpec::provider_id(&self) -> Option<ProviderId>`
  - `rupu_auth::account::resolve_provider_id(name: &str, accounts: &[AccountSpec]) -> Option<ProviderId>`

- [ ] **Step 1: Write the failing test for `from_vendor_str`**

Append to the existing `mod tests` at the bottom of `crates/rupu-auth/src/backend.rs`:

```rust
    #[test]
    fn from_vendor_str_accepts_canonical_names_and_aliases() {
        assert_eq!(
            ProviderId::from_vendor_str("anthropic"),
            Some(ProviderId::Anthropic)
        );
        assert_eq!(
            ProviderId::from_vendor_str("codex"),
            Some(ProviderId::Openai)
        );
        assert_eq!(
            ProviderId::from_vendor_str("google_gemini"),
            Some(ProviderId::Gemini)
        );
        assert_eq!(
            ProviderId::from_vendor_str("github_copilot"),
            Some(ProviderId::Copilot)
        );
    }

    /// An account name is NOT a vendor name. This is the distinction the
    /// whole multi-account model rests on.
    #[test]
    fn from_vendor_str_rejects_account_names() {
        assert_eq!(ProviderId::from_vendor_str("anthropic-work"), None);
        assert_eq!(ProviderId::from_vendor_str("openai-compatible"), None);
        assert_eq!(ProviderId::from_vendor_str(""), None);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-auth from_vendor_str`
Expected: FAIL — `no function or associated item named 'from_vendor_str' found`

- [ ] **Step 3: Implement `from_vendor_str`**

Add inside the existing `impl ProviderId` block in `crates/rupu-auth/src/backend.rs`, directly after `as_str`:

```rust
    /// Parse a **vendor** name into a `ProviderId`.
    ///
    /// Accepts the canonical name plus every alias
    /// `rupu_runtime::provider_factory` recognizes, so a config
    /// `kind = "codex"` and a bare provider name `codex` resolve
    /// identically.
    ///
    /// This is deliberately NOT `FromStr`: an account name (e.g.
    /// `anthropic-work`) is a valid provider string but not a vendor,
    /// and conflating the two is the bug this whole change removes.
    pub fn from_vendor_str(s: &str) -> Option<Self> {
        match s {
            "anthropic" => Some(Self::Anthropic),
            "openai" | "openai_codex" | "codex" => Some(Self::Openai),
            "gemini" | "google_gemini" => Some(Self::Gemini),
            "copilot" | "github_copilot" => Some(Self::Copilot),
            "local" => Some(Self::Local),
            "github" => Some(Self::Github),
            "gitlab" => Some(Self::Gitlab),
            "linear" => Some(Self::Linear),
            "jira" => Some(Self::Jira),
            _ => None,
        }
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p rupu-auth from_vendor_str`
Expected: PASS (2 tests)

- [ ] **Step 5: Write the failing tests for `resolve_provider_id`**

Create `crates/rupu-auth/src/account.rs`:

```rust
//! Declared account identities.
//!
//! An **account** is an identity (whose token this is). A **kind** is a
//! vendor (which client to build). Before multi-account support these
//! were the same string, which is why only one credential per vendor
//! could exist.
//!
//! This module owns `AccountSpec` rather than `rupu-config` because
//! `rupu-auth` must not depend on `rupu-config` (hexagonal rule 1). The
//! CLI reads config and hands the list down.

use crate::backend::ProviderId;

/// One declared account: its name, and the vendor it authenticates against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSpec {
    pub name: String,
    pub kind: String,
}

impl AccountSpec {
    pub fn new(name: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
        }
    }

    /// The vendor this account authenticates against, if `kind` names a
    /// built-in. `None` for `openai-compatible` and for unknown kinds.
    pub fn provider_id(&self) -> Option<ProviderId> {
        ProviderId::from_vendor_str(&self.kind)
    }
}

/// Resolve a provider string to its vendor.
///
/// A declared account wins. Otherwise the name is tried as a vendor name
/// directly — the back-compat rule from the design spec §3.1, which is
/// what keeps a pre-existing `anthropic/api-key` working with no config
/// at all.
pub fn resolve_provider_id(name: &str, accounts: &[AccountSpec]) -> Option<ProviderId> {
    if let Some(a) = accounts.iter().find(|a| a.name == name) {
        return a.provider_id();
    }
    ProviderId::from_vendor_str(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_account_resolves_to_its_kind() {
        let accounts = vec![
            AccountSpec::new("anthropic-work", "anthropic"),
            AccountSpec::new("anthropic-personal", "anthropic"),
        ];
        assert_eq!(
            resolve_provider_id("anthropic-work", &accounts),
            Some(ProviderId::Anthropic)
        );
        assert_eq!(
            resolve_provider_id("anthropic-personal", &accounts),
            Some(ProviderId::Anthropic)
        );
    }

    /// Spec §3.1. A user who never declared anything must keep working.
    #[test]
    fn bare_vendor_name_resolves_with_no_declared_accounts() {
        assert_eq!(resolve_provider_id("anthropic", &[]), Some(ProviderId::Anthropic));
        assert_eq!(resolve_provider_id("github", &[]), Some(ProviderId::Github));
    }

    #[test]
    fn undeclared_non_vendor_name_does_not_resolve() {
        assert_eq!(resolve_provider_id("anthropic-work", &[]), None);
    }

    /// A declared account may shadow a bare vendor name, and its own
    /// `kind` is what counts.
    #[test]
    fn declared_account_named_after_a_vendor_uses_its_kind() {
        let accounts = vec![AccountSpec::new("anthropic", "anthropic")];
        assert_eq!(
            resolve_provider_id("anthropic", &accounts),
            Some(ProviderId::Anthropic)
        );
    }

    #[test]
    fn openai_compatible_kind_has_no_provider_id() {
        let a = AccountSpec::new("oracle", "openai-compatible");
        assert_eq!(a.provider_id(), None);
    }
}
```

- [ ] **Step 6: Run the tests to verify they fail**

Run: `cargo test -p rupu-auth account::`
Expected: FAIL — the module is not declared in `lib.rs`, so it does not compile.

- [ ] **Step 7: Export the module**

In `crates/rupu-auth/src/lib.rs`, add next to the other `pub mod` lines (keep alphabetical-ish grouping with `account_key`):

```rust
pub mod account;
pub use account::AccountSpec;
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p rupu-auth`
Expected: PASS. In particular `key_format_is_stable_across_providers` must still pass — it is untouched.

- [ ] **Step 9: Lint and commit**

```bash
cargo clippy -p rupu-auth --all-targets -- -D warnings
cargo fmt -- crates/rupu-auth/src/account.rs crates/rupu-auth/src/backend.rs crates/rupu-auth/src/lib.rs
git add crates/rupu-auth/src/account.rs crates/rupu-auth/src/backend.rs crates/rupu-auth/src/lib.rs
git commit -m "feat(auth): AccountSpec + ProviderId::from_vendor_str

Splits account identity from vendor kind. No behavior change yet -
these types are not wired into any resolution path in this commit."
```

---

### Task 2: Resolver resolves named accounts, including SSO

Makes `KeychainResolver` able to serve a declared account: read `<account>/<mode>` for **both** modes (today named accounts are api-key only), and refresh SSO against the account's *kind*.

**Files:**
- Modify: `crates/rupu-auth/src/resolver.rs`

**Interfaces:**
- Consumes: `AccountSpec`, `resolve_provider_id` (Task 1).
- Produces:
  - `KeychainResolver::with_accounts(self, accounts: Vec<AccountSpec>) -> Self` (builder, consumes and returns self)
  - `KeychainResolver::peek_sso_named(&self, name: &str) -> Option<String>`
  - Behavior: `CredentialResolver::get(account_name, hint)` resolves declared accounts with SSO-then-api-key precedence.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` at the bottom of `crates/rupu-auth/src/resolver.rs`:

```rust
    /// Two accounts of the same vendor store and read back independently.
    /// This is the core capability the whole arc exists to deliver.
    #[tokio::test]
    async fn two_accounts_of_same_kind_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: vec![
                crate::account::AccountSpec::new("anthropic-work", "anthropic"),
                crate::account::AccountSpec::new("anthropic-personal", "anthropic"),
            ],
        };

        resolver
            .store_named(
                "anthropic-work",
                AuthMode::ApiKey,
                &StoredCredential::api_key("work-key"),
            )
            .await
            .unwrap();
        resolver
            .store_named(
                "anthropic-personal",
                AuthMode::ApiKey,
                &StoredCredential::api_key("personal-key"),
            )
            .await
            .unwrap();

        let (mode, creds) = resolver.get("anthropic-work", None).await.unwrap();
        assert_eq!(mode, AuthMode::ApiKey);
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "work-key"));

        let (_, creds) = resolver.get("anthropic-personal", None).await.unwrap();
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "personal-key"));
    }

    /// Named accounts must support SSO, not just api-key. Before this
    /// task `get_named` only ever tried api-key.
    #[tokio::test]
    async fn named_account_resolves_sso_and_prefers_it_over_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: vec![crate::account::AccountSpec::new(
                "anthropic-work",
                "anthropic",
            )],
        };

        resolver
            .store_named(
                "anthropic-work",
                AuthMode::ApiKey,
                &StoredCredential::api_key("the-key"),
            )
            .await
            .unwrap();

        let sso = StoredCredential {
            credentials: AuthCredentials::OAuth {
                access: "the-token".into(),
                refresh: "the-refresh".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("the-refresh".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
        };
        resolver
            .store_named("anthropic-work", AuthMode::Sso, &sso)
            .await
            .unwrap();

        let (mode, creds) = resolver.get("anthropic-work", None).await.unwrap();
        assert_eq!(mode, AuthMode::Sso, "SSO must win over api-key");
        assert!(matches!(creds, AuthCredentials::OAuth { access, .. } if access == "the-token"));
    }

    /// Spec §3.1 regression guard: a user with only the legacy bare key
    /// and NO declared accounts must resolve exactly as before.
    #[tokio::test]
    async fn bare_builtin_still_resolves_with_no_declared_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: Vec::new(),
        };
        resolver
            .store(
                ProviderId::Anthropic,
                AuthMode::ApiKey,
                &StoredCredential::api_key("legacy"),
            )
            .await
            .unwrap();

        let (mode, creds) = resolver.get("anthropic", None).await.unwrap();
        assert_eq!(mode, AuthMode::ApiKey);
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "legacy"));
    }

    /// An undeclared, non-vendor name is a typo, not an account.
    #[tokio::test]
    async fn undeclared_account_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = KeychainResolver {
            path: dir.path().join("auth.json"),
            accounts: Vec::new(),
        };
        let err = resolver.get("anthropic-typo", None).await.unwrap_err();
        assert!(
            err.to_string().contains("anthropic-typo"),
            "error should name the offending string, got: {err}"
        );
    }
```

`StoredCredential` (`crates/rupu-auth/src/stored.rs`) is `{ credentials, refresh_token: Option<String>, expires_at: Option<DateTime<Utc>> }`, and `AuthCredentials::OAuth` (`crates/rupu-providers/src/auth/mod.rs`) is `{ access: String, refresh: String, expires: u64, extra: HashMap<String, serde_json::Value> }` — both literals above match those exactly. `tempfile` and `serial_test` are already dev-dependencies of `rupu-auth`, and `#[tokio::test]` is already used in this file, so no `Cargo.toml` changes are needed.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-auth --lib resolver`
Expected: FAIL — `KeychainResolver` has no field `accounts`.

- [ ] **Step 3: Add the `accounts` field and builder**

In `crates/rupu-auth/src/resolver.rs`, change the struct and its constructors:

```rust
pub struct KeychainResolver {
    /// Where credentials live: a chmod-600 JSON file. There is no
    /// second backend — see the type docs.
    path: PathBuf,
    /// Accounts declared in config. Empty means "built-in vendor names
    /// only", which is exactly the pre-multi-account behavior.
    accounts: Vec<crate::account::AccountSpec>,
}
```

In `with_service`, change the constructor tail to:

```rust
        Self {
            path,
            accounts: Vec::new(),
        }
```

Add after `with_service`:

```rust
    /// Declare the config's accounts so `get` / `refresh` can resolve a
    /// named account to its vendor kind.
    ///
    /// `rupu-auth` cannot read config itself (hexagonal rule 1), so the
    /// CLI resolves the list and passes it here. Leaving this unset
    /// keeps the pre-multi-account behavior exactly.
    pub fn with_accounts(mut self, accounts: Vec<crate::account::AccountSpec>) -> Self {
        self.accounts = accounts;
        self
    }
```

- [ ] **Step 4: Extract the env-var tier so both paths share it**

Still in `resolver.rs`, add this private helper next to `get_named`:

```rust
    /// `RUPU_<UPPER_ACCOUNT>_API_KEY`. Non-alphanumeric characters in an
    /// account name map to `_` so `anthropic-work` reads
    /// `RUPU_ANTHROPIC_WORK_API_KEY`.
    fn env_api_key(account: &str) -> Option<AuthCredentials> {
        let upper: String = account
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        let key = std::env::var(format!("RUPU_{upper}_API_KEY")).ok()?;
        if key.is_empty() {
            return None;
        }
        Some(AuthCredentials::ApiKey { key })
    }
```

Then rewrite `get_named` to use it (this also fixes the pre-existing bug that `anthropic-work` would have looked for `RUPU_ANTHROPIC-WORK_API_KEY`, an illegal env var name):

```rust
    /// Resolve a config-declared provider name to an api-key credential:
    /// `auth.json["<name>/api-key"]` (or legacy `["<name>"]`), then
    /// `RUPU_<UPPER_NAME>_API_KEY`.
    async fn get_named(&self, provider: &str) -> Result<(AuthMode, AuthCredentials)> {
        if let Some(sc) = self.read_account(provider, Some(provider), AuthMode::ApiKey)? {
            return Ok((AuthMode::ApiKey, sc.credentials));
        }
        if let Some(creds) = Self::env_api_key(provider) {
            return Ok((AuthMode::ApiKey, creds));
        }
        anyhow::bail!(
            "no credentials for '{provider}'. Run: rupu auth login --account {provider} \
             --mode api-key, or set the matching RUPU_*_API_KEY env var"
        )
    }
```

- [ ] **Step 5: Route SSO refresh by kind**

Replace `refresh_inner`'s signature so the vendor comes in explicitly rather than being derived from the name. Change:

```rust
    async fn refresh_inner(
        &self,
        p: ProviderId,
        _mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<StoredCredential> {
```

to:

```rust
    /// Refresh against a **vendor**, not an account name. Two accounts of
    /// the same kind refresh against the same OAuth config but store
    /// under their own keys — that is what makes multi-account SSO work.
    async fn refresh_inner(
        &self,
        kind: ProviderId,
        sc: &StoredCredential,
    ) -> Result<StoredCredential> {
```

Inside the body, rename every use of `p` to `kind`. The existing `provider_oauth(p)` call becomes `provider_oauth(kind)`. Update the two existing call sites in `get` and `refresh` to drop the now-removed `mode` argument.

- [ ] **Step 6: Rewrite `get` to handle declared accounts**

Replace the whole `async fn get` body in `impl CredentialResolver for KeychainResolver` with:

```rust
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)> {
        let modes: Vec<AuthMode> = match hint {
            Some(m) => vec![m],
            None => vec![AuthMode::Sso, AuthMode::ApiKey],
        };

        // A declared account, or a bare vendor name. Both read
        // `<name>/<mode>`; `account_for(pid, mode)` and
        // `format!("{name}/{mode}")` produce byte-identical keys, so the
        // legacy bare-vendor path is a special case of this one — it just
        // additionally tolerates the Slice-A legacy key.
        if let Some(kind) = crate::account::resolve_provider_id(provider, &self.accounts) {
            let legacy = if Self::parse_provider(provider).is_ok() {
                Some(provider)
            } else {
                None
            };
            for mode in modes {
                if let Some(mut sc) = self.read_account(provider, legacy, mode)? {
                    let now = chrono::Utc::now();
                    if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                        let new = self.refresh_inner(kind, &sc).await?;
                        self.store_named(provider, mode, &new).await?;
                        sc = new;
                    }
                    return Ok((mode, sc.credentials));
                }
            }
            if let Some(creds) = Self::env_api_key(provider) {
                return Ok((AuthMode::ApiKey, creds));
            }
            anyhow::bail!(
                "no credentials configured for {provider}. \
                 Run: rupu auth login --account {provider} --mode <api-key|sso>"
            )
        }

        // Not a vendor and not declared: an openai-compatible entry, or a
        // typo. `get_named` produces the actionable error either way.
        self.get_named(provider).await
    }
```

Note: `store_named` is used for the refresh write-back rather than `store(pid, ..)` because the account name is the key. For a bare vendor name these are the same string — verified by `account_key.rs`'s own tests.

- [ ] **Step 7: Route the `refresh` trait method by kind**

Replace the `refresh` body:

```rust
    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let kind = crate::account::resolve_provider_id(provider, &self.accounts)
            .ok_or_else(|| anyhow::anyhow!("unknown provider or account: {provider}"))?;
        let legacy = if Self::parse_provider(provider).is_ok() {
            Some(provider)
        } else {
            None
        };
        let sc = self
            .read_account(provider, legacy, mode)?
            .ok_or_else(|| anyhow::anyhow!("no stored credential for {provider}/{mode:?}"))?;
        let new = self.refresh_inner(kind, &sc).await?;
        self.store_named(provider, mode, &new).await?;
        Ok(new.credentials)
    }
```

- [ ] **Step 8: Add `peek_sso_named`**

Next to the existing `peek_sso`, add:

```rust
    /// `peek_sso` for an account name rather than a built-in vendor.
    /// Same human-readable expiry strings.
    pub async fn peek_sso_named(&self, name: &str) -> Option<String> {
        let sc = self.read_account(name, Some(name), AuthMode::Sso).ok()??;
        let Some(exp) = sc.expires_at else {
            return Some("no expiry".into());
        };
        let now = chrono::Utc::now();
        let dur = exp.signed_duration_since(now);
        if dur.num_seconds() <= 0 {
            Some("expired — re-login".into())
        } else if dur.num_days() >= 1 {
            Some(format!("expires in {}d", dur.num_days()))
        } else {
            Some(format!("expires in {}h", dur.num_hours().max(1)))
        }
    }
```

- [ ] **Step 9: Run the tests**

Run: `cargo test -p rupu-auth`
Expected: PASS, including all four new tests and the untouched `key_format_is_stable_across_providers`.

- [ ] **Step 10: Verify nothing else broke**

Run: `cargo test --workspace`
Expected: PASS. `KeychainResolver::new()` is unchanged in behavior (empty account list), so every existing call site is unaffected.

- [ ] **Step 11: Lint and commit**

```bash
cargo clippy -p rupu-auth --all-targets -- -D warnings
cargo fmt -- crates/rupu-auth/src/resolver.rs
git add crates/rupu-auth/src/resolver.rs
git commit -m "feat(auth): resolve declared accounts, with SSO and kind-routed refresh

get() now serves a declared account under <account>/<mode> with the same
SSO-then-api-key precedence built-ins get, and refreshes against the
account's kind rather than its name. Named accounts previously supported
api-key only.

Bare vendor names are unchanged: account_for(pid, mode) and
<name>/<mode> are byte-identical keys."
```

---

### Task 3: Config accepts built-in `kind` values

**Files:**
- Modify: `crates/rupu-config/src/config.rs` (the `validate` method + tests)

**Interfaces:**
- Consumes: nothing from earlier tasks (independent of Tasks 1–2; may be done in parallel).
- Produces: `[providers.<name>] kind = "<builtin vendor>"` parses and validates.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `crates/rupu-config/src/config.rs`:

```rust
    #[test]
    fn validate_accepts_builtin_kind() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic-work".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }

    /// A built-in kind needs no base_url / default_model — those are
    /// openai-compatible requirements only.
    #[test]
    fn validate_builtin_kind_does_not_require_base_url() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "openai-personal".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unknown_kind() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "weird".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("not-a-vendor".into()),
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("not-a-vendor"), "got: {err}");
        assert!(err.contains("openai-compatible"), "should list valid kinds, got: {err}");
    }

    /// The reserved-name rule stays scoped to openai-compatible: you may
    /// not name a custom endpoint "anthropic", but an account named
    /// "anthropic" with kind "anthropic" is the legacy default and fine.
    #[test]
    fn validate_still_rejects_reserved_name_for_openai_compatible() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                default_model: Some("m".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_account_named_after_its_own_vendor() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-config validate_`
Expected: `validate_rejects_unknown_kind` FAILS (unknown kinds are currently accepted silently). The others may already pass — that is fine and expected; they are regression guards.

- [ ] **Step 3: Add the kind list and rewrite the validation loop**

In `crates/rupu-config/src/config.rs`, add next to `RESERVED_PROVIDER_NAMES`:

```rust
/// Vendor kinds a `[providers.<name>]` entry may declare, in addition to
/// `"openai-compatible"`. Kept in lockstep with
/// `rupu_runtime::provider_factory::is_builtin_provider` and
/// `rupu_auth::backend::ProviderId::from_vendor_str`.
const BUILTIN_PROVIDER_KINDS: &[&str] = &[
    "anthropic",
    "openai",
    "openai_codex",
    "codex",
    "gemini",
    "google_gemini",
    "copilot",
    "github_copilot",
    "local",
];
```

Replace the `for (name, p) in &self.providers { ... }` body inside `validate` with:

```rust
        for (name, p) in &self.providers {
            match p.kind.as_deref() {
                // No kind: the account name itself is the vendor
                // (design spec §3.1). Nothing to validate here.
                None => {}
                Some("openai-compatible") => {
                    if RESERVED_PROVIDER_NAMES.contains(&name.as_str()) {
                        return Err(crate::layer::LayerError::Invalid(format!(
                            "provider '{name}': \"openai-compatible\" cannot reuse the reserved \
                             built-in provider name '{name}'; choose a distinct name"
                        )));
                    }
                    if p.base_url.is_none() {
                        return Err(crate::layer::LayerError::Invalid(format!(
                            "provider '{name}': kind=\"openai-compatible\" requires base_url"
                        )));
                    }
                    if p.default_model.as_deref().is_none_or(|m| m.is_empty()) {
                        return Err(crate::layer::LayerError::Invalid(format!(
                            "provider '{name}': kind=\"openai-compatible\" requires default_model"
                        )));
                    }
                }
                Some(k) if BUILTIN_PROVIDER_KINDS.contains(&k) => {}
                Some(k) => {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "provider '{name}': unknown kind \"{k}\"; expected \
                         \"openai-compatible\" or one of: {}",
                        BUILTIN_PROVIDER_KINDS.join(", ")
                    )));
                }
            }
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-config`
Expected: PASS, including the pre-existing `validate_rejects_openai_compatible_without_base_url` and `validate_accepts_openai_compatible_with_base_url`.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy -p rupu-config --all-targets -- -D warnings
cargo fmt -- crates/rupu-config/src/config.rs
git add crates/rupu-config/src/config.rs
git commit -m "feat(config): accept built-in vendor names as provider kind

kind was openai-compatible or nothing; it now names any built-in vendor,
which is what lets two accounts share a vendor. Unknown kinds are now a
hard error instead of being silently ignored. The reserved-name rule
stays scoped to openai-compatible."
```

---

### Task 4: Provider factory dispatches on kind

**Files:**
- Modify: `crates/rupu-runtime/src/provider_factory.rs`

**Interfaces:**
- Consumes: `rupu_config::ProviderConfig.kind` accepting built-ins (Task 3).
- Produces:
  - `provider_factory::resolve_kind(name: &str, providers: &BTreeMap<String, rupu_config::ProviderConfig>) -> Option<String>`
  - `provider_factory::ProviderConfig` gains `pub kind: Option<String>`.

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `crates/rupu-runtime/src/provider_factory.rs` (create the module if absent):

```rust
    use std::collections::BTreeMap;

    fn providers_with(name: &str, kind: &str) -> BTreeMap<String, rupu_config::ProviderConfig> {
        let mut m = BTreeMap::new();
        m.insert(
            name.to_string(),
            rupu_config::ProviderConfig {
                kind: Some(kind.to_string()),
                ..Default::default()
            },
        );
        m
    }

    #[test]
    fn resolve_kind_prefers_declared_kind() {
        let p = providers_with("anthropic-work", "anthropic");
        assert_eq!(
            resolve_kind("anthropic-work", &p).as_deref(),
            Some("anthropic")
        );
    }

    /// Spec §3.1: with no config at all, a built-in name is its own kind.
    #[test]
    fn resolve_kind_falls_back_to_the_name_for_builtins() {
        let empty = BTreeMap::new();
        assert_eq!(resolve_kind("anthropic", &empty).as_deref(), Some("anthropic"));
        assert_eq!(resolve_kind("codex", &empty).as_deref(), Some("codex"));
    }

    #[test]
    fn resolve_kind_is_none_for_undeclared_non_builtin() {
        let empty = BTreeMap::new();
        assert_eq!(resolve_kind("anthropic-work", &empty), None);
    }

    #[test]
    fn resolve_kind_reports_openai_compatible() {
        let p = providers_with("oracle", "openai-compatible");
        assert_eq!(
            resolve_kind("oracle", &p).as_deref(),
            Some("openai-compatible")
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-runtime resolve_kind`
Expected: FAIL — `cannot find function 'resolve_kind'`

- [ ] **Step 3: Implement `resolve_kind`**

Add to `crates/rupu-runtime/src/provider_factory.rs`, next to `openai_compatible_params`:

```rust
/// Resolve an account name to its vendor kind string.
///
/// `[providers.<name>].kind` wins; otherwise the name itself, when it is
/// a built-in vendor name (design spec §3.1 — this is what keeps a
/// config-less `anthropic` working). `None` means the name is neither
/// declared nor a vendor.
pub fn resolve_kind(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> Option<String> {
    if let Some(k) = providers.get(name).and_then(|p| p.kind.clone()) {
        return Some(k);
    }
    if is_builtin_provider(name) {
        return Some(name.to_string());
    }
    None
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-runtime resolve_kind`
Expected: PASS (4 tests)

- [ ] **Step 5: Add `kind` to the factory's per-build config**

In the same file, add a field to the factory's own `ProviderConfig` struct:

```rust
    /// Resolved vendor kind for this account, from
    /// [`resolve_kind`]. `None` means "dispatch on the provider name",
    /// which is the pre-multi-account behavior and is what every
    /// existing `Default::default()` call site gets.
    pub kind: Option<String>,
```

- [ ] **Step 6: Dispatch on kind**

In `build_for_provider_with_config`, replace `let client = match name {` with:

```rust
    // The account name identifies *who*; the kind identifies *what
    // vendor*. Falling back to the name preserves the single-account
    // behavior exactly.
    let kind = config.kind.as_deref().unwrap_or(name);
    let client = match kind {
```

Leave every match arm's body unchanged. The `_ =>` arm (openai-compatible) also stays as-is.

Also change the tuning fallback on the line above so vendor defaults key off the vendor rather than the account name:

```rust
    let tuning = config
        .tuning
        .clone()
        .unwrap_or_else(|| rupu_providers::ProviderTuning::for_provider(kind));
```

Move the `let kind = ...` binding above the `let tuning = ...` binding so it is in scope.

- [ ] **Step 7: Populate `kind` where `openai_compatible` is already populated**

Find every call site that builds a `provider_factory::ProviderConfig` with `openai_compatible: Some(...)` (search: `rg 'openai_compatible:' crates/`). At each one, also set `kind: resolve_kind(&provider_name, &cfg.providers)`. Do not change call sites that use `Default::default()` — they intentionally keep name-based dispatch.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 9: Lint and commit**

```bash
cargo clippy -p rupu-runtime --all-targets -- -D warnings
cargo fmt -- crates/rupu-runtime/src/provider_factory.rs
git add -A
git commit -m "feat(runtime): build providers by vendor kind, not account name

The factory matched on the provider name to pick a client, which is why
a second Anthropic account could not exist. It now matches on the
resolved kind, falling back to the name so single-account setups are
byte-identical."
```

---

### Task 5: `rupu auth login --account/--kind`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`

**Interfaces:**
- Consumes: `resolve_kind` (Task 4), `ProviderId::from_vendor_str` (Task 1), `store_named` (existing).
- Produces: `rupu auth login --account <name> --kind <vendor> --mode <api-key|sso>` storing under `<account>/<mode>` and declaring the account in global config.

- [ ] **Step 1: Add the flags**

In the `Login` variant of the `Action` enum in `crates/rupu-cli/src/cmd/auth.rs`:

```rust
    Login {
        /// Account name — the identity. Use a distinct name per identity
        /// (e.g. `anthropic-work`, `anthropic-personal`). A bare vendor
        /// name (`anthropic`) is the single-account default.
        #[arg(long, alias = "provider")]
        account: String,
        /// Vendor this account authenticates against (anthropic | openai |
        /// gemini | copilot | github | gitlab | linear | jira | local).
        /// Required the first time an account name is used; inferred from
        /// config or from the account name afterwards.
        #[arg(long)]
        kind: Option<String>,
        /// Authentication mode.
        #[arg(long, value_enum, default_value = "api-key")]
        mode: AuthModeArg,
        /// API key (only valid with --mode api-key). If omitted, reads from stdin.
        #[arg(long)]
        key: Option<String>,
    },
```

Update the `Action::Login { .. }` destructuring in `handle` to pass `account` and `kind` through to `login`.

- [ ] **Step 2: Write the failing test for the config-declaration helper**

Append to `mod tests` in `crates/rupu-cli/src/cmd/auth.rs`:

```rust
    #[test]
    fn declare_account_writes_kind_without_dotted_key_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_provider = \"anthropic\"\n").unwrap();

        declare_account_in_config(&path, "anthropic-work", "anthropic").unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            v["providers"]["anthropic-work"]["kind"].as_str(),
            Some("anthropic")
        );
        // Pre-existing keys survive.
        assert_eq!(v["default_provider"].as_str(), Some("anthropic"));
    }

    /// An account name containing a dot must land as ONE table key, not
    /// two nested tables. This is why we insert into the table tree
    /// directly instead of building a dotted key string.
    #[test]
    fn declare_account_treats_a_dotted_name_as_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        declare_account_in_config(&path, "gpt-4.1-box", "openai").unwrap();
        let v: toml::Value =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v["providers"]["gpt-4.1-box"]["kind"].as_str(),
            Some("openai")
        );
    }

    #[test]
    fn declare_account_refuses_to_clobber_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();
        assert!(declare_account_in_config(&path, "x", "openai").is_err());
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p rupu-cli declare_account`
Expected: FAIL — `cannot find function 'declare_account_in_config'`

- [ ] **Step 4: Implement the config declaration helper**

Add to `crates/rupu-cli/src/cmd/auth.rs`:

```rust
/// Write `[providers.<account>] kind = "<kind>"` into a config file,
/// preserving everything already there.
///
/// Inserts into the parsed table tree directly rather than building a
/// dotted key. A dotted key would have to be quoted to survive an
/// account name containing a `.`, and that quoting contract has four
/// lockstep implementations across this repo — sidestepping it here
/// keeps this off that list.
fn declare_account_in_config(path: &Path, account: &str, kind: &str) -> anyhow::Result<()> {
    use toml::Value;

    let mut root: Value = if path.exists() {
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "refusing to write: {} is not valid TOML ({e}). \
                 Fix or move the file first — writing would discard its contents.",
                path.display()
            )
        })?
    } else {
        Value::Table(Default::default())
    };

    let root_table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config root is not a table"))?;

    let providers = root_table
        .entry("providers".to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`providers` is already a value, not a table"))?;

    let entry = providers
        .entry(account.to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("`providers.{account}` is already a value, not a table")
        })?;

    entry.insert("kind".to_string(), Value::String(kind.to_string()));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(&root)?)?;
    Ok(())
}
```

Add `use std::path::Path;` to the file's imports if not already present.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p rupu-cli declare_account`
Expected: PASS (3 tests)

- [ ] **Step 6: Rewrite `login` to resolve kind and store by account**

Replace the `login` function signature and body:

```rust
async fn login(
    account: &str,
    kind_arg: Option<&str>,
    mode: AuthModeArg,
    key: Option<&str>,
) -> anyhow::Result<()> {
    warn_about_stranded_keychain_credentials();

    let global = paths::global_dir()?;
    let cfg_path = global.join("config.toml");
    let cfg = rupu_config::layer_files_locked(Some(&cfg_path), None).unwrap_or_default();

    // Kind precedence: explicit flag, then config, then the account name
    // itself when it is a vendor (design spec §3.1).
    let declared = rupu_runtime::provider_factory::resolve_kind(account, &cfg.providers);
    let kind = match (kind_arg, declared.as_deref()) {
        (Some(k), Some(d)) if k != d => anyhow::bail!(
            "account '{account}' is already declared with kind \"{d}\"; \
             remove [providers.{account}] from {} to change it",
            cfg_path.display()
        ),
        (Some(k), _) => k.to_string(),
        (None, Some(d)) => d.to_string(),
        (None, None) => anyhow::bail!(
            "'{account}' is not a known vendor and is not declared in config. \
             Pass --kind <vendor> to declare it (anthropic | openai | gemini | \
             copilot | github | gitlab | linear | jira | local)."
        ),
    };

    // Persist the declaration so every later resolver call knows this
    // account exists. Skipped when the account name IS the vendor —
    // that needs no config to resolve.
    if kind != account && cfg.providers.get(account).and_then(|p| p.kind.as_deref()).is_none() {
        paths::ensure_dir(&global)?;
        declare_account_in_config(&cfg_path, account, &kind)?;
        println!("rupu: declared [providers.{account}] kind = \"{kind}\"");
    }

    if kind == "openai-compatible" {
        let AuthModeArg::ApiKey = mode else {
            anyhow::bail!("openai-compatible providers only support --mode api-key");
        };
        let secret = read_secret(key)?;
        let resolver = rupu_auth::resolver::KeychainResolver::new();
        let sc = rupu_auth::stored::StoredCredential::api_key(secret);
        resolver
            .store_named(account, rupu_providers::AuthMode::ApiKey, &sc)
            .await?;
        println!("rupu: stored {account} api-key credential");
        return Ok(());
    }

    let pid = rupu_auth::ProviderId::from_vendor_str(&kind)
        .ok_or_else(|| anyhow::anyhow!("unknown vendor kind: {kind}"))?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    let mode_neutral: rupu_providers::AuthMode = mode.clone().into();

    match mode {
        AuthModeArg::ApiKey => {
            let secret = match key {
                Some(k) => k.to_string(),
                None => read_api_key_from_stdin(account, pid)?,
            };
            if secret.is_empty() {
                anyhow::bail!("empty API key");
            }
            let sc = rupu_auth::stored::StoredCredential::api_key(secret);
            // store_named, not store: the ACCOUNT is the key. For a bare
            // vendor name these produce the identical string.
            resolver.store_named(account, mode_neutral, &sc).await?;
            println!("rupu: stored {account} api-key credential");
        }
        AuthModeArg::Sso => {
            let oauth = rupu_auth::oauth::providers::provider_oauth(pid)
                .ok_or_else(|| anyhow::anyhow!("vendor {kind} has no SSO flow"))?;
            let stored = match oauth.flow {
                rupu_auth::oauth::providers::OAuthFlow::Callback => {
                    rupu_auth::oauth::callback::run(pid).await?
                }
                rupu_auth::oauth::providers::OAuthFlow::Device => {
                    rupu_auth::oauth::device::run(pid).await?
                }
            };
            resolver.store_named(account, mode_neutral, &stored).await?;
            println!("rupu: stored {account} sso credential");
        }
    }
    Ok(())
}

/// Read a secret from `--key` or stdin, rejecting empty input.
fn read_secret(key: Option<&str>) -> anyhow::Result<String> {
    let secret = match key {
        Some(k) => k.to_string(),
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        }
    };
    if secret.is_empty() {
        anyhow::bail!("empty API key");
    }
    Ok(secret)
}
```

Preserve the existing SSO match arms exactly as they appear in the current source — the sketch above shows the Callback/Device split, but copy the real arms (including any post-auth handshake steps) rather than retyping from this plan.

- [ ] **Step 7: Apply the same treatment to `logout`**

Add `#[arg(long, alias = "provider")] account: Option<String>` in place of `provider`, and route deletion through `forget_named(account, mode)` instead of `forget(pid, mode)`. `--all` behavior is unchanged.

- [ ] **Step 8: Build and test**

```bash
cargo test -p rupu-cli
cargo build -p rupu-cli
```
Expected: PASS / builds clean.

- [ ] **Step 9: Manual smoke against a temp home**

```bash
export RUPU_HOME=$(mktemp -d)
cargo run -p rupu-cli -- auth login --account anthropic-work --kind anthropic --mode api-key --key sk-test-work
cargo run -p rupu-cli -- auth login --account anthropic-personal --kind anthropic --mode api-key --key sk-test-personal
cat $RUPU_HOME/config.toml
cat $RUPU_HOME/auth.json
```

Expected `auth.json` keys: `anthropic-work/api-key` and `anthropic-personal/api-key`, with distinct values. Expected `config.toml` to contain both `[providers.anthropic-work]` and `[providers.anthropic-personal]` with `kind = "anthropic"`.

- [ ] **Step 10: Verify the legacy path is untouched**

```bash
export RUPU_HOME=$(mktemp -d)
cargo run -p rupu-cli -- auth login --provider anthropic --mode api-key --key sk-legacy
cat $RUPU_HOME/auth.json
test ! -f $RUPU_HOME/config.toml && echo "OK: no config written for a bare vendor name"
```

Expected: `auth.json` contains exactly `anthropic/api-key`, and no `config.toml` was created — a bare vendor name needs no declaration.

- [ ] **Step 11: Lint and commit**

```bash
cargo clippy -p rupu-cli --all-targets -- -D warnings
cargo fmt -- crates/rupu-cli/src/cmd/auth.rs
git add crates/rupu-cli/src/cmd/auth.rs
git commit -m "feat(cli): auth login --account/--kind for multiple accounts per vendor

--provider stays as an alias. Supplying --kind for a new account also
declares it in the global config, so no hand-editing of TOML is needed
before authenticating. Credentials store under <account>/<mode>."
```

---

### Task 6: `rupu auth status` lists accounts with kinds

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs` (the `status` function and `AuthStatusRow`)

**Interfaces:**
- Consumes: `peek_sso_named` (Task 2), `resolve_kind` (Task 4).
- Produces: an `ACCOUNT`/`KIND` table covering built-ins and every declared account.

- [ ] **Step 1: Add `kind` to the row struct**

```rust
struct AuthStatusRow {
    account: String,
    kind: String,
    api_key: bool,
    sso: String,
}
```

Rename the existing `provider` field to `account` and update the JSON report struct and the table header accordingly. The header becomes `ACCOUNT | KIND | API KEY | SSO`.

- [ ] **Step 2: Rewrite the row-collection loop**

Replace the body of `status` between `let mut rows = Vec::new();` and the render call:

```rust
    let global = crate::paths::global_dir().ok();
    let cfg = global
        .as_ref()
        .and_then(|g| rupu_config::layer_files_locked(Some(&g.join("config.toml")), None).ok())
        .unwrap_or_default();

    // Built-in vendor names always listed, so a single-account user sees
    // the same table as before.
    let mut names: Vec<String> = [
        "anthropic", "openai", "gemini", "copilot", "github", "gitlab", "linear", "jira",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Then every declared account, in config order, skipping duplicates.
    for name in cfg.providers.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }

    for name in names {
        let kind = rupu_runtime::provider_factory::resolve_kind(&name, &cfg.providers)
            .unwrap_or_else(|| "-".to_string());
        let api_present = resolver
            .peek_named(&name, rupu_providers::AuthMode::ApiKey)
            .await
            || std::env::var(format!(
                "RUPU_{}_API_KEY",
                name.chars()
                    .map(|c| if c.is_ascii_alphanumeric() {
                        c.to_ascii_uppercase()
                    } else {
                        '_'
                    })
                    .collect::<String>()
            ))
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        rows.push(AuthStatusRow {
            account: name.clone(),
            kind,
            api_key: api_present,
            sso: resolver.peek_sso_named(&name).await.unwrap_or_default(),
        });
    }
```

This deletes the old two-pass structure: the hardcoded `ProviderId` loop and the separate `openai-compatible`-only config pass are replaced by one pass over the union.

- [ ] **Step 3: Build and smoke-test**

```bash
export RUPU_HOME=$(mktemp -d)
cargo run -p rupu-cli -- auth login --account anthropic-work --kind anthropic --mode api-key --key sk-a
cargo run -p rupu-cli -- auth login --account openai-personal --kind openai --mode api-key --key sk-b
cargo run -p rupu-cli -- auth status
```

Expected: a table listing the eight built-ins plus `anthropic-work` (kind `anthropic`, API KEY ✓) and `openai-personal` (kind `openai`, API KEY ✓).

- [ ] **Step 4: Verify JSON output still parses**

Run: `cargo run -p rupu-cli -- auth status --format json | python3 -m json.tool`
Expected: valid JSON with `account` and `kind` fields present.

- [ ] **Step 5: Full workspace check**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
cargo fmt -- crates/rupu-cli/src/cmd/auth.rs
git add crates/rupu-cli/src/cmd/auth.rs
git commit -m "feat(cli): auth status lists every account with its kind

One pass over built-ins plus declared accounts, replacing the hardcoded
ProviderId loop and the separate openai-compatible-only config pass."
```

---

## Acceptance

Arc 1 is done when this transcript reproduces against a scratch `RUPU_HOME` (SSO steps need real browser consent and are verified manually; api-key steps are scriptable):

```bash
rupu auth login --account anthropic-work     --kind anthropic --mode sso
rupu auth login --account anthropic-personal --kind anthropic --mode sso
rupu auth login --account openai-work        --kind openai    --mode sso
rupu auth login --account openai-personal    --kind openai    --mode api-key
rupu auth status
```

```
ACCOUNT             KIND        API KEY   SSO
anthropic-work      anthropic   -         expires in 27d
anthropic-personal  anthropic   -         expires in 30d
openai-work         openai      -         expires in 8d
openai-personal     openai      ✓         -
```

And an agent pinned to one of them runs against that identity:

```yaml
---
name: work-reviewer
provider: anthropic-work
model: claude-opus-4-6
---
```

**Out of scope for Arc 1:** all SCM/GitHub multi-account work (`Registry` reshape, `[[scm.rules]]`, `rupu scm bind`, daemon account resolution). That is Arc 2, specified in §6 of the design doc.

## Wiring note for whoever does Arc 2

`KeychainResolver::with_accounts` (Task 2) exists but **nothing calls it yet** — every CLI construction site still uses `KeychainResolver::new()`, which means declared accounts resolve through `resolve_kind`'s name-fallback only. That is correct and sufficient for Arc 1 because `rupu auth login` writes the config declaration and `provider_factory::resolve_kind` reads config directly.

The resolver's own account list becomes load-bearing when SSO refresh must resolve an account it was not handed — hook `with_accounts` up at the `KeychainResolver::new()` call sites in `rupu-cli` at that point, passing `cfg.providers` mapped to `AccountSpec`.
