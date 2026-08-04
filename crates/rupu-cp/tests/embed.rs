//! End-to-end tests for the embedded web UI + SPA fallback.
//!
//! `rust-embed` embeds `web/dist/` at COMPILE time, so a real
//! `npm run build` (or the build.rs placeholder) must exist before this runs.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;

async fn spawn_server() -> SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    let state =
        rupu_cp::state::AppState::new(dir.path().into(), rupu_config::PricingConfig::default());
    // Keep the tempdir alive for the lifetime of the test process.
    std::mem::forget(dir);
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn spawn_server_with_config(config_toml: &str) -> SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), config_toml).unwrap();
    let state =
        rupu_cp::state::AppState::new(dir.path().into(), rupu_config::PricingConfig::default());
    std::mem::forget(dir);
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn root_serves_embedded_index_html() {
    let addr = spawn_server().await;
    let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"), "content-type was {ct}");
    let body = resp.text().await.unwrap();
    assert!(!body.is_empty(), "index body was empty");
    assert!(
        body.contains("rupu"),
        "index body should mention rupu, got: {body}"
    );
}

#[tokio::test]
async fn unknown_client_route_falls_back_to_index() {
    let addr = spawn_server().await;
    // A path the server has no registered route for and no matching asset:
    // SPA fallback should serve index.html (200 text/html), NOT 404.
    let resp = reqwest::get(format!("http://{addr}/runs/some-client-route"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"), "content-type was {ct}");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("rupu"),
        "fallback body should be the SPA index"
    );
}

#[tokio::test]
async fn api_routes_take_precedence_over_fallback() {
    let addr = spawn_server().await;
    let resp = reqwest::get(format!("http://{addr}/api/dashboard"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.contains("application/json"),
        "dashboard should return JSON, got content-type {ct}"
    );
    // Empty store: should still parse as a JSON object.
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(
        json.is_object(),
        "dashboard payload should be a JSON object"
    );
}

#[tokio::test]
async fn healthz_still_ok() {
    let addr = spawn_server().await;
    let body = reqwest::get(format!("http://{addr}/healthz"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn index_carries_shell_meta_v1_by_default() {
    let addr = spawn_server().await;
    let body = reqwest::get(format!("http://{addr}/")).await.unwrap().text().await.unwrap();
    assert!(
        body.contains(r#"<meta name="rupu-shell" content="v1">"#),
        "expected v1 shell meta, got: {body}"
    );
}

#[tokio::test]
async fn index_carries_shell_meta_v2_when_configured() {
    let addr = spawn_server_with_config("[ui.cp]\nshell = \"v2\"\n").await;
    let body = reqwest::get(format!("http://{addr}/")).await.unwrap().text().await.unwrap();
    assert!(body.contains(r#"<meta name="rupu-shell" content="v2">"#));
}

#[tokio::test]
async fn spa_fallback_also_carries_shell_meta() {
    let addr = spawn_server_with_config("[ui.cp]\nshell = \"v2\"\n").await;
    let body = reqwest::get(format!("http://{addr}/overview"))
        .await.unwrap().text().await.unwrap();
    assert!(body.contains(r#"<meta name="rupu-shell" content="v2">"#));
}
