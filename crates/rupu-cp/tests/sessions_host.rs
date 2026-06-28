//! Task 4: host-aware sessions (list fan-out + detail/runs/usage proxy).
//!
//! Tests:
//! - `GET /api/sessions` (no ?host) fans out across local + remote host.
//!   Every row is tagged `host_id`.
//! - `GET /api/sessions?host=all` — same as above, explicit.
//! - `GET /api/sessions?host=local` — local only, tagged `host_id=local`.
//! - An offline host in the list contributes nothing (tolerated, no 500).
//! - `GET /api/sessions/:id?host=<remote>` — proxies to the owning host.
//! - `GET /api/sessions/:id/runs?host=<remote>` — proxies.
//! - `GET /api/sessions/:id/usage-timeline?host=<remote>` — proxies.

use reqwest::StatusCode;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Write a minimal `session.json` under `<global>/sessions/<id>/`.
fn seed_session(global: &std::path::Path, id: &str, updated_at: &str) {
    let dir = global.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let payload = serde_json::json!({
        "session_id": id,
        "agent_name": "test-agent",
        "model": "claude-sonnet-4-6",
        "provider_name": "anthropic",
        "status": "active",
        "total_turns": 1,
        "total_tokens_in": 0,
        "total_tokens_out": 0,
        "total_tokens_cached": 0,
        "created_at": updated_at,
        "updated_at": updated_at,
        "workspace_id": "ws1"
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_string(&payload).unwrap(),
    )
    .unwrap();
}

/// Spin up a CP server at an ephemeral port.
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

/// Spin up a CP server that also knows about one remote host.
async fn spawn_server_with_remote(
    dir: &std::path::Path,
    mock_base_url: &str,
) -> (std::net::SocketAddr, String) {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let host = state
        .hosts
        .add_host("mock-remote", mock_base_url, None)
        .expect("add_host");
    let host_id = host.id.clone();
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, host_id)
}

// ── List fan-out tests ────────────────────────────────────────────────────────

/// Fan-out: both local + remote session rows appear, each tagged `host_id`.
#[tokio::test]
async fn session_list_fan_out_merges_local_and_remote() {
    let tmp = tempfile::tempdir().unwrap();
    seed_session(tmp.path(), "local_sess_01", "2026-06-27T10:00:00Z");

    let remote_row = serde_json::json!({
        "session_id": "remote_sess_01",
        "agent_name": "remote-agent",
        "model": "claude-sonnet-4-6",
        "provider_name": "anthropic",
        "status": "active",
        "total_turns": 2,
        "total_tokens_in": 0,
        "total_tokens_out": 0,
        "total_tokens_cached": 0,
        "created_at": "2026-06-27T09:00:00Z",
        "updated_at": "2026-06-27T09:00:00Z",
        "scope": "active",
        "workspace_id": "ws2"
    });
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET")
            .path("/api/sessions")
            .query_param("host", "local");
        then.status(200).json_body(serde_json::json!([remote_row]));
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["session_id"].as_str()).collect();

    assert!(
        ids.contains(&"local_sess_01"),
        "local session should appear; got {ids:?}"
    );
    assert!(
        ids.contains(&"remote_sess_01"),
        "remote session should appear; got {ids:?}"
    );

    // Every row must have host_id.
    for row in &body {
        assert!(
            row.get("host_id").is_some(),
            "every row must have host_id; missing in: {row}"
        );
    }

    let local_row = body.iter().find(|r| r["session_id"] == "local_sess_01").unwrap();
    assert_eq!(local_row["host_id"], "local");

    let remote_found = body.iter().find(|r| r["session_id"] == "remote_sess_01").unwrap();
    assert_eq!(remote_found["host_id"], host_id);
}

/// Explicit `?host=all` behaves identically to absent.
#[tokio::test]
async fn session_list_host_all_fans_out() {
    let tmp = tempfile::tempdir().unwrap();
    seed_session(tmp.path(), "local_sess_all_01", "2026-06-27T10:00:00Z");

    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET")
            .path("/api/sessions")
            .query_param("host", "local");
        then.status(200).json_body(serde_json::json!([{
            "session_id": "remote_sess_all_01",
            "agent_name": "a",
            "status": "active",
            "scope": "active",
            "updated_at": "2026-06-27T08:00:00Z",
            "workspace_id": "w"
        }]));
    });

    let (addr, _) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions?host=all"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["session_id"].as_str()).collect();
    assert!(ids.contains(&"local_sess_all_01"), "local missing; got {ids:?}");
    assert!(ids.contains(&"remote_sess_all_01"), "remote missing; got {ids:?}");
}

/// `?host=local` returns only local sessions, tagged `host_id=local`.
/// Remote mock must NOT be called.
#[tokio::test]
async fn session_list_host_local_only() {
    let tmp = tempfile::tempdir().unwrap();
    seed_session(tmp.path(), "local_only_sess_01", "2026-06-27T10:00:00Z");

    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/sessions");
        then.status(200).json_body(serde_json::json!([{
            "session_id": "should_not_appear",
            "scope": "active"
        }]));
    });

    let (addr, _) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/sessions?host=local"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["session_id"].as_str()).collect();
    assert!(ids.contains(&"local_only_sess_01"), "local missing; got {ids:?}");
    assert!(
        !ids.contains(&"should_not_appear"),
        "remote session must not appear with ?host=local; got {ids:?}"
    );

    let row = body.iter().find(|r| r["session_id"] == "local_only_sess_01").unwrap();
    assert_eq!(row["host_id"], "local");
}

/// Offline host contributes nothing; list endpoint still returns 200 with
/// local sessions intact.
#[tokio::test]
async fn session_list_offline_host_tolerated() {
    let tmp = tempfile::tempdir().unwrap();
    seed_session(tmp.path(), "local_offline_sess_01", "2026-06-27T10:00:00Z");

    let state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default());
    state
        .hosts
        .add_host("offline-host", "http://127.0.0.1:1", None)
        .unwrap();
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let resp = reqwest::get(format!("http://{addr}/api/sessions"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "offline host must not 500");

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["session_id"].as_str()).collect();
    assert!(
        ids.contains(&"local_offline_sess_01"),
        "local session must still appear; got {ids:?}"
    );
}

// ── Single-session detail/runs/usage proxy tests ──────────────────────────────

/// `GET /api/sessions/:id?host=<remote>` proxies to the remote host.
#[tokio::test]
async fn get_session_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let remote_response = serde_json::json!({
        "session_id": "remote_detail_sess_01",
        "agent_name": "remote-agent",
        "status": "active",
        "scope": "active"
    });
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/sessions/remote_detail_sess_01");
        then.status(200).json_body(remote_response.clone());
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/sessions/remote_detail_sess_01?host={host_id}"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["session_id"], "remote_detail_sess_01");
    assert_eq!(body["agent_name"], "remote-agent");
}

/// `GET /api/sessions/:id/runs?host=<remote>` proxies.
#[tokio::test]
async fn get_session_runs_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let remote_rows = serde_json::json!([{
        "run_id": "remote_run_01",
        "prompt": "remote prompt",
        "transcript_path": "/tmp/remote.jsonl",
        "status": "ok",
        "tokens_in": 0,
        "tokens_out": 0,
        "tokens_cached": 0,
        "duration_ms": 0
    }]);
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/sessions/remote_runs_sess_01/runs");
        then.status(200).json_body(remote_rows.clone());
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/sessions/remote_runs_sess_01/runs?host={host_id}"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["run_id"], "remote_run_01");
}

/// `GET /api/sessions/:id/usage-timeline?host=<remote>` proxies.
#[tokio::test]
async fn get_session_usage_timeline_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let remote_timeline = serde_json::json!([{
        "run_id": "remote_run_01",
        "turn": 1,
        "input_tokens": 10,
        "output_tokens": 20
    }]);
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET")
            .path("/api/sessions/remote_timeline_sess_01/usage-timeline");
        then.status(200).json_body(remote_timeline.clone());
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/sessions/remote_timeline_sess_01/usage-timeline?host={host_id}"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("should be array");
    assert_eq!(arr[0]["run_id"], "remote_run_01");
}

/// Unknown `?host=` value → 404 (not a 500).
#[tokio::test]
async fn get_session_unknown_host_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/sessions/any_sess?host=no-such-host"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `?host=local` for detail endpoint still returns local session (existing
/// behaviour, regression guard).
#[tokio::test]
async fn get_session_host_local_returns_local() {
    let tmp = tempfile::tempdir().unwrap();
    seed_session(tmp.path(), "local_detail_sess_01", "2026-06-27T10:00:00Z");
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/sessions/local_detail_sess_01?host=local"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["session_id"], "local_detail_sess_01");
}
