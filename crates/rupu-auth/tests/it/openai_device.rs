//! OpenAI's device auth (Codex CLI's `--device-auth`) against an httpmock
//! issuer: the usercode request, the pending polls, the exchange with the
//! server's verifier, and the ways a login ends without a credential.

use std::time::Duration;

use httpmock::prelude::*;
use rupu_auth::oauth::openai_device::{run_with, Options};
use rupu_providers::auth::AuthCredentials;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

fn options(server: &MockServer) -> Options {
    Options {
        issuer: server.base_url(),
        client_id: CLIENT_ID.into(),
        budget: Duration::from_secs(30),
        interval_override: None,
    }
}

/// [`options`] polling with no wait, for the tests that poll more than once.
fn fast_options(server: &MockServer) -> Options {
    Options {
        interval_override: Some(Duration::ZERO),
        ..options(server)
    }
}

fn usercode_mock(server: &MockServer, body: serde_json::Value) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/api/accounts/deviceauth/usercode")
            .header("content-type", "application/json")
            .json_body(serde_json::json!({ "client_id": CLIENT_ID }));
        then.status(200).json_body(body);
    })
}

fn approved_poll_mock(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/api/accounts/deviceauth/token")
            .json_body(serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234" }));
        then.status(200).json_body(serde_json::json!({
            "authorization_code": "ac-1",
            "code_verifier": "srv-verifier",
            "code_challenge": "srv-challenge",
        }));
    })
}

fn pending_poll_mock(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/api/accounts/deviceauth/token");
        then.status(403);
    })
}

fn exchange_mock(server: &MockServer) -> httpmock::Mock<'_> {
    let redirect = format!(
        "redirect_uri={}",
        form_encoded(&format!("{}/deviceauth/callback", server.base_url()))
    );
    server.mock(move |when, then| {
        when.method(POST)
            .path("/oauth/token")
            .body_contains("grant_type=authorization_code")
            .body_contains("code=ac-1")
            .body_contains("code_verifier=srv-verifier")
            .body_contains(&redirect)
            .body_contains(format!("client_id={CLIENT_ID}"));
        then.status(200).json_body(serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "id_token": "idt",
            "expires_in": 3600,
        }));
    })
}

fn form_encoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn assert_stored(stored: &rupu_auth::stored::StoredCredential) {
    match &stored.credentials {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "at");
            assert_eq!(extra.get("id_token").and_then(|v| v.as_str()), Some("idt"));
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    assert_eq!(stored.refresh_token.as_deref(), Some("rt"));
}

#[tokio::test]
async fn device_auth_completes_and_stores_the_id_token() {
    let server = MockServer::start();
    let usercode = usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234", "interval": "0" }),
    );
    let poll = approved_poll_mock(&server);
    let exchange = exchange_mock(&server);

    let stored = run_with(options(&server)).await.expect("login");

    assert_stored(&stored);
    usercode.assert_hits(1);
    poll.assert_hits(1);
    exchange.assert_hits(1);
}

#[tokio::test]
async fn pending_replies_are_polled_until_approval() {
    let server = MockServer::start();
    usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "usercode": "ABCD-1234", "interval": "0" }),
    );
    let exchange = exchange_mock(&server);
    let pending = pending_poll_mock(&server);

    let login = tokio::spawn(run_with(fast_options(&server)));
    while pending.hits_async().await < 2 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let pending_hits = pending.hits_async().await;
    pending.delete_async().await;
    let approved = approved_poll_mock(&server);

    let stored = login.await.unwrap().expect("login");

    assert_stored(&stored);
    assert!(pending_hits >= 2, "{pending_hits}");
    approved.assert_hits(1);
    exchange.assert_hits(1);
}

#[tokio::test]
async fn a_404_poll_is_pending_too() {
    let server = MockServer::start();
    usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234", "interval": "0" }),
    );
    let exchange = exchange_mock(&server);
    let pending = server.mock(|when, then| {
        when.method(POST).path("/api/accounts/deviceauth/token");
        then.status(404);
    });

    let login = tokio::spawn(run_with(fast_options(&server)));
    while pending.hits_async().await < 2 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    pending.delete_async().await;
    let approved = approved_poll_mock(&server);

    assert_stored(&login.await.unwrap().expect("login"));
    approved.assert_hits(1);
    exchange.assert_hits(1);
}

#[tokio::test]
async fn interval_as_a_number_is_accepted() {
    let server = MockServer::start();
    usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234", "interval": 0 }),
    );
    approved_poll_mock(&server);
    exchange_mock(&server);

    assert_stored(&run_with(options(&server)).await.expect("login"));
}

#[tokio::test]
async fn a_missing_interval_is_accepted() {
    let server = MockServer::start();
    usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234" }),
    );
    approved_poll_mock(&server);
    exchange_mock(&server);

    assert_stored(&run_with(options(&server)).await.expect("login"));
}

/// An interval rupu can't read means "none" (the 5 s default), never a
/// failed login.
#[tokio::test]
async fn an_unparsable_interval_does_not_fail_the_login() {
    for interval in [
        serde_json::json!(""),
        serde_json::json!("5.0"),
        serde_json::json!(5.0),
        serde_json::json!(-1),
    ] {
        let server = MockServer::start();
        usercode_mock(
            &server,
            serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234", "interval": interval }),
        );
        let poll = approved_poll_mock(&server);
        let exchange = exchange_mock(&server);

        let stored = run_with(options(&server))
            .await
            .unwrap_or_else(|e| panic!("interval {interval}: {e}"));

        assert_stored(&stored);
        poll.assert_hits(1);
        exchange.assert_hits(1);
    }
}

#[tokio::test]
async fn a_404_on_usercode_says_device_login_is_not_enabled() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/accounts/deviceauth/usercode");
        then.status(404);
    });

    let err = run_with(options(&server)).await.unwrap_err().to_string();

    assert!(
        err.contains("device login is not enabled for this ChatGPT account"),
        "{err}"
    );
    assert!(err.contains("ChatGPT settings"), "{err}");
}

#[tokio::test]
async fn another_usercode_failure_names_its_status() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/accounts/deviceauth/usercode");
        then.status(500);
    });

    let err = run_with(options(&server)).await.unwrap_err().to_string();

    assert!(
        err.contains("device login request failed with status 500"),
        "{err}"
    );
}

#[tokio::test]
async fn the_budget_ends_a_login_nobody_approves() {
    let server = MockServer::start();
    usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234", "interval": "0" }),
    );
    let pending = pending_poll_mock(&server);
    let exchange = exchange_mock(&server);

    // A "0" interval waits the 5 s default between polls; the budget cuts
    // that wait short.
    let started = std::time::Instant::now();
    let err = run_with(Options {
        budget: Duration::from_millis(300),
        ..options(&server)
    })
    .await
    .unwrap_err()
    .to_string();

    assert!(err.contains("device login timed out"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(pending.hits() >= 1);
    exchange.assert_hits(0);
}

#[tokio::test]
async fn an_unexpected_poll_status_fails_the_login() {
    let server = MockServer::start();
    usercode_mock(
        &server,
        serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234", "interval": "0" }),
    );
    let poll = server.mock(|when, then| {
        when.method(POST).path("/api/accounts/deviceauth/token");
        then.status(500);
    });
    let exchange = exchange_mock(&server);

    let err = run_with(options(&server)).await.unwrap_err().to_string();

    assert!(err.contains("device login failed with status 500"), "{err}");
    poll.assert_hits(1);
    exchange.assert_hits(0);
}

#[test]
fn the_verification_page_is_the_issuer_s_codex_device_page() {
    assert_eq!(
        rupu_auth::oauth::openai_device::verification_url("https://auth.openai.com"),
        "https://auth.openai.com/codex/device"
    );
}
