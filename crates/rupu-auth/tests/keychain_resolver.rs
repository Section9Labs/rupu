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

    fn set(key: &'static str, value: &str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::set_var(key, value);
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

/// An explicit `hint = Some(Sso)` must never be silently satisfied by
/// the `RUPU_<VENDOR>_API_KEY` env fallback. Before this fix, `get`'s
/// vendor branch fell through to `env_api_key` unconditionally once no
/// stored credential matched any mode in `modes` — which for an SSO
/// hint is a one-element `[Sso]` list, so a missing stored SSO
/// credential plus a present env API key silently returned an API key
/// instead of erroring. That violates docs/providers.md's invariant
/// that SSO never automatically falls back to API-key: the user chose
/// SSO on purpose (e.g. an agent's `auth: sso` frontmatter), and a
/// silent substitution masks the real problem (no SSO credential
/// stored) instead of surfacing it.
#[tokio::test]
#[serial]
async fn explicit_sso_hint_is_not_satisfied_by_env_api_key() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", auth_path.as_os_str());
    let _env_guard = EnvVarGuard::set("RUPU_ANTHROPIC_API_KEY", "sk-should-not-be-used");

    let r = KeychainResolver::new();

    // No stored credential of any kind for anthropic, but the env
    // fallback key IS set. An explicit SSO hint must still error.
    let err = r
        .get("anthropic", Some(AuthMode::Sso))
        .await
        .expect_err("an explicit SSO hint must not be satisfied by an env API key");
    let msg = err.to_string();
    assert!(
        !msg.contains("sk-should-not-be-used"),
        "error must not leak the env api key: {msg}"
    );

    // With no hint at all, the same env key IS still a valid fallback
    // (unconstrained precedence: SSO then API key then env).
    let (mode, creds) = r
        .get("anthropic", None)
        .await
        .expect("unhinted get should still fall back to the env api key");
    assert_eq!(mode, AuthMode::ApiKey);
    match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => {
            assert_eq!(key, "sk-should-not-be-used")
        }
        other => panic!("expected api-key creds, got {other:?}"),
    }

    std::env::remove_var("RUPU_AUTH_FILE");
}

/// The same `hint = Some(Sso)` guard applies on the `get_named` path
/// (an undeclared account name, or an `openai-compatible` entry) as on
/// the declared/vendor path above. Before this fix, `get`'s fallback
/// call to `get_named` dropped `hint` entirely, so `get_named` always
/// tried stored SSO then stored API key then env API key regardless of
/// what the caller asked for -- an agent pinned to an undeclared
/// account name with `auth: sso` frontmatter and a matching
/// `RUPU_<ACCOUNT>_API_KEY` env var would silently receive an API key.
#[tokio::test]
#[serial]
async fn explicit_sso_hint_is_not_satisfied_by_env_api_key_for_undeclared_account() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::env::set_var("RUPU_AUTH_FILE", auth_path.as_os_str());
    let _env_guard = EnvVarGuard::set("RUPU_ORACLE_PERSONAL_API_KEY", "sk-should-not-be-used");

    let r = KeychainResolver::new();

    // "oracle-personal" is not a built-in vendor and not declared in
    // any config, so `get` routes it through `get_named`. No stored
    // credential of any kind exists, but the env fallback key IS set.
    // An explicit SSO hint must still error rather than silently
    // returning that API key.
    let err = r
        .get("oracle-personal", Some(AuthMode::Sso))
        .await
        .expect_err("an explicit SSO hint must not be satisfied by an env API key");
    let msg = err.to_string();
    assert!(
        !msg.contains("sk-should-not-be-used"),
        "error must not leak the env api key: {msg}"
    );

    // With no hint at all, the same env key IS still a valid fallback
    // (unconstrained precedence: SSO then API key then env) -- this
    // path is unchanged.
    let (mode, creds) = r
        .get("oracle-personal", None)
        .await
        .expect("unhinted get should still fall back to the env api key");
    assert_eq!(mode, AuthMode::ApiKey);
    match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => {
            assert_eq!(key, "sk-should-not-be-used")
        }
        other => panic!("expected api-key creds, got {other:?}"),
    }

    std::env::remove_var("RUPU_AUTH_FILE");
}

/// An SSO credential near expiry is refreshed inline by `get`. The OAuth
/// server rotates the refresh token, so a `get` dropped mid-refresh (a
/// pause, a listing timeout) must still persist the new token, and the next
/// `get` must not start a second refresh with the rotated-out one: it waits
/// for the refresh in flight and reads what it stored. Exactly one token
/// request reaches the server.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn an_abandoned_sso_refresh_still_persists_and_is_not_repeated() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .delay(std::time::Duration::from_millis(300))
            .json_body(serde_json::json!({
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "expires_in": 3600
            }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));

    let r = KeychainResolver::new();
    r.store(
        ProviderId::Anthropic,
        AuthMode::Sso,
        &StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "access-1".into(),
                refresh: "refresh-1".into(),
                expires: 1,
                extra: Default::default(),
            },
            refresh_token: Some("refresh-1".into()),
            // Inside the refresh buffer.
            expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(10)),
        },
    )
    .await
    .expect("store");

    let dropped = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        r.get("anthropic", Some(AuthMode::Sso)),
    )
    .await;
    assert!(dropped.is_err(), "the caller gave up mid-refresh");

    let (_, creds) = r
        .get("anthropic", Some(AuthMode::Sso))
        .await
        .expect("the next get succeeds");
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth {
            access, refresh, ..
        } => {
            assert_eq!(access, "access-2");
            assert_eq!(refresh, "refresh-2");
        }
        other => panic!("expected OAuth creds, got {other:?}"),
    }
    token.assert_hits(1);
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(saved.contains("refresh-2"), "{saved}");
}
