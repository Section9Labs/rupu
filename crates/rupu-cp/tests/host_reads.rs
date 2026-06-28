//! Task 3: host-aware agent + autoflow run lists (fan-out).
//!
//! Tests:
//! - `GET /api/runs/agents` fan-out merges local + remote, each tagged `host_id`.
//! - An offline host contributes nothing without failing the endpoint.
//! - `?host=local` returns only local runs, tagged `host_id=local`.
//! - `GET /api/runs/autoflows` fan-out merges local + remote cycles.
//! - `GET /api/runs/autoflows/events` fan-out merges local + remote events.

use chrono::Utc;
use reqwest::StatusCode;
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
    AutoflowHistoryStore,
};

// ── Spawn helpers ─────────────────────────────────────────────────────────────

/// Spin up a server with one remote host pre-registered via the registry.
async fn spawn_server_with_remote(
    dir: &std::path::Path,
    mock_base_url: &str,
) -> (std::net::SocketAddr, String) {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let host = state
        .hosts
        .add_host("mock-remote", mock_base_url, None)
        .expect("add_host should succeed");
    let host_id = host.id.clone();
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, host_id)
}

/// Spin up a read-only local-only server.
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

// ── Data seeders ──────────────────────────────────────────────────────────────

fn seed_standalone_meta(global_dir: &std::path::Path, run_id: &str) {
    let transcripts = global_dir.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    let meta = serde_json::json!({
        "run_id": run_id,
        "session_id": null,
        "trigger_source": "run_cli"
    });
    std::fs::write(
        transcripts.join(format!("{run_id}.meta.json")),
        serde_json::to_string(&meta).unwrap(),
    )
    .unwrap();
}

fn seed_autoflow_cycle(global_dir: &std::path::Path) -> String {
    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);
    let now = Utc::now();
    let mut cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, now);
    cycle.finished_at = now.to_rfc3339();
    store.save(&cycle).unwrap();
    cycle.cycle_id
}

/// Seeds a cycle that contains a `RunLaunched` event.  Returns the cycle_id.
fn seed_autoflow_event(global_dir: &std::path::Path) -> String {
    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);
    let now = Utc::now();
    let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, now);
    store
        .append_cycle_event(
            &cycle,
            AutoflowCycleEvent {
                kind: AutoflowCycleEventKind::RunLaunched,
                workflow: Some("triage-wf".into()),
                run_id: Some("local_af_event_run_01".into()),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    cycle.cycle_id
}

// ── Agent run fan-out tests ───────────────────────────────────────────────────

/// Fan-out across local + remote: both rows appear, each tagged with `host_id`.
#[tokio::test]
async fn agent_list_fan_out_merges_local_and_remote() {
    let tmp = tempfile::tempdir().unwrap();
    seed_standalone_meta(tmp.path(), "local_agent_r1");

    let remote_row = serde_json::json!({
        "run_id": "remote_agent_r1",
        "source": "standalone",
        "agent": null,
        "session_id": null,
        "trigger_source": "run_cli",
        "status": null,
        "started_at": "2026-06-01T10:00:00Z",
        "transcript_path": null,
        "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0},
        "turns": 0,
        "duration_ms": null
    });
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/runs/agents");
        then.status(200).json_body(serde_json::json!([remote_row]));
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["run_id"].as_str()).collect();
    assert!(
        ids.contains(&"local_agent_r1"),
        "local run should appear; got {ids:?}"
    );
    assert!(
        ids.contains(&"remote_agent_r1"),
        "remote run should appear; got {ids:?}"
    );

    // Every row must carry host_id.
    for row in &body {
        assert!(
            row.get("host_id").is_some(),
            "every row must have host_id; missing in: {row}"
        );
    }

    let local_row = body.iter().find(|r| r["run_id"] == "local_agent_r1").unwrap();
    assert_eq!(local_row["host_id"], "local");

    let remote_row_found = body.iter().find(|r| r["run_id"] == "remote_agent_r1").unwrap();
    assert_eq!(remote_row_found["host_id"], host_id);
}

/// An offline host contributes nothing; the endpoint still returns 200.
#[tokio::test]
async fn agent_list_fan_out_offline_host_no_fail() {
    let tmp = tempfile::tempdir().unwrap();
    seed_standalone_meta(tmp.path(), "local_agent_offline_r1");

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

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "offline host must not cause list to fail"
    );

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["run_id"].as_str()).collect();
    assert!(
        ids.contains(&"local_agent_offline_r1"),
        "local run should still appear despite offline remote; got {ids:?}"
    );
}

/// `?host=local` returns only local runs, tagged `host_id=local`; does NOT
/// proxy to the remote (even when one is registered).
#[tokio::test]
async fn agent_list_host_local_returns_only_local_tagged() {
    let tmp = tempfile::tempdir().unwrap();
    seed_standalone_meta(tmp.path(), "local_scoped_agent_r1");

    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/runs/agents");
        then.status(200).json_body(serde_json::json!([{
            "run_id": "remote_should_not_appear",
            "source": "standalone",
            "started_at": "2026-06-01T10:00:00Z"
        }]));
    });

    let (addr, _host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents?host=local"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["run_id"].as_str()).collect();
    assert!(
        ids.contains(&"local_scoped_agent_r1"),
        "local run should appear with ?host=local; got {ids:?}"
    );
    assert!(
        !ids.contains(&"remote_should_not_appear"),
        "remote run must NOT appear with ?host=local; got {ids:?}"
    );

    let row = body
        .iter()
        .find(|r| r["run_id"] == "local_scoped_agent_r1")
        .unwrap();
    assert_eq!(row["host_id"], "local", "local row must be tagged host_id=local");
}

/// Single local server (no remotes): fan-out tags the row `host_id=local`.
#[tokio::test]
async fn agent_list_single_host_tagged_local() {
    let tmp = tempfile::tempdir().unwrap();
    seed_standalone_meta(tmp.path(), "local_tagged_agent_r1");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let row = body
        .iter()
        .find(|r| r["run_id"] == "local_tagged_agent_r1")
        .expect("seeded run must appear");
    assert_eq!(row["host_id"], "local");
}

// ── Autoflow cycle fan-out tests ──────────────────────────────────────────────

/// Fan-out across local + remote: both cycles appear, each tagged `host_id`.
#[tokio::test]
async fn autoflow_cycles_fan_out_merges_local_and_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let local_cycle_id = seed_autoflow_cycle(tmp.path());

    let remote_row = serde_json::json!({
        "cycle_id": "remote_cycle_01",
        "mode": "tick",
        "worker_name": null,
        "started_at": "2026-06-01T09:00:00Z",
        "finished_at": "2026-06-01T09:01:00Z",
        "workflow_count": 2,
        "ran_cycles": 2,
        "skipped_cycles": 0,
        "failed_cycles": 0,
        "run_ids": [],
        "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}
    });
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/runs/autoflows");
        then.status(200).json_body(serde_json::json!([remote_row]));
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let cycle_ids: Vec<&str> = body.iter().filter_map(|r| r["cycle_id"].as_str()).collect();
    assert!(
        cycle_ids.contains(&local_cycle_id.as_str()),
        "local cycle should appear; got {cycle_ids:?}"
    );
    assert!(
        cycle_ids.contains(&"remote_cycle_01"),
        "remote cycle should appear; got {cycle_ids:?}"
    );

    for row in &body {
        assert!(
            row.get("host_id").is_some(),
            "every row must have host_id; missing in: {row}"
        );
    }

    let local_row = body
        .iter()
        .find(|r| r["cycle_id"].as_str() == Some(local_cycle_id.as_str()))
        .unwrap();
    assert_eq!(local_row["host_id"], "local");

    let remote_row_found = body.iter().find(|r| r["cycle_id"] == "remote_cycle_01").unwrap();
    assert_eq!(remote_row_found["host_id"], host_id);
}

/// `?host=local` for autoflows returns only local cycles, tagged.
#[tokio::test]
async fn autoflow_cycles_host_local_returns_only_local() {
    let tmp = tempfile::tempdir().unwrap();
    let local_cycle_id = seed_autoflow_cycle(tmp.path());

    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/runs/autoflows");
        then.status(200).json_body(serde_json::json!([{
            "cycle_id": "remote_cycle_should_not_appear",
            "mode": "tick",
            "started_at": "2026-06-01T09:00:00Z",
            "finished_at": "2026-06-01T09:01:00Z"
        }]));
    });

    let (addr, _) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows?host=local"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["cycle_id"].as_str()).collect();
    assert!(
        ids.contains(&local_cycle_id.as_str()),
        "local cycle must appear; got {ids:?}"
    );
    assert!(
        !ids.contains(&"remote_cycle_should_not_appear"),
        "remote cycle must NOT appear with ?host=local; got {ids:?}"
    );

    let row = body
        .iter()
        .find(|r| r["cycle_id"].as_str() == Some(local_cycle_id.as_str()))
        .unwrap();
    assert_eq!(row["host_id"], "local");
}

// ── Autoflow event fan-out tests ──────────────────────────────────────────────

/// Fan-out across local + remote: both events appear, each tagged `host_id`.
#[tokio::test]
async fn autoflow_events_fan_out_merges_local_and_remote() {
    let tmp = tempfile::tempdir().unwrap();
    seed_autoflow_event(tmp.path());

    let remote_row = serde_json::json!({
        "event_id": "remote_event_01",
        "cycle_id": "remote_cycle_01",
        "at": "2026-06-01T09:00:00Z",
        "kind": "run_launched",
        "workflow": "some-wf",
        "issue_display_ref": null,
        "run_id": "remote_event_run_01",
        "status": null,
        "worker_name": null,
        "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}
    });
    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/runs/autoflows/events");
        then.status(200).json_body(serde_json::json!([remote_row]));
    });

    let (addr, host_id) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows/events"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Local event should appear (tagged local)
    let has_local = body.iter().any(|r| r["host_id"] == "local");
    assert!(has_local, "local events should be tagged host_id=local; body: {body:?}");

    // Remote event should appear
    let remote_ev = body.iter().find(|r| r["event_id"] == "remote_event_01");
    assert!(
        remote_ev.is_some(),
        "remote event should appear; body: {body:?}"
    );
    assert_eq!(remote_ev.unwrap()["host_id"], host_id);

    for row in &body {
        assert!(
            row.get("host_id").is_some(),
            "every row must have host_id; missing in: {row}"
        );
    }
}

/// `?host=local` for autoflow events returns only local events, tagged.
#[tokio::test]
async fn autoflow_events_host_local_returns_only_local() {
    let tmp = tempfile::tempdir().unwrap();
    seed_autoflow_event(tmp.path());

    let mock = httpmock::MockServer::start_async().await;
    let _m = mock.mock(|when, then| {
        when.method("GET").path("/api/runs/autoflows/events");
        then.status(200).json_body(serde_json::json!([{
            "event_id": "remote_event_should_not_appear",
            "cycle_id": "remote_cycle_01",
            "at": "2026-06-01T09:00:00Z",
            "kind": "run_launched"
        }]));
    });

    let (addr, _) = spawn_server_with_remote(tmp.path(), &mock.base_url()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/autoflows/events?host=local"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    // All rows should be local
    assert!(!body.is_empty(), "local events should appear");
    for row in &body {
        assert_eq!(row["host_id"], "local", "all rows must be tagged local");
    }

    let ids: Vec<&str> = body.iter().filter_map(|r| r["event_id"].as_str()).collect();
    assert!(
        !ids.contains(&"remote_event_should_not_appear"),
        "remote event must NOT appear with ?host=local; got {ids:?}"
    );
}
