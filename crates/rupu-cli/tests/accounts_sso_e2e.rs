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
//! Mutates the process-global `RUPU_AUTH_FILE` env var, so this test
//! locks a local `ENV_LOCK` for its whole body — this test binary's own
//! serialization point, mirroring the pattern every other env-mutating
//! integration test in this crate (e.g. `cli_auth.rs`) already uses.

use rupu_auth::{CredentialResolver, StoredCredential};
use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn resolver_for_reads_two_distinct_sso_accounts_of_the_same_kind() {
    let _guard = ENV_LOCK.lock().await;
    let _env = EnvVarGuard::unset("RUPU_AUTH_FILE");

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
