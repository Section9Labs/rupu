//! The resolver has exactly one storage backend: a chmod-600 JSON file.
//!
//! These tests pin that there is no second path. Unlike the real-keychain
//! round-trip they replace, none of them are `#[ignore]`d — nothing touches
//! the OS keychain any more, so there is no "Always Allow" GUI prompt to
//! hang CI and no reason to opt out of the default run.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;
use serial_test::serial;

/// RAII guard: removes an env var for the test's duration and restores
/// whatever value (if any) was already there on drop, even on panic.
/// `get()` now falls through to the matching `RUPU_<PROVIDER>_API_KEY`
/// for any built-in vendor (spec §5.6), so a "missing after forget"
/// assertion is only reliable if that var is guaranteed unset for the
/// duration -- ambient environment (a developer's shell, CI secrets)
/// must not be able to flake it.
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

/// Full round-trip through the file backend: store, read back, forget.
/// Also asserts the file is created chmod 600 — a credential file that is
/// group- or world-readable is a leak.
#[tokio::test]
#[serial]
async fn file_backend_round_trip() {
    let _env_guard = EnvVarGuard::unset("RUPU_ANTHROPIC_API_KEY");
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", auth_path.as_os_str());

    let r = KeychainResolver::new();
    let sc = StoredCredential::api_key("sk-file-test");
    r.store(ProviderId::Anthropic, AuthMode::ApiKey, &sc)
        .await
        .expect("store");

    assert!(auth_path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "auth.json must be chmod 600, got {mode:o}");
    }

    let (mode, creds) = r
        .get("anthropic", Some(AuthMode::ApiKey))
        .await
        .expect("get from file backend");
    assert_eq!(mode, AuthMode::ApiKey);
    let key = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        other => panic!("expected api-key creds, got {other:?}"),
    };
    assert_eq!(key, "sk-file-test");

    r.forget(ProviderId::Anthropic, AuthMode::ApiKey)
        .await
        .expect("forget");
    assert!(
        r.get("anthropic", Some(AuthMode::ApiKey)).await.is_err(),
        "should be missing after forget"
    );

    std::env::remove_var("RUPU_AUTH_FILE");
}

/// `RUPU_AUTH_BACKEND=keychain` used to select the OS keychain. That
/// backend is gone, so the variable must not divert storage anywhere —
/// silently honoring it would leave a user believing their credentials
/// are in a keystore when they are in a plaintext file.
#[tokio::test]
#[serial]
async fn requesting_the_keychain_by_env_var_still_uses_the_file_store() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", auth_path.as_os_str());
    std::env::set_var("RUPU_AUTH_BACKEND", "keychain");

    let r = KeychainResolver::new();
    r.store(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-no-divert"),
    )
    .await
    .expect("store");

    assert!(
        auth_path.exists(),
        "there is no keychain backend any more; the env var must not divert storage"
    );

    std::env::remove_var("RUPU_AUTH_BACKEND");
    std::env::remove_var("RUPU_AUTH_FILE");
}

/// `with_service` kept its signature for source compatibility, but the
/// service name no longer selects anything. Two resolvers built with
/// different service names must see the same credentials.
#[tokio::test]
#[serial]
async fn the_service_argument_no_longer_selects_a_store() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", auth_path.as_os_str());

    let a = KeychainResolver::with_service("service-one");
    a.store(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-shared"),
    )
    .await
    .expect("store");

    let b = KeychainResolver::with_service("service-two");
    let (_mode, creds) = b
        .get("anthropic", Some(AuthMode::ApiKey))
        .await
        .expect("a differently-named resolver must see the same file");
    match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-shared"),
        other => panic!("expected api-key creds, got {other:?}"),
    }

    std::env::remove_var("RUPU_AUTH_FILE");
}
