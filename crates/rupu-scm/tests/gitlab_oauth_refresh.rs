//! GitLab OAuth tokens are refreshed through the credential store before
//! the connectors use them.
//!
//! GitLab OAuth access tokens expire two hours after issue and the refresh
//! token rotates on every refresh, while a `Registry` — and the connectors
//! it built — lives as long as its process (`cp serve`, `mcp serve`, session
//! daemons). These tests drive the real `KeychainResolver` (a temp
//! `RUPU_HOME`) and a mocked token endpoint, and check that a request goes
//! out with the refreshed token and that the rotation lands in `auth.json`
//! for the next process.
//!
//! `RUPU_HOME` and the token-endpoint seam are process-wide env vars, so the
//! tests are `#[serial]`.

use std::sync::Arc;

use httpmock::prelude::*;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;
use rupu_scm::{AccountId, EventSourceRef, Registry, RepoRef};
use serial_test::serial;

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

/// A stored SSO credential whose access token expires `expires_in` from now.
fn sso(access: &str, refresh: &str, expires_in: chrono::Duration) -> StoredCredential {
    let at = chrono::Utc::now() + expires_in;
    StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: access.into(),
            refresh: refresh.into(),
            expires: at.timestamp_millis() as u64,
            extra: Default::default(),
        },
        refresh_token: Some(refresh.into()),
        expires_at: Some(at),
    }
}

/// `[scm.<account>] kind = "gitlab"`, its API at `server`.
fn config_for(account: &str, server: &MockServer) -> rupu_config::Config {
    let mut cfg = rupu_config::Config::default();
    cfg.scm.platforms.insert(
        account.into(),
        rupu_config::ScmPlatformConfig {
            kind: Some("gitlab".into()),
            base_url: Some(server.url("/api/v4")),
            ..Default::default()
        },
    );
    cfg
}

fn mirror() -> RepoRef {
    RepoRef {
        platform: rupu_scm::Platform::Gitlab,
        owner: "section9labs".into(),
        repo: "rupu-mirror".into(),
    }
}

/// The token endpoint: rotates `r1` into `a2` / `r2`, valid two hours.
fn token_endpoint(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .body_contains("grant_type=refresh_token")
            .body_contains("refresh_token=r1");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "a2",
                "refresh_token": "r2",
                "expires_in": 7200,
                "token_type": "Bearer",
            }));
    })
}

/// The project endpoint, answering only the refreshed token.
fn project_endpoint(server: &MockServer) -> httpmock::Mock<'_> {
    let body = std::fs::read_to_string("tests/fixtures/gitlab/project_get_happy.json").unwrap();
    server.mock(move |when, then| {
        when.method(GET)
            .path("/api/v4/projects/section9labs%2Frupu-mirror")
            .header("authorization", "Bearer a2");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    })
}

/// What the store holds for `account` now.
async fn stored_access(home: &std::path::Path, account: &str) -> (String, String) {
    let _home = EnvVarGuard::set("RUPU_HOME", home.to_str().unwrap());
    match KeychainResolver::new()
        .get(account, Some(AuthMode::Sso))
        .await
    {
        Ok((
            _,
            AuthCredentials::OAuth {
                access, refresh, ..
            },
        )) => (access, refresh),
        other => panic!("expected a stored OAuth credential, got {other:?}"),
    }
}

/// An account the resolver was not told about (`get` falls through to
/// `get_named`, which hands back the stored token as-is) holding an
/// already-expired token: the connector refreshes it through the store
/// before the request, and the rotation is persisted.
#[tokio::test]
#[serial]
async fn an_undeclared_account_s_expired_token_is_refreshed_and_persisted_before_use() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start_async().await;
    let _home = EnvVarGuard::set("RUPU_HOME", home.path().to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/oauth/token"));

    let resolver = KeychainResolver::new();
    resolver
        .store_named(
            "gl-work",
            AuthMode::Sso,
            &sso("a1", "r1", -chrono::Duration::minutes(1)),
        )
        .await
        .unwrap();
    let token = token_endpoint(&server);
    let project = project_endpoint(&server);

    let registry = Registry::discover(
        &resolver,
        &config_for("gl-work", &server),
        Arc::new(rupu_netflow::NullSink),
    )
    .await;
    let repo = registry
        .repo_by_account(&AccountId::new("gl-work"))
        .expect("gl-work's repo connector built");
    repo.get_repo(&mirror())
        .await
        .expect("the request goes out with the refreshed token");

    token.assert_hits(1);
    project.assert_hits(1);
    assert_eq!(
        stored_access(home.path(), "gl-work").await,
        ("a2".to_string(), "r2".to_string()),
        "the rotation is persisted for the next process"
    );
}

/// The resolver only refreshes when a credential is read, and a connector
/// reads its credential once, when the registry is built. A token with a
/// few minutes left at that point — outside the resolver's one-minute
/// margin, inside the connector's five — is refreshed before the request,
/// not sent to expire mid-flight.
#[tokio::test]
#[serial]
async fn a_token_that_nears_expiry_after_the_connector_is_built_is_refreshed_before_use() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start_async().await;
    let _home = EnvVarGuard::set("RUPU_HOME", home.path().to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/oauth/token"));

    let resolver = KeychainResolver::new();
    resolver
        .store_named(
            "gitlab",
            AuthMode::Sso,
            &sso("a1", "r1", chrono::Duration::minutes(3)),
        )
        .await
        .unwrap();
    let token = token_endpoint(&server);
    let project = project_endpoint(&server);

    let registry = Registry::discover(
        &resolver,
        &config_for("gitlab", &server),
        Arc::new(rupu_netflow::NullSink),
    )
    .await;
    token.assert_hits(0);
    let repo = registry
        .repo_by_account(&AccountId::new("gitlab"))
        .expect("gitlab's repo connector built");
    repo.get_repo(&mirror())
        .await
        .expect("the request goes out with the refreshed token");

    token.assert_hits(1);
    project.assert_hits(1);
    assert_eq!(
        stored_access(home.path(), "gitlab").await,
        ("a2".to_string(), "r2".to_string())
    );
}

/// The event poller (cron ticks, autoflows) holds its own copy of the
/// credential and refreshes it the same way.
#[tokio::test]
#[serial]
async fn the_event_poller_refreshes_an_expired_token_before_polling() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start_async().await;
    let _home = EnvVarGuard::set("RUPU_HOME", home.path().to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/oauth/token"));

    let resolver = KeychainResolver::new();
    resolver
        .store_named(
            "gl-work",
            AuthMode::Sso,
            &sso("a1", "r1", -chrono::Duration::minutes(1)),
        )
        .await
        .unwrap();
    let token = token_endpoint(&server);
    // Matched on the tail of the path so it holds whichever way the events
    // connector joins `base_url` and `/api/v4`.
    let events = server.mock(|when, then| {
        when.method(GET)
            .path_contains("/projects/section9labs%2Frupu-mirror/events")
            .header("authorization", "Bearer a2");
        then.status(200)
            .header("content-type", "application/json")
            .body("[]");
    });

    let registry = Registry::discover(
        &resolver,
        &config_for("gl-work", &server),
        Arc::new(rupu_netflow::NullSink),
    )
    .await;
    let (_, poller) = registry
        .events_for_source(
            &EventSourceRef::Repo { repo: mirror() },
            None,
            Some(&AccountId::new("gl-work")),
        )
        .expect("gl-work's event connector built");
    let since = format!(
        "since:{}",
        (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339()
    );
    poller
        .poll_events(&EventSourceRef::Repo { repo: mirror() }, Some(&since), 50)
        .await
        .expect("the poll goes out with the refreshed token");

    token.assert_hits(1);
    events.assert_hits(1);
}
