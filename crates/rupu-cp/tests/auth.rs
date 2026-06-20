//! Bearer-token auth for the `/api/*` surface.
//!
//! When a token is configured, `/api/*` requires
//! `Authorization: Bearer <token>` (401 otherwise) while `/healthz` and the
//! static UI / SPA fallback stay open. With no token configured the API is a
//! pass-through (Phase-1 localhost posture).

use rupu_config::PricingConfig;

async fn spawn(token: Option<String>) -> std::net::SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    // Leak the tempdir so it outlives the spawned server for the test.
    let path = dir.keep();
    let state = rupu_cp::state::AppState::new(path, PricingConfig::default());
    let app = rupu_cp::server::router(state, token);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn api_requires_bearer_when_token_set() {
    let addr = spawn(Some("secret123".to_string())).await;
    let client = reqwest::Client::new();

    // No header → 401.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "missing token should be rejected");

    // Wrong token → 401.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .header("Authorization", "Bearer nope")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "wrong token should be rejected");

    // Correct token → 200.
    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .header("Authorization", "Bearer secret123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "correct token should be accepted");
}

#[tokio::test]
async fn healthz_and_static_stay_open_with_token() {
    let addr = spawn(Some("secret123".to_string())).await;
    let client = reqwest::Client::new();

    // /healthz is open even with a token configured.
    let resp = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/healthz must stay open");

    // Static SPA fallback is open (the browser loads without a header).
    let resp = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "static UI must stay open");
}

#[tokio::test]
async fn api_open_when_no_token() {
    let addr = spawn(None).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("http://{addr}/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "no token → API is a pass-through");
}
