/// `AppState::new` must expose a host registry whose `list_hosts()` returns
/// exactly the built-in `local` host. No remote hosts should be present in a
/// freshly-created state against an empty temp directory.
#[test]
fn app_state_hosts_contains_local_only() {
    let dir = tempfile::tempdir().unwrap();
    let state = rupu_cp::state::AppState::new(
        dir.path().into(),
        rupu_config::PricingConfig::default(),
    );
    let hosts = state.hosts.list_hosts();
    assert_eq!(hosts.len(), 1, "expected exactly 1 host, got {hosts:?}");
    assert_eq!(hosts[0].id, "local");
}

/// Confirm the Axum router still builds when `AppState` carries the new
/// `hosts` field — regression guard for the router construction path.
#[tokio::test]
async fn router_builds_with_hosts_field() {
    let dir = tempfile::tempdir().unwrap();
    let state = rupu_cp::state::AppState::new(
        dir.path().into(),
        rupu_config::PricingConfig::default(),
    );
    // Just build the router — no request needed; this would panic/fail to
    // compile if AppState construction or router wiring were broken.
    let _app = rupu_cp::server::router(state, None);
}

#[tokio::test]
async fn healthz_ok() {
    let dir = tempfile::tempdir().unwrap();
    let state =
        rupu_cp::state::AppState::new(dir.path().into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let body = reqwest::get(format!("http://{addr}/healthz"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
}
