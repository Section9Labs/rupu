//! Integration tests for the host info endpoint:
//! - `GET /api/host/info`

use reqwest::StatusCode;

async fn spawn_server(dir: &std::path::Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// `GET /api/host/info` returns 200 with version and capabilities.
#[tokio::test]
async fn host_info_returns_version_and_capabilities() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/host/info"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();

    // Check that version field exists and matches CARGO_PKG_VERSION
    let version = body
        .get("version")
        .and_then(|v| v.as_str())
        .expect("version field should be a string");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));

    // Check that capabilities object exists with the three required fields
    let capabilities = body
        .get("capabilities")
        .expect("capabilities field should exist");

    assert!(
        capabilities.get("backends").is_some(),
        "capabilities should have backends field"
    );
    assert!(
        capabilities.get("scm_hosts").is_some(),
        "capabilities should have scm_hosts field"
    );
    assert!(
        capabilities.get("permission_modes").is_some(),
        "capabilities should have permission_modes field"
    );
}
