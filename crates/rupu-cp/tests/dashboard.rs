/// Integration tests for `GET /api/dashboard`.
use chrono::Utc;
use rupu_config::PricingConfig;
use rupu_coverage::CoveragePaths;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use rupu_runtime::{WorkerCapabilities, WorkerKind, WorkerRecord};
use rupu_workspace::worker_store::WorkerStore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Seeding helpers (mirrors the patterns in endpoints.rs and runs.rs)
// ---------------------------------------------------------------------------

fn seed_run(id: &str, status: RunStatus, started_offset_secs: i64) -> RunRecord {
    let started_at = Utc::now() - chrono::Duration::seconds(started_offset_secs);
    let finished_at = if status.is_terminal() {
        Some(started_at + chrono::Duration::seconds(1))
    } else {
        None
    };
    RunRecord {
        id: id.into(),
        workflow_name: "dashboard-wf".into(),
        status,
        inputs: BTreeMap::from([("prompt".into(), "hello".into())]),
        event: None,
        workspace_id: "ws_test".into(),
        workspace_path: PathBuf::from("/tmp/test-proj"),
        transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
        started_at,
        finished_at,
        error_message: None,
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
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

fn seed_worker(store: &WorkerStore, id: &str) {
    let worker = WorkerRecord {
        version: WorkerRecord::VERSION,
        worker_id: id.to_string(),
        kind: WorkerKind::Cli,
        name: "test-worker".to_string(),
        host: "localhost".to_string(),
        capabilities: WorkerCapabilities::default(),
        registered_at: "2026-06-16T00:00:00Z".to_string(),
        last_seen_at: "2026-06-16T01:00:00Z".to_string(),
    };
    store.save(&worker).unwrap();
}

fn seed_coverage_target(workspace: &Path, target_id: &str, assertion_lines: usize) {
    let paths = CoveragePaths::new(workspace, target_id);
    paths.ensure_dir().unwrap();
    let line = serde_json::json!({
        "concern_id": "stride:spoofing",
        "file_path": "src/auth.rs",
        "status": "clean",
        "evidence": { "summary": "ok", "line_ranges": [], "finding_ids": [] },
        "run_id": "r1",
        "model": "m",
        "surface": "workflow",
        "declared_at": "2026-06-16T00:00:00Z"
    })
    .to_string();
    let content = std::iter::repeat(format!("{line}\n"))
        .take(assertion_lines)
        .collect::<String>();
    std::fs::write(&paths.concerns, content).unwrap();
}

fn minimal_session_json(id: &str) -> String {
    serde_json::json!({
        "session_id": id,
        "agent_name": "foo",
        "model": "claude-sonnet-4-6",
        "status": "active",
        "total_turns": 1,
        "created_at": "2026-06-16T00:00:00Z",
        "updated_at": "2026-06-16T01:00:00Z",
        "active_run_id": null,
        "target": null,
    })
    .to_string()
}

async fn spawn_server(
    global: &Path,
    workspace: &Path,
) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(global.into(), PricingConfig::default())
        .with_workspace_dir(workspace.into());
    let app = rupu_cp::server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Seed: 1 Running run + 1 Completed run, 1 active session, 1 worker,
/// 1 coverage target with 3 assertion lines.
/// Assert all dashboard fields are correctly aggregated.
#[tokio::test]
async fn dashboard_aggregate_correct_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let workspace = tmp.path();

    // -- runs
    let run_store = RunStore::new(global.join("runs"));
    let run1 = seed_run("dash_run_01", RunStatus::Running, 100);
    let run2 = seed_run("dash_run_02", RunStatus::Completed, 200);
    // run2 is older (started_offset 200s ago) → should be second in recent_runs
    run_store
        .create(run1, "name: dashboard-wf\nsteps: []\n")
        .unwrap();
    run_store
        .create(run2, "name: dashboard-wf\nsteps: []\n")
        .unwrap();

    // -- active session
    let sess_dir = global.join("sessions").join("dash_sess_1");
    std::fs::create_dir_all(&sess_dir).unwrap();
    std::fs::write(
        sess_dir.join("session.json"),
        minimal_session_json("dash_sess_1"),
    )
    .unwrap();

    // -- worker
    let worker_store = WorkerStore {
        root: global.join("autoflows").join("workers"),
    };
    seed_worker(&worker_store, "dash_worker_1");

    // -- coverage (1 target, 3 assertion lines)
    seed_coverage_target(workspace, "dash_target_1", 3);

    let addr = spawn_server(global, workspace).await;

    let resp = reqwest::get(format!("http://{addr}/api/dashboard"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 from /api/dashboard");

    let body: serde_json::Value = resp.json().await.unwrap();

    // runs
    let runs = &body["runs"];
    assert_eq!(
        runs["total"].as_u64(),
        Some(2),
        "runs.total should be 2; body={body}"
    );
    assert_eq!(
        runs["by_status"]["running"].as_u64(),
        Some(1),
        "by_status.running should be 1"
    );
    assert_eq!(
        runs["by_status"]["completed"].as_u64(),
        Some(1),
        "by_status.completed should be 1"
    );
    // All six status keys must be present even when zero
    for key in &["failed", "awaiting_approval", "pending", "rejected"] {
        assert!(
            runs["by_status"].get(key).is_some(),
            "by_status missing key '{key}'"
        );
        assert_eq!(
            runs["by_status"][key].as_u64(),
            Some(0),
            "by_status.{key} should be 0"
        );
    }

    // recent_runs — both runs returned, newest first
    let recent = body["recent_runs"].as_array().expect("recent_runs should be array");
    assert_eq!(recent.len(), 2, "expected 2 recent_runs");
    // run1 started 100s ago → more recent → should be first
    assert_eq!(
        recent[0]["id"].as_str(),
        Some("dash_run_01"),
        "newest run should be first; got {:?}",
        recent[0]["id"]
    );
    assert_eq!(
        recent[1]["id"].as_str(),
        Some("dash_run_02"),
        "older run should be second"
    );
    // Each recent run has the required fields
    assert!(recent[0]["workflow_name"].as_str().is_some());
    assert!(recent[0]["status"].as_str().is_some());
    assert!(recent[0]["started_at"].as_str().is_some());

    // sessions
    let sessions = &body["sessions"];
    assert!(
        sessions["total"].as_u64().unwrap_or(0) >= 1,
        "sessions.total should be >= 1"
    );
    assert_eq!(sessions["active"].as_u64(), Some(1), "active sessions should be 1");
    assert_eq!(sessions["archived"].as_u64(), Some(0), "archived sessions should be 0");

    // workers
    let workers = &body["workers"];
    assert_eq!(workers["total"].as_u64(), Some(1), "workers.total should be 1");

    // coverage
    let cov = &body["coverage"];
    assert_eq!(cov["targets"].as_u64(), Some(1), "coverage.targets should be 1");
    assert_eq!(
        cov["assertions"].as_u64(),
        Some(3),
        "coverage.assertions should be 3"
    );
}

/// Empty state: no runs / no sessions / no workers / no coverage.
/// Dashboard must still return 200 with zero counts.
#[tokio::test]
async fn dashboard_empty_state_returns_zeros() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path(), tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/dashboard"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["runs"]["total"].as_u64(), Some(0));
    assert!(
        body["recent_runs"].as_array().map_or(false, |a| a.is_empty()),
        "recent_runs should be empty"
    );
    assert_eq!(body["sessions"]["total"].as_u64(), Some(0));
    assert_eq!(body["workers"]["total"].as_u64(), Some(0));
    assert_eq!(body["coverage"]["targets"].as_u64(), Some(0));
    assert_eq!(body["coverage"]["assertions"].as_u64(), Some(0));
}

/// recent_runs capped at 10 even when more runs exist.
#[tokio::test]
async fn dashboard_recent_runs_capped_at_10() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();

    let run_store = RunStore::new(global.join("runs"));
    for i in 0..15_u64 {
        let run = seed_run(&format!("dash_cap_{i:02}"), RunStatus::Completed, i as i64 * 10);
        run_store
            .create(run, "name: dashboard-wf\nsteps: []\n")
            .unwrap();
    }

    let addr = spawn_server(global, global).await;

    let resp = reqwest::get(format!("http://{addr}/api/dashboard"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["runs"]["total"].as_u64(), Some(15), "runs.total should be 15");
    let recent = body["recent_runs"].as_array().expect("recent_runs array");
    assert_eq!(recent.len(), 10, "recent_runs should be capped at 10");
}
