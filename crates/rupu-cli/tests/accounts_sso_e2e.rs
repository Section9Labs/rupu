//! End-to-end proof that declared accounts resolve multi-account SSO.
//!
//! Task 7 exists because `KeychainResolver::with_accounts` (added in an
//! earlier task) had zero production callers: every named-account
//! credential lookup fell through to the api-key-only `get_named` path,
//! so `rupu auth login --account anthropic-work --mode sso` stored a
//! token nothing could ever read back. `rupu_cli::accounts::resolver_for`
//! is the seam that wires a config's declared accounts into the
//! resolver; this test proves that seam actually delivers two
//! independent, correctly-moded SSO credentials for two accounts of the
//! same vendor `kind` — the multi-account arc's headline capability.
//!
//! **The `refresh` discriminator below is load-bearing, not decoration.**
//! A first version of this test asserted only `get()` outcomes, and
//! post-merge review found that every one of those assertions is
//! independently satisfied by Step 4's `get_named` SSO-first fix alone —
//! `get_named` composes the byte-identical `<name>/<mode>` store key that
//! the account-aware `resolve_provider_id` branch does, so a resolver
//! with an EMPTY account list (i.e. `resolver_for` with
//! `.with_accounts(...)` deleted — the exact "zero callers" bug this
//! task exists to close) still passed the whole suite. `refresh` is the
//! one operation only the account-aware branch can satisfy: it needs a
//! resolved `ProviderId` (`resolve_provider_id` must return `Some`) to
//! know which vendor's OAuth endpoint to hit; `get_named` never resolves
//! one and bails immediately with `"unknown provider or account"`. So
//! asserting `refresh` gets PAST that bail (fails later, on the HTTP
//! call, not on account resolution) is the one assertion that actually
//! requires `with_accounts` to have been called.
//!
//! Mutates the process-global `RUPU_AUTH_FILE` / `RUPU_OAUTH_TOKEN_URL_OVERRIDE`
//! env vars, so this test locks a local `ENV_LOCK` for its whole body —
//! this test binary's own serialization point, mirroring the pattern
//! every other env-mutating integration test in this crate (e.g.
//! `cli_auth.rs`) already uses.

// Throwaway in-process mock-server client, not rupu's egress (mirrors
// `rupu-auth/tests/oauth_callback.rs`'s identical allow + rationale).
#![allow(clippy::disallowed_methods)]

use httpmock::prelude::*;
use rupu_auth::{CredentialResolver, StoredCredential};
use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn resolver_for_reads_two_distinct_sso_accounts_of_the_same_kind() {
    let _guard = ENV_LOCK.lock().await;
    let _auth_env = EnvVarGuard::unset("RUPU_AUTH_FILE");
    let _oauth_env = EnvVarGuard::unset("RUPU_OAUTH_TOKEN_URL_OVERRIDE");

    let tmp = tempfile::tempdir().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", &auth_path);

    // Two declared accounts, same vendor kind — the arc's headline case.
    let mut cfg = rupu_config::Config::default();
    cfg.providers.insert(
        "anthropic-work".into(),
        rupu_config::ProviderConfig {
            kind: Some("anthropic".into()),
            ..Default::default()
        },
    );
    cfg.providers.insert(
        "anthropic-personal".into(),
        rupu_config::ProviderConfig {
            kind: Some("anthropic".into()),
            ..Default::default()
        },
    );

    let resolver = rupu_cli::accounts::resolver_for(&cfg);

    // Store two SSO credentials directly under `<name>/sso`, matching
    // exactly what `rupu auth login --account <name> --mode sso` writes.
    let work_sso = StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: "work-token".into(),
            refresh: "work-refresh".into(),
            expires: 0,
            extra: Default::default(),
        },
        refresh_token: Some("work-refresh".into()),
        expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
    };
    let personal_sso = StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: "personal-token".into(),
            refresh: "personal-refresh".into(),
            expires: 0,
            extra: Default::default(),
        },
        refresh_token: Some("personal-refresh".into()),
        expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
    };
    resolver
        .store_named("anthropic-work", AuthMode::Sso, &work_sso)
        .await
        .unwrap();
    resolver
        .store_named("anthropic-personal", AuthMode::Sso, &personal_sso)
        .await
        .unwrap();

    // Each account resolves to ITS OWN token, not the other's, and both
    // land as Sso — not silently downgraded to an api-key/env lookup.
    // NOTE: these assertions alone do NOT prove `with_accounts` was
    // wired — see the module doc comment. The `refresh` discriminator
    // below is what actually proves it.
    let (work_mode, work_creds) = resolver.get("anthropic-work", None).await.unwrap();
    assert_eq!(work_mode, AuthMode::Sso, "anthropic-work must resolve SSO");
    assert!(
        matches!(work_creds, AuthCredentials::OAuth { ref access, .. } if access == "work-token"),
        "anthropic-work resolved the wrong credential: {work_creds:?}"
    );

    let (personal_mode, personal_creds) = resolver.get("anthropic-personal", None).await.unwrap();
    assert_eq!(
        personal_mode,
        AuthMode::Sso,
        "anthropic-personal must resolve SSO"
    );
    assert!(
        matches!(personal_creds, AuthCredentials::OAuth { ref access, .. } if access == "personal-token"),
        "anthropic-personal resolved the wrong credential: {personal_creds:?}"
    );

    // ── The discriminator ────────────────────────────────────────────
    //
    // `refresh` on a declared account must get PAST `resolve_provider_id`
    // (which requires `self.accounts` to be populated) and fail later,
    // on the HTTP call to the vendor's OAuth token endpoint — not bail
    // early with "unknown provider or account". A mocked token endpoint
    // that answers 500 keeps this hermetic (no real network, no
    // flakiness) while still distinguishing "reached the network layer"
    // from "bailed on account resolution": `refresh_inner` only reaches
    // its `client.post(&token_url)` call after `resolve_provider_id`
    // succeeds, so a 500 response is only observable when accounts are
    // wired.
    let server = MockServer::start();
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(500);
    });
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));

    let refresh_err = resolver
        .refresh("anthropic-work", AuthMode::Sso)
        .await
        .expect_err("refresh must fail (mocked 500), but must reach the network to do so");
    let refresh_err_msg = refresh_err.to_string();
    assert!(
        !refresh_err_msg.contains("unknown provider or account"),
        "refresh bailed on account resolution instead of reaching the network — \
         accounts are NOT wired into this resolver (this is the exact \"with_accounts \
         had zero callers\" bug Task 7 exists to close): {refresh_err_msg}"
    );
    assert!(
        refresh_err_msg.contains("refresh failed for 'anthropic-work'")
            && refresh_err_msg.contains("HTTP"),
        "expected an HTTP-layer refresh failure naming the account, got: {refresh_err_msg}"
    );
    token_mock.assert();
}

/// The SCM counterpart of the test above (arc progress ledger, Ruling 7):
/// `rupu_scm::Registry::discover` is the first code in the tree to call
/// `resolver.get(<name>, ..)` for a *non-vendor* SCM account name — but
/// `rupu_cli::accounts::account_specs` originally read `cfg.providers`
/// only, so a declared `[scm.gh-work]` account never became an
/// `AccountSpec` and its SSO token could never refresh. Same
/// discriminator as above: `refresh` is the one operation that requires
/// `resolve_provider_id("gh-work", ..)` to actually resolve.
#[tokio::test]
async fn resolver_for_reads_a_declared_scm_accounts_sso_credential() {
    let _guard = ENV_LOCK.lock().await;
    let _auth_env = EnvVarGuard::unset("RUPU_AUTH_FILE");
    let _oauth_env = EnvVarGuard::unset("RUPU_OAUTH_TOKEN_URL_OVERRIDE");

    let tmp = tempfile::tempdir().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", &auth_path);

    // A single named GitHub account, declared under `[scm.gh-work]` —
    // the shape a two-GitHub-account user's config actually takes.
    let mut cfg = rupu_config::Config::default();
    cfg.scm.platforms.insert(
        "gh-work".into(),
        rupu_config::ScmPlatformConfig {
            kind: Some("github".into()),
            ..Default::default()
        },
    );

    let resolver = rupu_cli::accounts::resolver_for(&cfg);

    // Store the SSO credential directly under `<name>/sso`, matching
    // exactly what `rupu auth login --account gh-work --kind github
    // --mode sso` writes.
    let work_sso = StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: "gh-work-token".into(),
            refresh: "gh-work-refresh".into(),
            expires: 0,
            extra: Default::default(),
        },
        refresh_token: Some("gh-work-refresh".into()),
        expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
    };
    resolver
        .store_named("gh-work", AuthMode::Sso, &work_sso)
        .await
        .unwrap();

    let (mode, creds) = resolver.get("gh-work", None).await.unwrap();
    assert_eq!(mode, AuthMode::Sso, "gh-work must resolve SSO");
    assert!(
        matches!(creds, AuthCredentials::OAuth { ref access, .. } if access == "gh-work-token"),
        "gh-work resolved the wrong credential: {creds:?}"
    );

    // ── The discriminator ────────────────────────────────────────────
    // Same reasoning as the LLM-provider test above: `refresh` only
    // reaches the network (and this mocked 500) once `resolve_provider_id`
    // has resolved "gh-work" to a `ProviderId`, which requires the
    // `[scm.gh-work]` account to have become an `AccountSpec`.
    let server = MockServer::start();
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(500);
    });
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));

    let refresh_err = resolver
        .refresh("gh-work", AuthMode::Sso)
        .await
        .expect_err("refresh must fail (mocked 500), but must reach the network to do so");
    let refresh_err_msg = refresh_err.to_string();
    assert!(
        !refresh_err_msg.contains("unknown provider or account"),
        "refresh bailed on account resolution instead of reaching the network — \
         `[scm.gh-work]` is NOT wired into `account_specs` (this is the exact \
         gap Ruling 7 describes): {refresh_err_msg}"
    );
    assert!(
        refresh_err_msg.contains("refresh failed for 'gh-work'")
            && refresh_err_msg.contains("HTTP"),
        "expected an HTTP-layer refresh failure naming the account, got: {refresh_err_msg}"
    );
    token_mock.assert();
}

/// RAII guard: removes an env var for the test's duration and restores
/// whatever value (if any) was already there on drop, even on panic.
/// Mirrors the identical guard in `crates/rupu-auth/tests/keychain_resolver.rs`.
struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    fn unset(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}
