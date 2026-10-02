//! Gemini's Code Assist project in the credential store.
//!
//! A Gemini client that sets the account up on its first request (a login
//! from before rupu stored projects) records the project through its
//! refresher: merged into the stored credential's `extra` under the auth
//! file's lock, only into the grant the client holds, and carried through
//! every later refresh. `#[serial]`: these set `RUPU_AUTH_FILE` and the
//! token-endpoint seam.

use std::collections::HashMap;

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;
use serde_json::json;
use serial_test::serial;

/// RAII guard: sets or clears an env var for the test's duration and
/// restores whatever was there on drop, even on panic.
struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
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

fn gemini_sso(refresh: &str, extra: HashMap<String, serde_json::Value>) -> StoredCredential {
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: "access-1".into(),
            refresh: refresh.into(),
            expires: expires.timestamp_millis() as u64,
            extra,
        },
        refresh_token: Some(refresh.into()),
        expires_at: Some(expires),
    }
}

/// What a Gemini client holds: its credential, as `record_extra` is handed it.
fn holder(refresh: &str) -> AuthCredentials {
    AuthCredentials::OAuth {
        access: "access-1".into(),
        refresh: refresh.into(),
        expires: 0,
        extra: HashMap::new(),
    }
}

fn project_fields(project: &str) -> HashMap<String, serde_json::Value> {
    HashMap::from([
        ("project_id".to_string(), json!(project)),
        ("variant".to_string(), json!("gemini-cli")),
    ])
}

/// The stored `gemini/sso` credential, read straight from the file.
fn stored_gemini(auth_path: &std::path::Path) -> Option<StoredCredential> {
    let text = std::fs::read_to_string(auth_path).ok()?;
    let map: HashMap<String, String> = serde_json::from_str(&text).unwrap();
    map.get("gemini/sso")
        .map(|s| serde_json::from_str(s).unwrap())
}

fn extra_of(sc: &StoredCredential) -> &HashMap<String, serde_json::Value> {
    match &sc.credentials {
        AuthCredentials::OAuth { extra, .. } => extra,
        other => panic!("expected OAuth, got {other:?}"),
    }
}

fn sidecar(auth_path: &std::path::Path) -> std::path::PathBuf {
    let mut s = auth_path.as_os_str().to_owned();
    s.push(".lock");
    s.into()
}

/// The recorded project and variant land in the stored credential's
/// `extra`, next to what was already there; the tokens are untouched.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn the_refresher_records_the_project_into_the_stored_credential() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let r = KeychainResolver::new();
    let prior = HashMap::from([("account_hint".to_string(), json!("kept"))]);
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &gemini_sso("refresh-1", prior),
    )
    .await
    .unwrap();

    let refresher = r.oauth_refresher("gemini", "gemini").expect("a refresher");
    let recorded = refresher
        .record_extra(holder("refresh-1"), project_fields("managed-1"))
        .await
        .unwrap();

    assert!(recorded);
    let sc = stored_gemini(&auth_path).expect("still stored");
    let extra = extra_of(&sc);
    assert_eq!(extra["project_id"], json!("managed-1"));
    assert_eq!(extra["variant"], json!("gemini-cli"));
    assert_eq!(extra["account_hint"], json!("kept"));
    match &sc.credentials {
        AuthCredentials::OAuth {
            access, refresh, ..
        } => {
            assert_eq!(access, "access-1");
            assert_eq!(refresh, "refresh-1");
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
}

/// Recording takes the auth file's lock: while another process holds
/// `auth.json.lock`, it waits (bounded) and fails with nothing written.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn recording_waits_for_the_auth_file_lock() {
    use fs2::FileExt;
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let r = KeychainResolver::new().with_lock_timeout(std::time::Duration::from_millis(200));
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &gemini_sso("refresh-1", HashMap::new()),
    )
    .await
    .unwrap();
    let before = std::fs::read_to_string(&auth_path).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(sidecar(&auth_path))
        .unwrap();
    lock.lock_exclusive().unwrap();

    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    let started = std::time::Instant::now();
    let err = refresher
        .record_extra(holder("refresh-1"), project_fields("managed-1"))
        .await
        .expect_err("a held lock fails the write");

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
    assert_eq!(std::fs::read_to_string(&auth_path).unwrap(), before);

    lock.unlock().unwrap();
    assert!(refresher
        .record_extra(holder("refresh-1"), project_fields("managed-1"))
        .await
        .unwrap());
}

/// A re-login since the client read its credential stored another grant
/// (possibly another Google account): the project is not written onto it.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_project_is_not_recorded_into_a_re_logged_in_credential() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let r = KeychainResolver::new();
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &gemini_sso("refresh-of-the-new-login", HashMap::new()),
    )
    .await
    .unwrap();
    let before = std::fs::read_to_string(&auth_path).unwrap();

    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    let recorded = refresher
        .record_extra(holder("refresh-1"), project_fields("managed-1"))
        .await
        .unwrap();

    assert!(!recorded);
    assert_eq!(std::fs::read_to_string(&auth_path).unwrap(), before);
}

/// A logout since: nothing to record into, and nothing is written.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_project_is_not_recorded_after_a_logout() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let r = KeychainResolver::new();
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &gemini_sso("refresh-1", HashMap::new()),
    )
    .await
    .unwrap();
    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    r.forget(ProviderId::Gemini, AuthMode::Sso).await.unwrap();

    let recorded = refresher
        .record_extra(holder("refresh-1"), project_fields("managed-1"))
        .await
        .unwrap();

    assert!(!recorded);
    assert!(stored_gemini(&auth_path).is_none());
}

/// The recorded project is carried through a token refresh (which rewrites
/// the whole credential), into the file and back to the caller.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_recorded_project_survives_a_refresh() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let token = server.mock(|when, then| {
        when.method(POST)
            .path("/token")
            .body_contains("refresh_token=refresh-1");
        then.status(200).json_body(json!({
            "access_token": "access-2",
            "expires_in": 3600
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &gemini_sso("refresh-1", HashMap::new()),
    )
    .await
    .unwrap();
    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    assert!(refresher
        .record_extra(holder("refresh-1"), project_fields("managed-1"))
        .await
        .unwrap());

    let refreshed = r.refresh("gemini", AuthMode::Sso).await.unwrap();

    token.assert_hits(1);
    match refreshed {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "access-2");
            assert_eq!(extra["project_id"], json!("managed-1"));
            assert_eq!(extra["variant"], json!("gemini-cli"));
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    let sc = stored_gemini(&auth_path).unwrap();
    assert_eq!(extra_of(&sc)["project_id"], json!("managed-1"));
}
