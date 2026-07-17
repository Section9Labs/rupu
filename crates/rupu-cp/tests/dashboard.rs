//! Integration tests for `GET /api/dashboard` (Task 7: fan-out across hosts).
//!
//! The endpoint now fans `dashboard_summary()` out across every registered
//! host and merges only the hosts that actually reported. A host that cannot
//! report (offline, or `Unsupported`) must surface in `hosts[]` as
//! `offline` / `unavailable` rather than contributing zeroed counts.

// ---------------------------------------------------------------------------
// Spawn helpers (mirrors tests/host_reads.rs; helpers are duplicated per file
// — there is no shared `tests/common/` module in this crate).
// ---------------------------------------------------------------------------

struct TestServer {
    base_url: String,
}

/// Spin up a read-only local-only server.
async fn spawn_server(dir: &std::path::Path) -> TestServer {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{addr}"),
    }
}

/// Spin up a server with one remote host pre-registered via the registry.
async fn spawn_server_with_remote(dir: &std::path::Path, mock_base_url: &str) -> TestServer {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    state
        .hosts
        .add_host("mock-remote", mock_base_url, None)
        .expect("add_host should succeed");
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{addr}"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dashboard_reports_per_host_freshness_and_never_zeroes_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert!(!hosts.is_empty(), "local must always appear");
    let local = &hosts[0];
    assert_eq!(local["host_id"], "local");
    assert_eq!(local["state"], "ok");
    assert!(
        local["captured_at"].as_str().unwrap().contains('T'),
        "captured_at must be RFC-3339 for the freshness strip"
    );
}

#[tokio::test]
async fn dashboard_rejects_unknown_range() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/dashboard?range=bogus", srv.base_url))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "an unparseable range must 400, not silently default"
    );
}

#[tokio::test]
async fn dashboard_unavailable_host_renders_unavailable_not_zero() {
    // A host that cannot report is NOT a host with no runs. Register an
    // unreachable remote and assert it surfaces as a distinct state.
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().unwrap();
    let remote = hosts
        .iter()
        .find(|h| h["host_id"] != "local")
        .expect("the unreachable remote must still appear in the freshness strip");
    assert_ne!(
        remote["state"], "ok",
        "an unreachable host must not report ok"
    );
    assert!(
        remote["captured_at"].is_null(),
        "an unreachable host has no captured_at — it never reported"
    );
}

#[tokio::test]
async fn dashboard_unknown_host_returns_404() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/dashboard?host=nope", srv.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "an unknown host id must 404");
}

#[tokio::test]
async fn dashboard_scoped_to_host_local_returns_only_local() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value =
        reqwest::get(format!("{}/api/dashboard?host=local", srv.base_url))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert_eq!(
        hosts.len(),
        1,
        "?host=local must not also probe the registered remote"
    );
    assert_eq!(hosts[0]["host_id"], "local");
}
