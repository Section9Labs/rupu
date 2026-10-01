//! Integration test for the PKCE callback flow.
//!
//! Drives the listener directly — no real browser — by:
//!   1. Setting test seam env vars before spawning.
//!   2. Polling the port file until the listener is ready.
//!   3. POSTing the redirect URL with a matching state directly to the listener.
//!   4. Letting the flow exchange the code against an httpmock'd token endpoint.
//!
//! The seams are process-wide env vars, so the tests are `#[serial]`.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::callback;
use rupu_auth::stored::StoredCredential;
use serial_test::serial;

/// Run the callback flow for `provider` against a token endpoint mocked by
/// `token`, driving the redirect by hand. Returns the stored credential and
/// the mock (for hit assertions).
async fn run_flow(
    provider: ProviderId,
    token: impl FnOnce(httpmock::When, httpmock::Then),
) -> (StoredCredential, httpmock::Mock<'static>) {
    // Each test run uses a unique suffix to avoid env-var collisions when
    // tests run in parallel.
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        .to_string();

    let port_file = std::env::temp_dir().join(format!("rupu-oauth-test-port-{suffix}.txt"));
    let _ = std::fs::remove_file(&port_file);

    // ── Set all env vars before the flow spawns ──────────────────────────
    std::env::remove_var("RUPU_OAUTH_LAST_STATE");
    std::env::set_var("RUPU_OAUTH_SKIP_BROWSER", "1");
    std::env::set_var("RUPU_OAUTH_PORT_FILE", &port_file);

    // ── Mock token endpoint ──────────────────────────────────────────────
    // Leaked on purpose: the mock's lifetime is tied to the server's, and
    // the caller asserts on it after the flow returns.
    let server: &'static MockServer = Box::leak(Box::new(MockServer::start()));
    let token_mock = server.mock(token);
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));

    // ── Spawn the flow ───────────────────────────────────────────────────
    let flow_handle = tokio::spawn(async move { callback::run(provider).await });

    // ── Poll for the port file ───────────────────────────────────────────
    let mut bound_port = 0u16;
    for _ in 0..200 {
        if let Ok(s) = std::fs::read_to_string(&port_file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                bound_port = p;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(bound_port > 0, "listener never wrote port file");

    // ── Read the state the flow exposed ─────────────────────────────────
    // Small spin to let the flow write RUPU_OAUTH_LAST_STATE after the port file.
    let mut state = String::new();
    for _ in 0..40 {
        if let Ok(s) = std::env::var("RUPU_OAUTH_LAST_STATE") {
            if !s.is_empty() {
                state = s;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!state.is_empty(), "flow never set RUPU_OAUTH_LAST_STATE");

    // ── POST the fake redirect ───────────────────────────────────────────
    let url = format!("http://127.0.0.1:{bound_port}/callback?code=stub-code&state={state}");
    let _resp = reqwest::get(&url)
        .await
        .expect("redirect GET should succeed");

    // ── Collect the result ───────────────────────────────────────────────
    let stored = flow_handle.await.unwrap().expect("flow should return Ok");

    // ── Cleanup ──────────────────────────────────────────────────────────
    std::env::remove_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_OAUTH_SKIP_BROWSER");
    std::env::remove_var("RUPU_OAUTH_PORT_FILE");
    std::env::remove_var("RUPU_OAUTH_LAST_STATE");
    let _ = std::fs::remove_file(&port_file);
    (stored, token_mock)
}

#[tokio::test]
#[serial]
async fn callback_completes_with_mocked_token_endpoint() {
    let (stored, token_mock) = run_flow(ProviderId::Anthropic, |when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "test-access",
                "refresh_token": "test-refresh",
                "expires_in": 3600,
                "token_type": "bearer",
            }));
    })
    .await;

    token_mock.assert();
    assert!(
        stored.refresh_token.is_some(),
        "refresh_token must be present"
    );
    assert!(stored.expires_at.is_some(), "expires_at must be present");
}

/// Google's token endpoint wants the installed app's `client_secret` on the
/// code exchange, next to the `client_id` (gemini-cli constructs its
/// `OAuth2Client` with both, and google-auth-library posts `client_secret`
/// form-encoded on `getToken`). The flow sends the Gemini CLI pair.
#[tokio::test]
#[serial]
async fn gemini_login_sends_the_client_secret_on_the_code_exchange() {
    let (stored, token_mock) = run_flow(ProviderId::Gemini, |when, then| {
        when.method(POST)
            .path("/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body_contains("grant_type=authorization_code")
            .body_contains("code=stub-code")
            .body_contains(
                "client_id=681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            )
            .body_contains("client_secret=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");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "g-access",
                "refresh_token": "g-refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
            }));
    })
    .await;

    token_mock.assert();
    assert_eq!(stored.refresh_token.as_deref(), Some("g-refresh"));
}
