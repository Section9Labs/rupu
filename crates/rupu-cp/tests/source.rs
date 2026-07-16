//! Endpoint-level tests for `GET /api/runs/:id/source`.

use chrono::Utc;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use std::collections::BTreeMap;

/// Build a minimal valid `RunRecord` whose `workspace_path` points at
/// `workspace`.
fn seed_run(id: &str, workspace: &std::path::Path) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "test-workflow".into(),
        status: RunStatus::Completed,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_test".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_dir: workspace.join(".rupu").join("transcripts"),
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
        final_output: None,
    }
}

async fn spawn_server(global_dir: &std::path::Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(global_dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn get_source_returns_windowed_slice_for_a_known_file() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();

    // A 5-line file; requesting line=2, context=1 should yield lines 1..=3.
    let contents = "line1\nline2\nline3\nline4\nline5\n";
    std::fs::write(workspace.path().join("x.rs"), contents).unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_src_1", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_src_1/source"))
        .query(&[("path", "x.rs"), ("line", "2"), ("context", "1")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available"], true);
    assert_eq!(body["language"], "rust");
    assert_eq!(body["startLine"], 1);
    assert_eq!(body["endLine"], 3);
    assert_eq!(body["targetLine"], 2);
    assert_eq!(body["totalLines"], 5);
    let lines = body["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["n"], 1);
    assert_eq!(lines[0]["text"], "line1");
    assert_eq!(lines[1]["text"], "line2");
    assert_eq!(lines[2]["text"], "line3");
}

#[tokio::test]
async fn get_source_rejects_path_traversal_with_400() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("x.rs"), "fn main() {}\n").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_src_2", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_src_2/source"))
        .query(&[("path", "../../etc/passwd")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_source_rejects_absolute_path_with_400() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("x.rs"), "fn main() {}\n").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_src_3", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_src_3/source"))
        .query(&[("path", "/etc/passwd")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_source_soft_fails_for_remote_host_query() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("x.rs"), "fn main() {}\n").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_src_4", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_src_4/source"))
        .query(&[("path", "x.rs"), ("host", "some-remote")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available"], false);
    assert!(body["reason"]
        .as_str()
        .unwrap()
        .contains("remote-host runs"));
}

#[tokio::test]
async fn get_source_returns_404_for_unknown_run() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_does_not_exist/source"))
        .query(&[("path", "x.rs")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_source_soft_fails_for_oversize_file() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    // Just over the 2 MiB preview limit.
    let big = "a".repeat(2 * 1024 * 1024 + 1);
    std::fs::write(workspace.path().join("big.rs"), big).unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_src_5", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_src_5/source"))
        .query(&[("path", "big.rs")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available"], false);
    assert_eq!(body["reason"], "File too large to preview");
}

#[tokio::test]
async fn get_source_handles_empty_file_without_panicking() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    // A genuinely empty (0-byte) file — common in real repos (.gitkeep, empty
    // __init__.py, stub modules) and reachable by any valid in-workspace path.
    std::fs::write(workspace.path().join("empty.rs"), "").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_src_6", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_src_6/source"))
        .query(&[("path", "empty.rs"), ("line", "1")])
        .send()
        .await
        .unwrap();

    // Must not panic (a 500/connection-reset would be the symptom) — a valid
    // 200 with a well-formed, zero-line slice.
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available"], true);
    assert_eq!(body["totalLines"], 0);
    assert_eq!(body["lines"].as_array().unwrap().len(), 0);
}
