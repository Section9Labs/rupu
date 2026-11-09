use std::sync::atomic::{AtomicUsize, Ordering};

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::device;
use rupu_auth::oauth::providers::{OAuthClient, EXTRA_CLIENT_ID, EXTRA_TOKEN_URL};
use rupu_providers::auth::AuthCredentials;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn device_code_flow_completes() {
    let server = MockServer::start();
    let device_mock = server.mock(|when, then| {
        when.method(POST).path("/login/device/code");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "device_code": "DEV-CODE",
                "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 1,
            }));
    });
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/login/oauth/access_token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "ghp-test",
                "token_type": "bearer",
                "scope": "read:user",
            }));
    });

    std::env::set_var(
        "RUPU_DEVICE_DEVICE_URL_OVERRIDE",
        server.url("/login/device/code"),
    );
    std::env::set_var(
        "RUPU_DEVICE_TOKEN_URL_OVERRIDE",
        server.url("/login/oauth/access_token"),
    );
    std::env::set_var("RUPU_DEVICE_FAST_POLL", "1");
    let stored = device::run(ProviderId::Copilot).await.expect("flow ok");
    device_mock.assert();
    token_mock.assert();
    // GitHub's tokens don't expire: the access token stands in for the
    // refresh token, and nothing else is recorded.
    match &stored.credentials {
        AuthCredentials::OAuth {
            access,
            refresh,
            expires,
            extra,
        } => {
            assert_eq!(access, "ghp-test");
            assert_eq!(refresh, "");
            assert_eq!(*expires, 0);
            assert!(extra.is_empty(), "{extra:?}");
        }
        _ => panic!("expected OAuth"),
    }
    assert_eq!(stored.refresh_token.as_deref(), Some("ghp-test"));
    assert_eq!(stored.expires_at, None);
    std::env::remove_var("RUPU_DEVICE_DEVICE_URL_OVERRIDE");
    std::env::remove_var("RUPU_DEVICE_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_DEVICE_FAST_POLL");
}

/// A self-managed GitLab's application, its endpoints on `server`.
fn corp_app(server: &MockServer) -> OAuthClient {
    OAuthClient {
        client_id: "corp-app".into(),
        authorize_url: server.url("/oauth/authorize"),
        token_url: server.url("/oauth/token"),
        device_url: Some(server.url("/oauth/authorize_device")),
    }
}

fn gitlab_device_mock(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/authorize_device")
            .body_contains("client_id=corp-app")
            .body_contains("api");
        then.status(200).json_body(serde_json::json!({
            "device_code": "dc",
            "user_code": "0A44L90H",
            "verification_uri": "https://gitlab.example.com/oauth/device",
            "expires_in": 300,
            "interval": 0,
        }));
    })
}

/// GitLab's token reply (doorkeeper's shape).
fn gitlab_token_body() -> serde_json::Value {
    serde_json::json!({
        "access_token": "at",
        "token_type": "Bearer",
        "expires_in": 7200,
        "refresh_token": "rt",
        "created_at": 1,
    })
}

#[tokio::test]
#[serial]
async fn a_gitlab_device_login_uses_the_chosen_application_and_records_it() {
    let server = MockServer::start();
    let device_mock = gitlab_device_mock(&server);
    let token_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .body_contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
            .body_contains("device_code=dc")
            .body_contains("client_id=corp-app")
            .matches(|req| {
                let body = String::from_utf8_lossy(req.body.as_deref().unwrap_or_default());
                !body.contains("client_secret")
            });
        then.status(200).json_body(gitlab_token_body());
    });

    std::env::set_var("RUPU_DEVICE_FAST_POLL", "1");
    let result = device::run_with_client(ProviderId::Gitlab, Some(corp_app(&server))).await;
    std::env::remove_var("RUPU_DEVICE_FAST_POLL");
    let stored = result.expect("flow ok");

    device_mock.assert_hits(1);
    token_mock.assert_hits(1);
    match &stored.credentials {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "at");
            assert_eq!(extra[EXTRA_CLIENT_ID], "corp-app");
            assert_eq!(extra[EXTRA_TOKEN_URL], server.url("/oauth/token").as_str());
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    assert_eq!(stored.refresh_token.as_deref(), Some("rt"));
    let expires_in = stored.expires_at.expect("an expiry") - chrono::Utc::now();
    assert!(
        (7100..=7200).contains(&expires_in.num_seconds()),
        "{expires_in}"
    );
}

/// Token-endpoint polls this test's server has answered. The first token
/// mock created is the first one httpmock tries for every poll (it tries
/// mocks in creation order and stops at the first match), so its matcher
/// counts each poll exactly once.
static POLLS: AtomicUsize = AtomicUsize::new(0);

#[tokio::test]
#[serial]
async fn gitlab_pending_and_slow_down_keep_polling_until_the_token() {
    POLLS.store(0, Ordering::SeqCst);
    let server = MockServer::start();
    gitlab_device_mock(&server);
    let pending = server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .matches(|_| POLLS.fetch_add(1, Ordering::SeqCst) == 0);
        then.status(400)
            .json_body(serde_json::json!({ "error": "authorization_pending" }));
    });
    // GitLab's `slow_down` names no new interval.
    let slow_down = server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .matches(|_| POLLS.load(Ordering::SeqCst) == 2);
        then.status(400)
            .json_body(serde_json::json!({ "error": "slow_down" }));
    });
    let granted = server.mock(|when, then| {
        when.method(POST).path("/oauth/token");
        then.status(200).json_body(gitlab_token_body());
    });

    std::env::set_var("RUPU_DEVICE_FAST_POLL", "1");
    let result = device::run_with_client(ProviderId::Gitlab, Some(corp_app(&server))).await;
    std::env::remove_var("RUPU_DEVICE_FAST_POLL");

    result.expect("flow ok");
    pending.assert_hits(1);
    slow_down.assert_hits(1);
    granted.assert_hits(1);
    assert_eq!(POLLS.load(Ordering::SeqCst), 3);
}

#[tokio::test]
#[serial]
async fn an_expired_device_code_ends_the_login() {
    let server = MockServer::start();
    gitlab_device_mock(&server);
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/oauth/token");
        then.status(400)
            .json_body(serde_json::json!({ "error": "expired_token" }));
    });

    std::env::set_var("RUPU_DEVICE_FAST_POLL", "1");
    let result = device::run_with_client(ProviderId::Gitlab, Some(corp_app(&server))).await;
    std::env::remove_var("RUPU_DEVICE_FAST_POLL");

    let err = result.unwrap_err().to_string();
    assert!(err.contains("expired_token"), "{err}");
    token_mock.assert_hits(1);
}

#[tokio::test]
#[serial]
async fn a_gitlab_without_the_device_endpoint_names_the_version() {
    let server = MockServer::start();
    let device_mock = server.mock(|when, then| {
        when.method(POST).path("/oauth/authorize_device");
        then.status(404);
    });
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/oauth/token");
        then.status(200).json_body(gitlab_token_body());
    });

    let err = device::run_with_client(ProviderId::Gitlab, Some(corp_app(&server)))
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("GitLab 17.9 or later"), "{err}");
    assert!(err.contains("--mode api-key"), "{err}");
    device_mock.assert_hits(1);
    token_mock.assert_hits(0);
}
