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

/// Restores a directory's mode on drop, even on panic.
#[cfg(unix)]
struct RestoreMode(std::path::PathBuf);

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// A token rotation the file could not take (a read-only directory) is the
/// live credential, held in the process's overlay. Recording the project
/// merges into THAT credential and lands both — never writing the file's
/// dead refresh token back over it — and a later `get` needs no token
/// request.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_project_recorded_over_an_unpersisted_rotation_lands_both() {
    use httpmock::prelude::*;
    use std::os::unix::fs::PermissionsExt;
    let server = MockServer::start();
    let from_1 = server.mock(|when, then| {
        when.method(POST)
            .path("/token")
            .body_contains("refresh_token=refresh-1");
        then.status(200).json_body(json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "expires_in": 3600
        }));
    });
    let from_2 = server.mock(|when, then| {
        when.method(POST)
            .path("/token")
            .body_contains("refresh_token=refresh-2");
        then.status(200)
            .json_body(json!({ "access_token": "access-3", "expires_in": 3600 }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let dir = tmp.path().join("store");
    std::fs::create_dir_all(&dir).unwrap();
    let auth_path = dir.join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/token"));
    let r = KeychainResolver::new();
    let near = chrono::Utc::now() + chrono::Duration::seconds(10);
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &StoredCredential {
            credentials: AuthCredentials::OAuth {
                access: "access-1".into(),
                refresh: "refresh-1".into(),
                expires: 1,
                extra: HashMap::new(),
            },
            refresh_token: Some("refresh-1".into()),
            expires_at: Some(near),
        },
    )
    .await
    .unwrap();

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let restore = RestoreMode(dir.clone());
    if std::fs::File::create(dir.join("probe")).is_ok() {
        eprintln!("skipping: directory modes are not enforced for this user (root?)");
        return;
    }
    // The rotation lands in memory only: the file keeps `refresh-1`.
    r.get("gemini", Some(AuthMode::Sso)).await.unwrap();
    from_1.assert_hits(1);
    drop(restore);

    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    let recorded = refresher
        .record_extra(holder("refresh-2"), project_fields("managed-1"))
        .await
        .unwrap();

    assert!(recorded);
    let sc = stored_gemini(&auth_path).unwrap();
    assert_eq!(extra_of(&sc)["project_id"], json!("managed-1"));
    match &sc.credentials {
        AuthCredentials::OAuth { refresh, .. } => assert_eq!(refresh, "refresh-2"),
        other => panic!("expected OAuth, got {other:?}"),
    }
    let (_, creds) = r.get("gemini", Some(AuthMode::Sso)).await.unwrap();
    match creds {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "access-2");
            assert_eq!(extra["project_id"], json!("managed-1"));
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    from_1.assert_hits(1);
    from_2.assert_hits(0);
}

/// A credential without a refresh token can't be told apart by it: the
/// access token identifies the grant instead, so a project set up with one
/// login's token never lands on another login's credential.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn without_a_refresh_token_the_access_token_identifies_the_grant() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let r = KeychainResolver::new();
    r.store(
        ProviderId::Gemini,
        AuthMode::Sso,
        &gemini_sso("", HashMap::new()),
    )
    .await
    .unwrap();
    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    let other_login = AuthCredentials::OAuth {
        access: "access-of-another-login".into(),
        refresh: String::new(),
        expires: 0,
        extra: HashMap::new(),
    };

    assert!(!refresher
        .record_extra(other_login, project_fields("managed-1"))
        .await
        .unwrap());
    assert!(!extra_of(&stored_gemini(&auth_path).unwrap()).contains_key("project_id"));

    assert!(refresher
        .record_extra(holder(""), project_fields("managed-1"))
        .await
        .unwrap());
    assert_eq!(
        extra_of(&stored_gemini(&auth_path).unwrap())["project_id"],
        json!("managed-1")
    );
}

/// The credential's own fields are not `extra`'s to overwrite: a reserved
/// key handed to `record_extra` never reaches the stored credential (it
/// would collide with them when the credential is written).
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn reserved_keys_never_reach_the_stored_credential() {
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
    let mut fields = project_fields("managed-1");
    fields.insert("access".to_string(), json!("not-a-token"));

    let refresher = r.oauth_refresher("gemini", "gemini").unwrap();
    assert!(refresher
        .record_extra(holder("refresh-1"), fields)
        .await
        .unwrap());

    let sc = stored_gemini(&auth_path).expect("still parses");
    match &sc.credentials {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "access-1");
            assert!(!extra.contains_key("access"), "{extra:?}");
            assert_eq!(extra["project_id"], json!("managed-1"));
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
}

/// Points Code Assist at `server` and clears the developer's own project
/// variables for the test's duration.
fn code_assist_env(server: &httpmock::MockServer, project: Option<&str>) -> Vec<EnvVarGuard> {
    let mut guards = vec![EnvVarGuard::set(
        "RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE",
        &server.url(""),
    )];
    for var in ["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"] {
        let prior = std::env::var(var).ok();
        std::env::remove_var(var);
        guards.push(EnvVarGuard { key: var, prior });
    }
    if let Some(p) = project {
        std::env::set_var("GOOGLE_CLOUD_PROJECT", p);
    }
    guards
}

/// A login's setup that fails (Code Assist answers 404 here) leaves the
/// stored login exactly as it was: the token is good, and the first Gemini
/// request retries the setup.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_failed_login_setup_keeps_the_stored_login() {
    let server = httpmock::MockServer::start();
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _env = code_assist_env(&server, None);
    let r = KeychainResolver::new();
    let login = gemini_sso("refresh-1", HashMap::new());
    r.store_named("gemini", AuthMode::Sso, &login)
        .await
        .unwrap();
    let before = std::fs::read_to_string(&auth_path).unwrap();

    rupu_auth::oauth::gemini::set_up_code_assist(&r, "gemini", &login).await;

    assert_eq!(std::fs::read_to_string(&auth_path).unwrap(), before);
}

/// With `GOOGLE_CLOUD_PROJECT` set, a login's setup asks for that project
/// but stores nothing: the variable is an override while it is set, not
/// the account's project.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn a_login_setup_with_google_cloud_project_set_stores_no_project() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let load = server.mock(|when, then| {
        when.method(POST)
            .path("/v1internal:loadCodeAssist")
            .json_body_partial(r#"{ "cloudaicompanionProject": "my-project-123" }"#);
        then.status(200).json_body(json!({
            "currentTier": { "id": "standard-tier" },
            "cloudaicompanionProject": "my-project-123",
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str().unwrap());
    let _env = code_assist_env(&server, Some("my-project-123"));
    let r = KeychainResolver::new();
    let login = gemini_sso("refresh-1", HashMap::new());
    r.store_named("gemini", AuthMode::Sso, &login)
        .await
        .unwrap();

    rupu_auth::oauth::gemini::set_up_code_assist(&r, "gemini", &login).await;

    load.assert_hits(1);
    assert!(!extra_of(&stored_gemini(&auth_path).unwrap()).contains_key("project_id"));
}
