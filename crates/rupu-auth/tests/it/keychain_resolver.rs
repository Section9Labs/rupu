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

/// Store an Anthropic SSO credential inside the refresh buffer.
async fn store_near_expiry_sso(r: &KeychainResolver) {
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
            expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(10)),
        },
    )
    .await
    .expect("store");
}

/// The refresh is bounded: a stalled token endpoint fails the `get` after
/// the refresh timeout instead of hanging — and the per-account refresh lock
/// is released, so the next `get` for that account isn't blocked behind it
/// (long-lived processes: the session daemon, `cp serve`).
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_stalled_token_endpoint_times_out_and_releases_the_lock() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .delay(std::time::Duration::from_secs(3))
            .json_body(serde_json::json!({ "access_token": "late" }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new().with_refresh_timeout(std::time::Duration::from_millis(100));
    store_near_expiry_sso(&r).await;

    for attempt in 0..2 {
        let started = std::time::Instant::now();
        let err = r
            .get("anthropic", Some(AuthMode::Sso))
            .await
            .expect_err("a stalled refresh fails");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "attempt {attempt}: bounded by the refresh timeout, not the endpoint ({err})"
        );
    }
}

/// Production refreshes are bounded at 30s.
#[test]
fn the_refresh_timeout_defaults_to_30s() {
    assert_eq!(
        rupu_auth::resolver::REFRESH_TIMEOUT,
        std::time::Duration::from_secs(30)
    );
}

// ---- cross-process: the auth.json.lock sidecar ------------------------------

/// `<auth file>.lock`, the sidecar every credential writer and refresher
/// locks.
fn sidecar(auth_path: &std::path::Path) -> std::path::PathBuf {
    let mut s = auth_path.as_os_str().to_owned();
    s.push(".lock");
    s.into()
}

/// What another `rupu` process does to the file: temp + rename, so a reader
/// sees the old content or the new, never a half-written file.
fn write_atomic(path: &std::path::Path, body: &str) {
    let tmp = path.with_extension("other.tmp");
    std::fs::write(&tmp, body).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// A StoredCredential JSON for the raw file map.
fn sso_payload(access: &str, refresh: &str, expires_in: chrono::Duration) -> String {
    serde_json::to_string(&StoredCredential {
        credentials: rupu_providers::auth::AuthCredentials::OAuth {
            access: access.into(),
            refresh: refresh.into(),
            expires: 1,
            extra: Default::default(),
        },
        refresh_token: Some(refresh.into()),
        expires_at: Some(chrono::Utc::now() + expires_in),
    })
    .unwrap()
}

/// Another process (here: a second open file description on a std thread)
/// holds the lock while it rotates the stored token. A near-expiry `get`
/// must wait for it, re-read under the lock, and adopt the rotation — with
/// no token request of its own (that request would carry the rotated-out
/// refresh token).
///
/// Ordering matters: the `get` reads the file once *before* taking the lock
/// (that read is what finds the credential near expiry), so the holder
/// rotates the file only after the `get` is under way plus a generous
/// margin for that one small read — a rotation visible to the unlocked
/// read would let the `get` return fresh without ever waiting, which is a
/// different (and here untested) path. The rotation is written temp +
/// rename, as a real process writes it.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_refresh_waits_for_another_process_and_adopts_its_rotation() {
    use fs2::FileExt;
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .json_body(serde_json::json!({ "access_token": "mine" }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    store_near_expiry_sso(&r).await;

    const MARGIN: std::time::Duration = std::time::Duration::from_millis(500);
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (get_started_tx, get_started_rx) = std::sync::mpsc::channel::<()>();
    let other = {
        let auth_path = auth_path.clone();
        std::thread::spawn(move || {
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(sidecar(&auth_path))
                .unwrap();
            lock.lock_exclusive().unwrap();
            locked_tx.send(()).unwrap();
            get_started_rx.recv().unwrap();
            std::thread::sleep(MARGIN);
            // The other process's rotation, under its lock.
            let map = serde_json::json!({
                "anthropic/sso": sso_payload("theirs", "refresh-theirs", chrono::Duration::hours(1)),
            });
            write_atomic(&auth_path, &serde_json::to_string(&map).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(100));
            lock.unlock().unwrap();
            std::time::Instant::now()
        })
    };
    locked_rx.recv().unwrap();

    let started = std::time::Instant::now();
    get_started_tx.send(()).unwrap();
    let (_, creds) = r
        .get("anthropic", Some(AuthMode::Sso))
        .await
        .expect("get succeeds");
    let finished = std::time::Instant::now();
    let unlocked_at = other.join().unwrap();
    assert!(
        finished >= unlocked_at,
        "the get finished only once the other holder released the lock (early by {:?})",
        unlocked_at.saturating_duration_since(finished)
    );
    assert!(
        started.elapsed() >= MARGIN,
        "waited for the other holder ({:?})",
        started.elapsed()
    );
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth {
            access, refresh, ..
        } => {
            assert_eq!(
                (access.as_str(), refresh.as_str()),
                ("theirs", "refresh-theirs")
            );
        }
        other => panic!("expected OAuth creds, got {other:?}"),
    }
    token.assert_hits(0);
}

/// Plain writes (login, logout) take the same lock: a store waits for
/// another process's read-modify-write instead of clobbering it.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_store_waits_for_another_process_holding_the_lock() {
    use fs2::FileExt;
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let other = {
        let auth_path = auth_path.clone();
        std::thread::spawn(move || {
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(sidecar(&auth_path))
                .unwrap();
            lock.lock_exclusive().unwrap();
            locked_tx.send(()).unwrap();
            let map = serde_json::json!({ "openai/api-key": "{\"credentials\":{\"type\":\"api_key\",\"key\":\"sk-theirs\"}}" });
            write_atomic(&auth_path, &serde_json::to_string(&map).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(300));
            lock.unlock().unwrap();
        })
    };
    locked_rx.recv().unwrap();
    let started = std::time::Instant::now();
    KeychainResolver::new()
        .store(
            ProviderId::Anthropic,
            AuthMode::ApiKey,
            &StoredCredential::api_key("sk-mine"),
        )
        .await
        .expect("store");
    assert!(started.elapsed() >= std::time::Duration::from_millis(200));
    other.join().unwrap();
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(
        saved.contains("sk-theirs") && saved.contains("sk-mine"),
        "the other writer's entry survives: {saved}"
    );
    // Written through a temp file + rename: nothing left behind.
    let stray: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

/// A provider client whose own token is due (its 5-minute buffer) asks the
/// resolver's refresher. When another holder already rotated the stored
/// credential, the refresher hands that back — no token request.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn the_refresher_adopts_another_holder_s_rotation() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .json_body(serde_json::json!({ "access_token": "mine" }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    std::fs::write(
        &auth_path,
        serde_json::to_string(&serde_json::json!({
            "anthropic/sso": sso_payload("access-2", "refresh-2", chrono::Duration::hours(1)),
        }))
        .unwrap(),
    )
    .unwrap();
    let refresher = r
        .oauth_refresher("anthropic", "anthropic")
        .expect("a vendor account has a refresher");
    let fresh = refresher
        .refresh(rupu_providers::auth::AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: 1,
            extra: Default::default(),
        })
        .await
        .expect("refresh");
    match fresh {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(access, "access-2")
        }
        other => panic!("{other:?}"),
    }
    token.assert_hits(0);
}

/// When the stored credential is still the one the client holds, the
/// refresher refreshes it — and persists the rotation, which a client
/// refreshing on its own would have kept only in memory.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn the_refresher_refreshes_and_persists_the_token_its_client_holds() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token").body_contains("refresh-1");
        then.status(200).json_body(serde_json::json!({
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
    // Outside the resolver's 60s buffer, inside a provider's 5 minutes.
    std::fs::write(
        &auth_path,
        serde_json::to_string(&serde_json::json!({
            "anthropic/sso": sso_payload("access-1", "refresh-1", chrono::Duration::minutes(3)),
        }))
        .unwrap(),
    )
    .unwrap();
    let fresh = r
        .oauth_refresher("anthropic", "anthropic")
        .unwrap()
        .refresh(rupu_providers::auth::AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: 1,
            extra: Default::default(),
        })
        .await
        .expect("refresh");
    token.assert_hits(1);
    match fresh {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(access, "access-2")
        }
        other => panic!("{other:?}"),
    }
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(saved.contains("refresh-2"), "persisted: {saved}");
}

// ---- private files: modes and symlinks ---------------------------------------

#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Both files the store creates are private: `auth.json` (the secrets) and
/// `auth.json.lock` (which every process opens for writing).
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn the_auth_file_and_its_lock_file_are_created_private() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    KeychainResolver::new()
        .store(
            ProviderId::Anthropic,
            AuthMode::ApiKey,
            &StoredCredential::api_key("sk-private"),
        )
        .await
        .expect("store");
    assert_eq!(mode_of(&auth_path), 0o600, "auth.json");
    assert_eq!(mode_of(&sidecar(&auth_path)), 0o600, "auth.json.lock");
}

/// A user who keeps `auth.json` as a symlink (a dotfiles checkout, a shared
/// volume) keeps it: the write lands in the link's target, and the lock
/// sits next to that target so every path to the same file takes the same
/// lock.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn a_symlinked_auth_file_is_written_through_to_its_target() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let real = tmp.path().join("real").join("creds.json");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, "{}").unwrap();
    let auth_path = tmp.path().join("auth.json");
    std::os::unix::fs::symlink(&real, &auth_path).unwrap();
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());

    let r = KeychainResolver::new();
    r.store(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-through-link"),
    )
    .await
    .expect("store");

    assert!(
        std::fs::symlink_metadata(&auth_path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "auth.json is still the symlink"
    );
    assert!(
        std::fs::read_to_string(&real)
            .unwrap()
            .contains("sk-through-link"),
        "the target holds the credential"
    );
    assert!(sidecar(&real).exists(), "the lock sits next to the target");
    assert!(
        !sidecar(&auth_path).exists(),
        "no lock next to the link itself"
    );
    let (_, creds) = r
        .get("anthropic", Some(AuthMode::ApiKey))
        .await
        .expect("reads back through the link");
    assert!(
        matches!(creds, rupu_providers::auth::AuthCredentials::ApiKey { key } if key == "sk-through-link")
    );
}

// ---- bounded lock wait --------------------------------------------------------

/// A lock holder that never lets go (a SIGSTOPped process, a hung NFS
/// lockd, a co-tenant flocking the file) fails the operation after the
/// lock timeout with an error naming the lock file — it never hangs the
/// refresh, login or logout, and it writes nothing. Both a plain write and
/// a refresh are bounded the same way.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_held_lock_fails_the_operation_after_the_lock_timeout() {
    use fs2::FileExt;
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .json_body(serde_json::json!({ "access_token": "mine" }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new().with_lock_timeout(std::time::Duration::from_millis(200));
    store_near_expiry_sso(&r).await;
    let before = std::fs::read_to_string(&auth_path).unwrap();

    let holder = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(sidecar(&auth_path))
        .unwrap();
    holder.lock_exclusive().unwrap();

    // A write.
    let started = std::time::Instant::now();
    let err = r
        .store(
            ProviderId::Openai,
            AuthMode::ApiKey,
            &StoredCredential::api_key("sk-mine"),
        )
        .await
        .expect_err("a held lock fails the store");
    let waited = started.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(200)
            && waited < std::time::Duration::from_secs(2),
        "bounded by the lock timeout: {waited:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("locked by another process") && msg.contains("auth.json.lock"),
        "{msg}"
    );

    // A refresh (near-expiry `get`): same bound, no token request.
    let started = std::time::Instant::now();
    let err = r
        .get("anthropic", Some(AuthMode::Sso))
        .await
        .expect_err("a held lock fails the refresh");
    let waited = started.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(200)
            && waited < std::time::Duration::from_secs(2),
        "bounded by the lock timeout: {waited:?}"
    );
    assert!(
        err.to_string().contains("locked by another process"),
        "{err}"
    );
    token.assert_hits(0);
    assert_eq!(
        std::fs::read_to_string(&auth_path).unwrap(),
        before,
        "nothing was written while the lock was held"
    );

    // Released: the same resolver works again.
    holder.unlock().unwrap();
    r.store(
        ProviderId::Openai,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-mine"),
    )
    .await
    .expect("store after release");
}

/// Production waits at most 30s for the lock.
#[test]
fn the_lock_timeout_defaults_to_30s() {
    assert_eq!(
        rupu_auth::resolver::LOCK_TIMEOUT,
        std::time::Duration::from_secs(30)
    );
}

// ---- a rotation that cannot be persisted ------------------------------------

/// Captures what a `tracing` subscriber writes.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Restores a directory's mode on drop (so the temp dir can be removed
/// even when an assertion fails first).
#[cfg(unix)]
struct RestoreMode(std::path::PathBuf);

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// The token endpoint has already rotated the refresh token when the write
/// of the new credential fails (a read-only directory, a full disk). The
/// credential in hand is then the only live one: the `get` still returns
/// it, so the run continues, and the loss is reported at ERROR naming the
/// file — never dropped silently. The file itself is untouched.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn a_persist_failure_after_the_token_post_still_returns_the_new_token() {
    use httpmock::prelude::*;
    use std::os::unix::fs::PermissionsExt;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token").body_contains("refresh-1");
        then.status(200).json_body(serde_json::json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "expires_in": 3600
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let dir = tmp.path().join("store");
    std::fs::create_dir_all(&dir).unwrap();
    let auth_path = dir.join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    store_near_expiry_sso(&r).await;
    let before = std::fs::read_to_string(&auth_path).unwrap();

    // No new file can be created in a read-only directory, so the temp +
    // rename write fails; the lock file already exists and still opens.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let _restore = RestoreMode(dir.clone());
    if std::fs::File::create(dir.join("probe")).is_ok() {
        eprintln!("skipping: directory modes are not enforced for this user (root?)");
        return;
    }
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);

    let (_, creds) = r
        .get("anthropic", Some(AuthMode::Sso))
        .await
        .expect("the get still succeeds with the rotated token");
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth {
            access, refresh, ..
        } => assert_eq!(
            (access.as_str(), refresh.as_str()),
            ("access-2", "refresh-2")
        ),
        other => panic!("expected OAuth creds, got {other:?}"),
    }
    token.assert_hits(1);
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("ERROR")
            && log.contains("auth.json")
            && log.contains("could not be persisted"),
        "the loss is logged loudly, naming the file: {log:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&auth_path).unwrap(),
        before,
        "the file is untouched — nothing half-written"
    );
}

/// After a persist failure the file still holds the rotated-out refresh
/// token, so every later refresh in this process that re-read the file
/// would post the dead token (`invalid_grant`, and possibly a revoked
/// grant). The rotation the file could not take is kept in a process-wide
/// overlay, consulted under the lock before the file: a later `get` hands
/// it out with no token request, a refresher adopts it, a forced refresh
/// rotates FROM it, and the first write that succeeds lands it and drops
/// the overlay — after which the file is the truth again.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn an_unpersisted_rotation_is_used_by_later_refreshes_in_the_process() {
    use httpmock::prelude::*;
    use std::os::unix::fs::PermissionsExt;
    let server = MockServer::start();
    let rotate = |from: &'static str, access: &'static str, refresh: &'static str| {
        server.mock(move |when, then| {
            when.method(POST).path("/token").body_contains(from);
            then.status(200).json_body(serde_json::json!({
                "access_token": access,
                "refresh_token": refresh,
                "expires_in": 3600
            }));
        })
    };
    let from_1 = rotate("refresh-1", "access-2", "refresh-2");
    let from_2 = rotate("refresh-2", "access-3", "refresh-3");
    let from_9 = rotate("refresh-9", "access-10", "refresh-10");
    let tmp = assert_fs::TempDir::new().unwrap();
    let dir = tmp.path().join("store");
    std::fs::create_dir_all(&dir).unwrap();
    let auth_path = dir.join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    store_near_expiry_sso(&r).await;
    let before = std::fs::read_to_string(&auth_path).unwrap();
    let access_of = |creds: rupu_providers::auth::AuthCredentials| match creds {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
        other => panic!("expected OAuth creds, got {other:?}"),
    };

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let restore = RestoreMode(dir.clone());
    if std::fs::File::create(dir.join("probe")).is_ok() {
        eprintln!("skipping: directory modes are not enforced for this user (root?)");
        return;
    }

    // The rotation lands in memory only.
    let (_, creds) = r.get("anthropic", Some(AuthMode::Sso)).await.unwrap();
    assert_eq!(access_of(creds), "access-2");
    from_1.assert_hits(1);
    assert_eq!(std::fs::read_to_string(&auth_path).unwrap(), before);

    // A later `get` (the file still says near-expiry `refresh-1`) hands out
    // the rotation with no token request.
    let (_, creds) = r.get("anthropic", Some(AuthMode::Sso)).await.unwrap();
    assert_eq!(access_of(creds), "access-2");
    from_1.assert_hits(1);

    // A client refreshing through the store (another step's factory client,
    // still holding `refresh-1`) adopts it too.
    let refresher = r.oauth_refresher("anthropic", "anthropic").unwrap();
    let adopted = refresher
        .refresh(rupu_providers::auth::AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: 1,
            extra: Default::default(),
        })
        .await
        .unwrap();
    assert_eq!(access_of(adopted), "access-2");
    from_1.assert_hits(1);

    // A forced refresh rotates FROM the rotation, never from the file's
    // dead token — and its own persist fails the same way.
    let creds = r.refresh("anthropic", AuthMode::Sso).await.unwrap();
    assert_eq!(access_of(creds), "access-3");
    from_2.assert_hits(1);
    assert_eq!(std::fs::read_to_string(&auth_path).unwrap(), before);

    // The directory is writable again: the next refresh lands the pending
    // rotation first (no token request) and the file is the truth again.
    drop(restore);
    let (_, creds) = r.get("anthropic", Some(AuthMode::Sso)).await.unwrap();
    assert_eq!(access_of(creds), "access-3");
    from_2.assert_hits(1);
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(saved.contains("refresh-3"), "persisted on retry: {saved}");

    // Another process rotates the file: a refresh now posts THAT token —
    // the overlay no longer shadows the file.
    write_atomic(
        &auth_path,
        &serde_json::json!({
            "anthropic/sso": sso_payload("access-9", "refresh-9", chrono::Duration::hours(1))
        })
        .to_string(),
    );
    let creds = r.refresh("anthropic", AuthMode::Sso).await.unwrap();
    assert_eq!(access_of(creds), "access-10");
    from_9.assert_hits(1);
    assert!(std::fs::read_to_string(&auth_path)
        .unwrap()
        .contains("refresh-10"));
}

// ---- refresh request formats (per provider, as the vendor's own client) ------

/// Store a near-expiry SSO credential for `kind` with the given `extra`.
async fn store_near_expiry_sso_for(
    r: &KeychainResolver,
    kind: ProviderId,
    extra: std::collections::HashMap<String, serde_json::Value>,
) {
    r.store(
        kind,
        AuthMode::Sso,
        &StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: "access-1".into(),
                refresh: "refresh-1".into(),
                expires: 1,
                extra,
            },
            refresh_token: Some("refresh-1".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(10)),
        },
    )
    .await
    .expect("store");
}

fn expect_rotated(creds: rupu_providers::auth::AuthCredentials) {
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => {
            assert_eq!(access, "access-2")
        }
        other => panic!("expected OAuth creds, got {other:?}"),
    }
}

/// OpenAI's token endpoint takes the refresh grant as JSON — what Codex
/// sends (openai/codex `codex-rs/login/src/oauth/client.rs`: "ChatGPT
/// refresh uses JSON; authorization-code and gateway grants use form
/// encoding"), and what rupu's own Codex client sends.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn an_openai_refresh_posts_a_json_body() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST)
            .path("/token")
            .header("content-type", "application/json")
            .json_body_partial(
                r#"{"grant_type":"refresh_token","refresh_token":"refresh-1","client_id":"app_EMoamEEZ73f0CkXaXp7hrann"}"#,
            );
        then.status(200).json_body(serde_json::json!({
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
    store_near_expiry_sso_for(&r, ProviderId::Openai, Default::default()).await;
    let (_, creds) = r
        .get("openai", Some(AuthMode::Sso))
        .await
        .expect("refresh as Codex does");
    token.assert_hits(1);
    expect_rotated(creds);
}

/// OpenAI's grant rotates the ID token with the access token; it is
/// persisted with the credential (`extra.id_token`), as codex-rs's
/// `persist_tokens` persists `id_token`, `access_token` and
/// `refresh_token` — the Codex client takes its account id from it when
/// the access token carries no claim.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn an_openai_refresh_persists_the_id_token() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200).json_body(serde_json::json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "id_token": "id-token-2",
            "expires_in": 3600
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    let mut prior = std::collections::HashMap::new();
    prior.insert(
        "id_token".to_string(),
        serde_json::Value::String("id-token-1".into()),
    );
    store_near_expiry_sso_for(&r, ProviderId::Openai, prior).await;
    let (_, creds) = r.get("openai", Some(AuthMode::Sso)).await.expect("refresh");
    match creds {
        rupu_providers::auth::AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "access-2");
            assert_eq!(
                extra.get("id_token").and_then(|v| v.as_str()),
                Some("id-token-2"),
                "the rotated ID token replaces the stored one"
            );
        }
        other => panic!("expected OAuth creds, got {other:?}"),
    }
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(
        saved.contains("id-token-2") && !saved.contains("id-token-1"),
        "{saved}"
    );
}

/// Anthropic's refresh grant goes as JSON — what Claude Code's own
/// `refreshOAuthToken` sends (oboard/claude-code-rev,
/// `src/services/oauth/client.ts`: `axios.post(TOKEN_URL, { grant_type:
/// 'refresh_token', refresh_token, client_id, scope }, { headers: {
/// 'Content-Type': 'application/json' } })`), and what rupu's own
/// Anthropic client sends (`refresh_anthropic_token_at`).
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn an_anthropic_refresh_posts_a_json_body() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST)
            .path("/token")
            .header("content-type", "application/json")
            .json_body_partial(
                r#"{"grant_type":"refresh_token","refresh_token":"refresh-1","client_id":"9d1c250a-e61b-44d9-88ed-5944d1962f5e"}"#,
            );
        then.status(200)
            .json_body(serde_json::json!({ "access_token": "access-2" }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    store_near_expiry_sso(&r).await;
    let (_, creds) = r
        .get("anthropic", Some(AuthMode::Sso))
        .await
        .expect("refresh");
    token.assert_hits(1);
    expect_rotated(creds);
}

/// Google's endpoint wants the installed app's `client_id` AND
/// `client_secret` on the refresh grant (form-encoded) — what gemini-cli's
/// `OAuth2Client` sends (`packages/core/src/code_assist/oauth2.ts`
/// constructs it with both; google-auth-library's `refreshTokenNoCache`
/// posts `refresh_token`, `client_id`, `client_secret`, `grant_type` as
/// `URLSearchParams`), and what rupu's own Gemini client sends. The pair
/// follows the credential's variant: Gemini CLI by default, Antigravity
/// when `extra.variant` says so.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_gemini_refresh_sends_the_variant_s_client_id_and_secret() {
    use httpmock::prelude::*;
    for (variant, client_id, client_secret) in [
        (
            None,
            "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            "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",
        ),
        (
            Some("antigravity"),
            "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
            "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",
        ),
    ] {
        let server = MockServer::start();
        let token = server.mock(|when, then| {
            when.method(POST)
                .path("/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=refresh-1")
                .body_contains(format!("client_id={client_id}"))
                .body_contains(format!("client_secret={client_secret}"));
            then.status(200)
                .json_body(serde_json::json!({ "access_token": "access-2" }));
        });
        let tmp = assert_fs::TempDir::new().unwrap();
        let auth_path = tmp.path().join("auth.json");
        let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
        let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
        let r = KeychainResolver::new();
        let mut extra = std::collections::HashMap::new();
        if let Some(v) = variant {
            extra.insert("variant".to_string(), serde_json::Value::String(v.into()));
        }
        store_near_expiry_sso_for(&r, ProviderId::Gemini, extra).await;
        let (_, creds) = r
            .get("gemini", Some(AuthMode::Sso))
            .await
            .unwrap_or_else(|e| panic!("variant {variant:?}: {e}"));
        token.assert_hits(1);
        expect_rotated(creds);
    }
}

// ---- every OAuth credential gets a refresher ---------------------------------

/// A credential stored under a name the resolver was not given as an
/// account (a bare `KeychainResolver::new()`, as `rupu dispatch` builds)
/// still gets a refresher: the factory knows the vendor kind from config
/// and passes it, so the client's refresh goes through the store and the
/// rotation is persisted — instead of rotating in memory only and leaving
/// a dead refresh token behind for the next process.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn an_undeclared_oauth_credential_gets_a_refresher_from_its_kind() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST).path("/token").body_contains("refresh-1");
        then.status(200).json_body(serde_json::json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "expires_in": 3600
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    // No accounts declared: `oracle-personal` is undeclared here.
    let r = KeychainResolver::new();
    std::fs::write(
        &auth_path,
        serde_json::to_string(&serde_json::json!({
            "oracle-personal/sso": sso_payload("access-1", "refresh-1", chrono::Duration::minutes(3)),
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        r.oauth_refresher("oracle-personal", "no-such-vendor")
            .is_none(),
        "an unknown kind has no OAuth config to refresh against"
    );
    let refresher = r
        .oauth_refresher("oracle-personal", "anthropic")
        .expect("the kind from config is enough");
    let fresh = refresher
        .refresh(rupu_providers::auth::AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: 1,
            extra: Default::default(),
        })
        .await
        .expect("refresh");
    token.assert_hits(1);
    expect_rotated(fresh);
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(
        saved.contains("\"oracle-personal/sso\"") && saved.contains("refresh-2"),
        "persisted under the undeclared name: {saved}"
    );
}
