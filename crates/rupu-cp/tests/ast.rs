//! Endpoint-level tests for `GET /api/runs/:id/ast`.

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
async fn get_ast_returns_subtree_for_a_rust_file() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let contents = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    std::fs::write(workspace.path().join("x.rs"), contents).unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_ast_1", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_ast_1/ast"))
        .query(&[("path", "x.rs"), ("line", "1"), ("col", "4")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available"], true);
    assert_eq!(body["language"], "rust");
    let root = &body["root"];
    let kind = root["kind"].as_str().unwrap();
    assert!(!kind.is_empty());

    // Some node in the tree is flagged matched: true.
    fn any_matched(n: &serde_json::Value) -> bool {
        if n["matched"] == true {
            return true;
        }
        n["children"]
            .as_array()
            .map(|cs| cs.iter().any(any_matched))
            .unwrap_or(false)
    }
    assert!(any_matched(root), "expected some node with matched: true");
}

#[tokio::test]
async fn get_ast_rejects_path_traversal_with_400() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("x.rs"), "fn main() {}\n").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_ast_2", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_ast_2/ast"))
        .query(&[("path", "../../etc/passwd")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_ast_soft_fails_for_unsupported_extension() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("x.unknownext"), "whatever").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_ast_3", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_ast_3/ast"))
        .query(&[("path", "x.unknownext"), ("line", "1"), ("col", "1")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["available"], false);
    assert!(body["reason"].as_str().unwrap().contains("grammar"));
}

#[tokio::test]
async fn get_ast_soft_fails_for_remote_host_query() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("x.rs"), "fn main() {}\n").unwrap();

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run_ast_4", workspace.path()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_ast_4/ast"))
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
async fn get_ast_returns_404_for_unknown_run() {
    use axum::http::StatusCode;

    let global = tempfile::tempdir().unwrap();
    let addr = spawn_server(global.path()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/api/runs/run_does_not_exist/ast"))
        .query(&[("path", "x.rs")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
