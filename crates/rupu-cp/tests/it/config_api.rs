//! `PUT /api/config/global`'s `restart_required`: the start-time-only keys
//! whose saved value differs from what the running server started with.

// Throwaway in-process client, not rupu's egress.
#![allow(clippy::disallowed_methods)]

struct NoopLauncher;

#[async_trait::async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_unused".into())
    }
}

async fn put_global(addr: std::net::SocketAddr, raw: &str) -> serde_json::Value {
    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/api/config/global"))
        .json(&serde_json::json!({ "raw": raw }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

#[tokio::test]
async fn restart_required_names_start_time_keys_changed_since_boot() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("config.toml"), "").unwrap();
    let state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
            .with_launcher(Some(std::sync::Arc::new(NoopLauncher)));
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // A live-applied key: nothing to restart for.
    let v = put_global(addr, "[cp]\nmax_workspace_bytes = 2048\n").await;
    assert_eq!(v["restart_required"], serde_json::json!([]));

    // A loop interval is read once, at start.
    let v = put_global(
        addr,
        "[cp]\nmax_workspace_bytes = 2048\ngate_sweep_interval_secs = 5\n",
    )
    .await;
    assert_eq!(
        v["restart_required"],
        serde_json::json!(["cp.gate_sweep_interval_secs"])
    );

    // Still pending after an unrelated edit: the server still runs the old
    // value until it restarts.
    let v = put_global(addr, "[cp]\ngate_sweep_interval_secs = 5\n").await;
    assert_eq!(
        v["restart_required"],
        serde_json::json!(["cp.gate_sweep_interval_secs"])
    );

    // Back to the boot value: nothing pending.
    let v = put_global(addr, "").await;
    assert_eq!(v["restart_required"], serde_json::json!([]));
}
