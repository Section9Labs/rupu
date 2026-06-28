//! Integration tests for Task 8: host-aware run observation.
//!
//! Covers:
//! - `GET /api/runs` fan-out: merges local + remote runs, each row tagged with
//!   `host_id`; an offline host contributes nothing without failing the list.
//! - `GET /api/runs/:id?host=<id>` proxies the detail request to the mock.
//! - `GET /api/events/stream?run=<id>&host=<id>` proxies SSE frames from the
//!   remote host.
//! - `GET /api/transcript?path=<p>&host=<id>` proxies transcript to the mock.
//! - Unknown `?host=` values → 404.

use chrono::Utc;
use reqwest::StatusCode;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::AsyncBufReadExt as _;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_run(id: &str) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "obs-wf".into(),
        status: RunStatus::Completed,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_obs".into(),
        workspace_path: PathBuf::from("/tmp/obs-proj"),
        transcript_dir: PathBuf::from("/tmp/obs-proj/.rupu/transcripts"),
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
        backend_id: None,
        worker_id: None,
        artifact_manifest_path: None,
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
    }
}

/// Spawn a read-only server that has one extra remote host pre-registered.
///
/// Returns `(server_addr, host_id)`. No launcher is installed — write-path
/// operations return 501; run-observation endpoints (the focus of this test
/// module) work fine because they only need the registry + run store.
async fn spawn_server_with_remote(
    dir: &std::path::Path,
    mock_base_url: &str,
) -> (std::net::SocketAddr, String) {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    // Add the remote host directly via the registry (bypasses the need for a
    // launcher; no token → keychain is not touched).
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

/// Spawn a read-only server. Used for single-host tests.
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

// ── Fan-out tests ─────────────────────────────────────────────────────────────

/// `GET /api/runs` fans out across local + remote: every row is tagged with
/// `host_id`, and both the local and the remote run appear in the merged list.
#[tokio::test]
async fn run_list_fan_out_merges_local_and_remote() {
    let tmp = tempfile::tempdir().unwrap();

    // Seed one local run.
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(make_run("local_obs_r1"), "name: obs-wf\nsteps: []\n")
        .unwrap();

    // Start a mock remote that returns one "remote" run.
    let remote_row = serde_json::json!({
        "id": "remote_obs_r1",
        "workflow_name": "obs-wf",
        "status": "completed",
        "started_at": "2026-06-27T10:00:00Z",
        "finished_at": "2026-06-27T10:01:00Z",
        "trigger": "manual",
        "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0},
        "turns": 0,
        "duration_ms": null
    });
    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET").path("/api/runs");
        then.status(200).json_body(serde_json::json!([remote_row]));
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(
        ids.contains(&"local_obs_r1"),
        "local run should be in merged list; got {ids:?}"
    );
    assert!(
        ids.contains(&"remote_obs_r1"),
        "remote run should be in merged list; got {ids:?}"
    );

    // Every row must carry a host_id.
    for row in &body {
        assert!(
            row.get("host_id").is_some(),
            "every row must have host_id; row missing it: {row}"
        );
    }

    // The local row's host_id must be "local"; the remote row's must be our id.
    let local_row = body
        .iter()
        .find(|r| r["id"] == "local_obs_r1")
        .expect("local run must appear");
    assert_eq!(
        local_row["host_id"],
        "local",
        "local run should be tagged host_id=local"
    );

    let remote_row = body
        .iter()
        .find(|r| r["id"] == "remote_obs_r1")
        .expect("remote run must appear");
    assert_eq!(
        remote_row["host_id"], host_id,
        "remote run should be tagged with the remote host id"
    );
}

/// An offline host (connection refused) contributes nothing to the fan-out
/// but does not cause the list endpoint to fail.
#[tokio::test]
async fn run_list_fan_out_offline_host_does_not_fail() {
    let tmp = tempfile::tempdir().unwrap();

    // Seed one local run.
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(make_run("local_offline_r1"), "name: obs-wf\nsteps: []\n")
        .unwrap();

    // Register a host at a port that immediately refuses connections (port 1).
    let state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default());
    state
        .hosts
        .add_host("offline-host", "http://127.0.0.1:1", None)
        .expect("add_host should succeed (write-only, no network yet)");

    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs"))
        .send()
        .await
        .unwrap();

    // Must be 200; the offline host contributes nothing (no error for the caller).
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "offline host must not cause list to fail"
    );
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(
        ids.contains(&"local_offline_r1"),
        "local run should still appear despite offline remote; got {ids:?}"
    );
}

/// `GET /api/runs` with a single-host local returns the same runs as before,
/// now tagged with `host_id: "local"`.
#[tokio::test]
async fn run_list_single_local_host_tagged_with_local() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(make_run("obs_tagged_r1"), "name: obs-wf\nsteps: []\n")
        .unwrap();

    let addr = spawn_server(tmp.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let row = body
        .iter()
        .find(|r| r["id"] == "obs_tagged_r1")
        .expect("seeded run must appear");
    assert_eq!(
        row["host_id"], "local",
        "single-host fan-out should tag row with host_id=local"
    );
}

// ── Single-host ?host= filter ─────────────────────────────────────────────────

/// `GET /api/runs?host=<remote>` returns only that host's runs.
#[tokio::test]
async fn run_list_host_param_scopes_to_one_host() {
    let tmp = tempfile::tempdir().unwrap();

    // Seed a local run.
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(make_run("local_scope_r1"), "name: obs-wf\nsteps: []\n")
        .unwrap();

    // Remote returns a different run.
    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET").path("/api/runs");
        then.status(200).json_body(serde_json::json!([{
            "id": "remote_scope_r1",
            "workflow_name": "obs-wf",
            "status": "completed",
            "started_at": "2026-06-27T09:00:00Z",
            "finished_at": "2026-06-27T09:01:00Z",
            "trigger": "manual",
            "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0},
            "turns": 0,
            "duration_ms": null
        }]));
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs?host={host_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ids: Vec<&str> = body.iter().filter_map(|r| r["id"].as_str()).collect();
    assert!(
        ids.contains(&"remote_scope_r1"),
        "remote run should appear when scoped to that host; got {ids:?}"
    );
    assert!(
        !ids.contains(&"local_scope_r1"),
        "local run must NOT appear when ?host= scopes to remote; got {ids:?}"
    );
}

/// `GET /api/runs?host=bad_host_id` → 404.
#[tokio::test]
async fn run_list_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs?host=host_DOESNOTEXIST"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── GET /api/runs/:id?host= ───────────────────────────────────────────────────

/// `GET /api/runs/:id?host=<remote>` proxies the detail request to the mock
/// and returns its response verbatim.
#[tokio::test]
async fn get_run_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let mock_detail = serde_json::json!({
        "run": {
            "id": "remote_detail_r1",
            "workflow_name": "obs-wf",
            "status": "completed"
        },
        "steps": [],
        "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}
    });

    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET").path("/api/runs/remote_detail_r1");
        then.status(200).json_body(mock_detail.clone());
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://{addr}/api/runs/remote_detail_r1?host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["run"]["id"], "remote_detail_r1",
        "proxied detail should carry the remote run id"
    );
}

/// `GET /api/runs/:id?host=bad_id` → 404.
#[tokio::test]
async fn get_run_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/some_run?host=host_DOESNOTEXIST"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `GET /api/runs/:id` (no `?host=`) still works exactly as before for the
/// local store — no regression.
#[tokio::test]
async fn get_run_local_no_host_param_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    store
        .create(make_run("obs_local_r1"), "name: obs-wf\nsteps: []\n")
        .unwrap();

    let addr = spawn_server(tmp.path()).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/obs_local_r1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["run"]["id"], "obs_local_r1");
}

// ── GET /api/events/stream?host= ─────────────────────────────────────────────

/// `GET /api/events/stream?run=<id>&host=<remote>` proxies SSE frames from
/// the mock host's `/api/events/stream?run=<id>` endpoint.
#[tokio::test]
async fn events_stream_proxies_to_remote_host() {
    use futures_util::TryStreamExt as _;

    let tmp = tempfile::tempdir().unwrap();

    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET")
            .path("/api/events/stream")
            .query_param("run", "remote_sse_r1");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body("data: {\"type\":\"run_started\",\"run_id\":\"remote_sse_r1\"}\n\n");
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://{addr}/api/events/stream?run=remote_sse_r1&host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "proxied stream must be text/event-stream; got {ct:?}"
    );

    // Read the byte stream and look for the forwarded SSE data line.
    let stream = resp.bytes_stream().map_err(std::io::Error::other);
    let async_reader = tokio_util::io::StreamReader::new(stream);
    let mut lines = tokio::io::BufReader::new(async_reader).lines();

    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if v["run_id"] == "remote_sse_r1" {
                        return true;
                    }
                }
            }
        }
        false
    })
    .await
    .expect("timed out waiting for proxied SSE frame");

    assert!(found, "proxied SSE stream should carry the remote run's event");
}

/// `GET /api/events/stream?run=<id>&host=<unknown>` → 404.
#[tokio::test]
async fn events_stream_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/events/stream?run=r1&host=host_DOESNOTEXIST"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── GET /api/transcript?host= ─────────────────────────────────────────────────

/// `GET /api/transcript?path=<p>&host=<remote>` proxies to the remote and
/// returns its response.
#[tokio::test]
async fn get_transcript_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let transcript_body = serde_json::json!({
        "events": [{"type": "assistant_message", "content": "hello"}],
        "summary": null
    });

    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET")
            .path("/api/transcript")
            .query_param("path", "/remote/run.jsonl");
        then.status(200).json_body(transcript_body.clone());
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://{addr}/api/transcript?path=/remote/run.jsonl&host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["events"].is_array(),
        "proxied transcript response should have events array"
    );
}

/// `GET /api/transcript?path=<p>&host=<unknown>` → 404.
#[tokio::test]
async fn get_transcript_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/transcript?path=/some/run.jsonl&host=host_DOESNOTEXIST"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `GET /api/transcript?path=<p>` (no `?host=`) still works as before for
/// the local path — validate+read. Here we expect 400 for a path outside roots,
/// not a 404 or 500.
#[tokio::test]
async fn get_transcript_local_no_host_param_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    // Path outside any allowed root (tmp) → 400 bad request.
    let resp = reqwest::get(format!(
        "http://{addr}/api/transcript?path=/etc/passwd.jsonl"
    ))
    .await
    .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "local path outside allowed roots should be 400"
    );
}

// ── GET /api/runs/:id/log?host= ───────────────────────────────────────────────

/// `GET /api/runs/:id/log?host=<remote>` proxies SSE frames from the mock
/// host's `/api/events/stream?run=<id>` endpoint (that is what
/// `HttpHostConnector::stream_run_events` calls on the remote).
#[tokio::test]
async fn get_run_log_proxies_to_remote_host() {
    use futures_util::TryStreamExt as _;

    let tmp = tempfile::tempdir().unwrap();

    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET")
            .path("/api/events/stream")
            .query_param("run", "remote_log_r1");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(
                "data: {\"type\":\"step_completed\",\"run_id\":\"remote_log_r1\"}\n\n",
            );
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://{addr}/api/runs/remote_log_r1/log?host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "proxied log stream must be text/event-stream; got {ct:?}"
    );

    // Read the byte stream and verify the forwarded SSE data line arrives.
    let stream = resp.bytes_stream().map_err(std::io::Error::other);
    let async_reader = tokio_util::io::StreamReader::new(stream);
    let mut lines = tokio::io::BufReader::new(async_reader).lines();

    let found = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if v["run_id"] == "remote_log_r1" {
                        return true;
                    }
                }
            }
        }
        false
    })
    .await
    .expect("timed out waiting for proxied SSE frame");

    assert!(found, "proxied SSE log stream should carry the remote run's event");
}

/// `GET /api/runs/:id/log?host=<unknown>` → 404.
#[tokio::test]
async fn get_run_log_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/some_run/log?host=host_DOESNOTEXIST"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── GET /api/runs/:id/graph?host= ────────────────────────────────────────────

/// `GET /api/runs/:id/graph?host=<remote>` proxies to the mock host and
/// returns its graph JSON verbatim.
#[tokio::test]
async fn get_run_graph_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let mock_graph = serde_json::json!({
        "run": {"id": "remote_graph_r1", "workflow_name": "g-wf", "status": "completed"},
        "workflow": {"steps": []},
        "step_results": [],
        "units": [],
        "usage": {"priced": false, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0}
    });

    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET").path("/api/runs/remote_graph_r1/graph");
        then.status(200).json_body(mock_graph.clone());
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://{addr}/api/runs/remote_graph_r1/graph?host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["run"]["id"], "remote_graph_r1",
        "proxied graph should carry the remote run id"
    );
    assert!(
        body.get("workflow").is_some(),
        "proxied graph should include workflow field"
    );
}

/// `GET /api/runs/:id/graph?host=<unknown>` → 404.
#[tokio::test]
async fn get_run_graph_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/some_run/graph?host=host_DOESNOTEXIST"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── GET /api/runs/:id/usage-timeline?host= ────────────────────────────────────

/// `GET /api/runs/:id/usage-timeline?host=<remote>` proxies to the mock host
/// and returns its timeline series JSON verbatim.
#[tokio::test]
async fn get_run_usage_timeline_proxies_to_remote_host() {
    let tmp = tempfile::tempdir().unwrap();

    let mock_series = serde_json::json!([
        {"step_id": "s1", "turn": 1, "input_tokens": 100, "output_tokens": 50, "cost_usd": 0.001},
        {"step_id": "s1", "turn": 2, "input_tokens": 200, "output_tokens": 80, "cost_usd": 0.002}
    ]);

    let mock_server = httpmock::MockServer::start_async().await;
    let _m = mock_server.mock(|when, then| {
        when.method("GET")
            .path("/api/runs/remote_usage_r1/usage-timeline");
        then.status(200).json_body(mock_series.clone());
    });

    let (addr, host_id) =
        spawn_server_with_remote(tmp.path(), &mock_server.base_url()).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!(
            "http://{addr}/api/runs/remote_usage_r1/usage-timeline?host={host_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.is_array(),
        "proxied usage-timeline should be a JSON array"
    );
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2, "proxied series should have 2 turn points");
    assert_eq!(
        arr[0]["step_id"], "s1",
        "proxied series should carry step_id"
    );
}

/// `GET /api/runs/:id/usage-timeline?host=<unknown>` → 404.
#[tokio::test]
async fn get_run_usage_timeline_unknown_host_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/some_run/usage-timeline?host=host_DOESNOTEXIST"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
