//! Integration test for the PKCE callback flow.
//!
//! Drives the listener directly — no real browser — by:
//!   1. Setting test seam env vars before spawning.
//!   2. Polling the port file until the listener is ready.
//!   3. POSTing the redirect URL with a matching state directly to the listener.
//!   4. Letting the flow exchange the code against an httpmock'd token endpoint.

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::callback;

#[tokio::test]
async fn callback_completes_with_mocked_token_endpoint() {
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
    std::env::set_var("RUPU_OAUTH_SKIP_BROWSER", "1");
    std::env::set_var("RUPU_OAUTH_PORT_FILE", &port_file);

    // ── Mock token endpoint ──────────────────────────────────────────────
    let server = MockServer::start();
    let token_mock = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "test-access",
                "refresh_token": "test-refresh",
                "expires_in": 3600,
                "token_type": "bearer",
            }));
    });
    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));

    // ── Spawn the flow ───────────────────────────────────────────────────
    let flow_handle = tokio::spawn(async move { callback::run(ProviderId::Anthropic).await });

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

    token_mock.assert();
    assert!(
        stored.refresh_token.is_some(),
        "refresh_token must be present"
    );
    assert!(stored.expires_at.is_some(), "expires_at must be present");

    // ── Cleanup ──────────────────────────────────────────────────────────
    std::env::remove_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_OAUTH_SKIP_BROWSER");
    std::env::remove_var("RUPU_OAUTH_PORT_FILE");
    std::env::remove_var("RUPU_OAUTH_LAST_STATE");
    let _ = std::fs::remove_file(&port_file);
}
