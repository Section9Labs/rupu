use chrono::Utc;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore, StepKind, StepResultRecord};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Build a minimal valid RunRecord with the given id.
fn seed_run(id: &str) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "test-workflow".into(),
        status: RunStatus::Completed,
        inputs: BTreeMap::from([("prompt".into(), "hello".into())]),
        event: None,
        workspace_id: "ws_test".into(),
        workspace_path: PathBuf::from("/tmp/test-proj"),
        transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
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

/// Build a minimal StepResultRecord for `run_id`.
fn seed_step(run_id: &str, step_id: &str) -> StepResultRecord {
    StepResultRecord {
        step_id: step_id.into(),
        run_id: run_id.into(),
        transcript_path: PathBuf::from(format!("/tmp/{step_id}.jsonl")),
        output: "done".into(),
        success: true,
        skipped: false,
        rendered_prompt: "do the thing".into(),
        kind: StepKind::Linear,
        items: Vec::new(),
        findings: Vec::new(),
        iterations: 0,
        resolved: true,
        finished_at: Utc::now(),
    }
}

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

#[tokio::test]
async fn list_runs_returns_seeded_run() {
    let tmp = tempfile::tempdir().unwrap();

    // Seed via the public RunStore API before the server starts.
    let store = RunStore::new(tmp.path().join("runs"));
    let run_id = "run_test_list_01";
    store
        .create(
            seed_run(run_id),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(!arr.is_empty(), "array should contain at least one run");
    let ids: Vec<&str> = arr
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert!(
        ids.contains(&run_id),
        "seeded run id not found in list; got {ids:?}"
    );
}

#[tokio::test]
async fn get_run_returns_run_and_steps() {
    let tmp = tempfile::tempdir().unwrap();

    let store = RunStore::new(tmp.path().join("runs"));
    let run_id = "run_test_get_01";
    store
        .create(
            seed_run(run_id),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();
    store
        .append_step_result(run_id, &seed_step(run_id, "step-a"))
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["run"]["id"].as_str(),
        Some(run_id),
        "run.id should match seeded id"
    );
    let steps = body["steps"].as_array().expect("steps should be an array");
    assert_eq!(steps.len(), 1, "expected exactly one step result");
    assert_eq!(steps[0]["step_id"].as_str(), Some("step-a"));
}

#[tokio::test]
async fn get_run_not_found_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/does-not-exist"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().is_some(),
        "404 body should have an 'error' field"
    );
}

#[tokio::test]
async fn list_runs_empty_when_no_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(arr.is_empty(), "no runs seeded, array should be empty");
}

// Verify that the AppState wires its RunStore to <global>/runs/, so seeding
// into that sub-dir is visible through the server endpoints.
#[tokio::test]
async fn app_state_run_store_path_matches_seed_location() {
    let tmp = tempfile::tempdir().unwrap();

    // AppState::new wires run_store to <global>/runs/
    let state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default());

    let run_id = "run_path_check_01";
    state
        .run_store
        .create(
            seed_run(run_id),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();

    // Re-open a fresh RunStore at the same path and verify it sees the run.
    let store2 = Arc::new(RunStore::new(tmp.path().join("runs")));
    let listed = store2.list().unwrap();
    let ids: Vec<&str> = listed.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&run_id), "run seeded via AppState not visible from explicit path; got {ids:?}");
}

// ── Trigger-type tests ───────────────────────────────────────────────────────

/// Seed three runs: manual, event-triggered, cron-triggered.
/// Assert GET /api/runs carries the right `trigger` for each.
#[tokio::test]
async fn list_runs_carries_trigger_field() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));

    // manual run — neither event nor source_wake_id
    let manual = seed_run("run_trigger_manual");
    store
        .create(manual, "name: test-workflow\nsteps: []\n")
        .unwrap();

    // event-triggered run
    let mut event_run = seed_run("run_trigger_event");
    event_run.event = Some(serde_json::json!({"x": 1}));
    store
        .create(event_run, "name: test-workflow\nsteps: []\n")
        .unwrap();

    // cron-triggered run
    let mut cron_run = seed_run("run_trigger_cron");
    cron_run.source_wake_id = Some("wake_1".into());
    store
        .create(cron_run, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 3, "expected all three seeded runs; got {}", arr.len());

    let find_trigger = |id: &str| -> &str {
        arr.iter()
            .find(|r| r["id"].as_str() == Some(id))
            .and_then(|r| r["trigger"].as_str())
            .unwrap_or("MISSING")
    };

    assert_eq!(find_trigger("run_trigger_manual"), "manual");
    assert_eq!(find_trigger("run_trigger_event"), "event");
    assert_eq!(find_trigger("run_trigger_cron"), "cron");
}

/// GET /api/runs/workflows returns only manual runs.
#[tokio::test]
async fn list_workflow_runs_filters_to_manual_only() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));

    let manual = seed_run("run_wf_manual");
    store
        .create(manual, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let mut event_run = seed_run("run_wf_event");
    event_run.event = Some(serde_json::json!({"x": 1}));
    store
        .create(event_run, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let mut cron_run = seed_run("run_wf_cron");
    cron_run.source_wake_id = Some("wake_1".into());
    store
        .create(cron_run, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/workflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(
        arr.len(),
        1,
        "only the manual run should be returned; got {arr:?}"
    );
    assert_eq!(
        arr[0]["id"].as_str(),
        Some("run_wf_manual"),
        "manual run id mismatch"
    );
    assert_eq!(
        arr[0]["trigger"].as_str(),
        Some("manual"),
        "trigger field should be 'manual'"
    );
}

/// GET /api/runs/workflows returns empty array when no manual runs exist.
#[tokio::test]
async fn list_workflow_runs_empty_when_no_manual_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));

    let mut event_run = seed_run("run_wf_only_event");
    event_run.event = Some(serde_json::json!({"type": "push"}));
    store
        .create(event_run, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/workflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(
        arr.is_empty(),
        "no manual runs seeded, /api/runs/workflows should be empty; got {arr:?}"
    );
}
