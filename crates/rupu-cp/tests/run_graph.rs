use chrono::Utc;
use rupu_orchestrator::runs::{
    RunRecord, RunStatus, RunStore, StepKind, StepResultRecord, UnitCheckpoint,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

// ── Workflow fixture ─────────────────────────────────────────────────────
//
// Two steps: one plain linear step (result seeded) and one for_each (no
// result seeded, so it should still appear in workflow.steps as "pending").

const WF_YAML: &str = r#"
name: graph-test
steps:
  - id: first-step
    agent: analyst
    actions: []
    prompt: "Analyse {{ inputs.target }}"

  - id: second-step
    agent: file-reviewer
    actions: []
    for_each: "{{ inputs.files }}"
    prompt: "Review {{ item }}"
"#;

// ── Helpers ──────────────────────────────────────────────────────────────

fn seed_run(id: &str) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "graph-test".into(),
        status: RunStatus::Completed,
        inputs: BTreeMap::from([("target".into(), "main.rs".into())]),
        event: None,
        workspace_id: "ws_graph_test".into(),
        workspace_path: PathBuf::from("/tmp/graph-test-proj"),
        transcript_dir: PathBuf::from("/tmp/graph-test-proj/.rupu/transcripts"),
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
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

fn seed_step(run_id: &str, step_id: &str) -> StepResultRecord {
    StepResultRecord {
        step_id: step_id.into(),
        run_id: run_id.into(),
        transcript_path: PathBuf::from(format!("/tmp/{step_id}.jsonl")),
        output: "done".into(),
        success: true,
        skipped: false,
        rendered_prompt: "Analyse main.rs".into(),
        kind: StepKind::Linear,
        items: Vec::new(),
        findings: Vec::new(),
        iterations: 0,
        resolved: true,
        finished_at: Utc::now(),
    }
}

fn seed_unit(run_id: &str, step_id: &str) -> UnitCheckpoint {
    UnitCheckpoint {
        step_id: step_id.into(),
        index: 0,
        item: serde_json::json!("src/lib.rs"),
        run_id: run_id.into(),
        transcript_path: PathBuf::from(format!("/tmp/{run_id}_{step_id}_unit0.jsonl")),
        output: "unit done".into(),
        success: true,
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

// ── Tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn run_graph_returns_workflow_step_results_and_units() {
    let tmp = tempfile::tempdir().unwrap();

    // Seed the store before starting the server.
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let run_id = "run_graph_test_01";

    store.create(seed_run(run_id), WF_YAML).unwrap();

    // Seed ONE step result for the first step only — second-step is pending.
    store
        .append_step_result(run_id, &seed_step(run_id, "first-step"))
        .unwrap();

    // Seed ONE unit checkpoint for the for_each step.
    store
        .append_unit_checkpoint(run_id, &seed_unit(run_id, "second-step"))
        .unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/graph"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 for existing run");

    let body: serde_json::Value = resp.json().await.unwrap();

    // run envelope present
    assert_eq!(
        body["run"]["id"].as_str(),
        Some(run_id),
        "run.id should match"
    );

    // Workflow DAG must include ALL steps, even those with no result.
    let steps = body["workflow"]["steps"]
        .as_array()
        .expect("workflow.steps should be an array");
    assert_eq!(
        steps.len(),
        2,
        "workflow.steps should include both steps (incl. the pending one)"
    );
    let step_ids: Vec<&str> = steps
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert!(
        step_ids.contains(&"first-step"),
        "first-step missing from dag; got {step_ids:?}"
    );
    assert!(
        step_ids.contains(&"second-step"),
        "second-step (pending) missing from dag; got {step_ids:?}"
    );

    // second-step should be mapped as for_each kind
    let second = steps
        .iter()
        .find(|s| s["id"].as_str() == Some("second-step"))
        .expect("second-step node must be present");
    assert_eq!(
        second["kind"].as_str(),
        Some("for_each"),
        "second-step kind should be for_each"
    );

    // step_results: only the seeded result
    let results = body["step_results"]
        .as_array()
        .expect("step_results should be an array");
    assert_eq!(results.len(), 1, "exactly one step result seeded");
    assert_eq!(
        results[0]["step_id"].as_str(),
        Some("first-step"),
        "step_result step_id should match"
    );

    // units: exactly the seeded checkpoint
    let units = body["units"]
        .as_array()
        .expect("units should be an array");
    assert_eq!(units.len(), 1, "exactly one unit checkpoint seeded");
    assert_eq!(
        units[0]["step_id"].as_str(),
        Some("second-step"),
        "unit checkpoint step_id should match"
    );
    assert_eq!(
        units[0]["index"].as_u64(),
        Some(0),
        "unit checkpoint index should be 0"
    );
    assert_eq!(
        units[0]["success"].as_bool(),
        Some(true),
        "unit checkpoint success should be true"
    );
}

#[tokio::test]
async fn run_graph_unknown_id_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/unknown/graph"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "unknown run should yield 404");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().is_some(),
        "404 body should have an 'error' field; got {body}"
    );
}

#[tokio::test]
async fn run_graph_no_units_or_results_returns_empty_arrays() {
    let tmp = tempfile::tempdir().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let run_id = "run_graph_empty_01";
    store.create(seed_run(run_id), WF_YAML).unwrap();
    // No step results, no unit checkpoints seeded.

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/graph"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["step_results"].as_array().map(|a| a.len()),
        Some(0),
        "step_results should be empty"
    );
    assert_eq!(
        body["units"].as_array().map(|a| a.len()),
        Some(0),
        "units should be empty"
    );
    // Workflow DAG still present.
    assert_eq!(
        body["workflow"]["steps"]
            .as_array()
            .map(|a| a.len()),
        Some(2),
        "workflow.steps should still have 2 entries"
    );
}
