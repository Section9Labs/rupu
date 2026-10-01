//! A GitLab credential refreshes as the application that issued it, at the
//! endpoint that issued it.
//!
//! A refresh token is only good with the application it was issued to, and
//! a self-managed instance's tokens come from that instance. The login
//! records both on the credential (`oauth_client_id`, `oauth_token_url`);
//! these tests drive the store's refresher against a mocked token endpoint.
//!
//! `RUPU_HOME` and the token-endpoint seam are process-wide env vars, so
//! the tests are `#[serial]`.

// Throwaway in-process mock-server client, not rupu's egress.
#![allow(clippy::disallowed_methods)]

use httpmock::prelude::*;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;
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

/// An expired SSO credential holding refresh token `r1`, with `extra`.
fn expired(extra: serde_json::Value) -> StoredCredential {
    let at = chrono::Utc::now() - chrono::Duration::minutes(1);
    StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: "a1".into(),
            refresh: "r1".into(),
            expires: at.timestamp_millis() as u64,
            extra: serde_json::from_value(extra).unwrap(),
        },
        refresh_token: Some("r1".into()),
        expires_at: Some(at),
    }
}

fn rotated() -> serde_json::Value {
    serde_json::json!({
        "access_token": "a2",
        "refresh_token": "r2",
        "expires_in": 7200,
        "token_type": "Bearer",
    })
}

#[tokio::test]
#[serial]
async fn a_gitlab_refresh_goes_to_the_recorded_application_and_endpoint() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start_async().await;
    let _home = EnvVarGuard::set("RUPU_HOME", home.path().to_str().unwrap());
    // The built-in endpoints' seam points elsewhere: a recorded endpoint is
    // the instance's own and is never redirected.
    let _url = EnvVarGuard::set(
        "RUPU_OAUTH_TOKEN_URL_OVERRIDE",
        &server.url("/not-this-one"),
    );
    let token = server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body_contains("grant_type=refresh_token")
            .body_contains("refresh_token=r1")
            .body_contains("client_id=corp-app");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(rotated());
    });

    let resolver = KeychainResolver::new();
    let stale = expired(serde_json::json!({
        "oauth_client_id": "corp-app",
        "oauth_token_url": server.url("/oauth/token"),
    }));
    resolver
        .store_named("gl-corp", AuthMode::Sso, &stale)
        .await
        .unwrap();

    let fresh = resolver
        .oauth_refresher("gl-corp", "gitlab")
        .expect("a GitLab OAuth credential gets a refresher")
        .refresh(stale.credentials.clone())
        .await
        .expect("refreshed at the recorded endpoint");

    token.assert_hits(1);
    let AuthCredentials::OAuth { access, extra, .. } = fresh else {
        panic!("expected OAuth");
    };
    assert_eq!(access, "a2");
    // Carried forward, so the next refresh goes to the same place.
    assert_eq!(extra["oauth_client_id"], "corp-app");
    assert_eq!(
        extra["oauth_token_url"],
        server.url("/oauth/token").as_str()
    );
    match resolver.get("gl-corp", Some(AuthMode::Sso)).await.unwrap() {
        (_, AuthCredentials::OAuth { access, extra, .. }) => {
            assert_eq!(access, "a2", "persisted");
            assert_eq!(extra["oauth_client_id"], "corp-app");
        }
        other => panic!("expected stored OAuth, got {other:?}"),
    }
}

/// A credential with no recorded application refreshes as the built-in
/// gitlab.com one (glab's).
#[tokio::test]
#[serial]
async fn a_gitlab_credential_without_a_recorded_application_refreshes_as_glab_s() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start_async().await;
    let _home = EnvVarGuard::set("RUPU_HOME", home.path().to_str().unwrap());
    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", &server.url("/oauth/token"));
    let token = server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .body_contains("grant_type=refresh_token")
            .body_contains("refresh_token=r1")
            .body_contains(
                "client_id=41d48f9422ebd655dd9cf2947d6979681dfaddc6d0c56f7628f6ada59559af1e",
            );
        then.status(200)
            .header("content-type", "application/json")
            .json_body(rotated());
    });

    let resolver = KeychainResolver::new();
    resolver
        .store_named("gitlab", AuthMode::Sso, &expired(serde_json::json!({})))
        .await
        .unwrap();

    let (_, creds) = resolver.get("gitlab", None).await.unwrap();

    token.assert_hits(1);
    match creds {
        AuthCredentials::OAuth { access, .. } => assert_eq!(access, "a2"),
        other => panic!("expected OAuth, got {other:?}"),
    }
}
