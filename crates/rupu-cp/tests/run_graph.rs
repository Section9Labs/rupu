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

  - id: panel-step
    actions: []
    panel:
      panelists:
        - reviewer-a
        - reviewer-b
      subject: "review me"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: developer
        max_iterations: 3
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
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
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
        3,
        "workflow.steps should include all steps (incl. the pending ones)"
    );
    let step_ids: Vec<&str> = steps.iter().filter_map(|s| s["id"].as_str()).collect();
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
    let units = body["units"].as_array().expect("units should be an array");
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
async fn run_graph_merges_panel_units_from_event_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let run_id = "run_graph_panel_01";

    store.create(seed_run(run_id), WF_YAML).unwrap();

    // A panel step's panelist/fixer runs live ONLY in events.jsonl for a
    // completed run — never in unit_checkpoints.jsonl. Seed two such units
    // (one with a matching UnitCompleted) plus a SECOND checkpoint for the
    // for_each step that ALSO has an events entry for the same (step,index)
    // to prove the checkpoint wins (no duplicate).
    store
        .append_unit_checkpoint(run_id, &seed_unit(run_id, "second-step"))
        .unwrap();

    let events = [
        // panelist 0 — started + completed (success)
        serde_json::json!({
            "type": "unit_started",
            "run_id": run_id,
            "step_id": "panel-step",
            "index": 0,
            "unit_key": "reviewer-a",
            "agent": "reviewer-a",
            "transcript_path": "/tmp/panel_reviewer_a.jsonl",
        }),
        serde_json::json!({
            "type": "unit_completed",
            "run_id": run_id,
            "step_id": "panel-step",
            "index": 0,
            "unit_key": "reviewer-a",
            "success": true,
            "tokens_in": 0,
            "tokens_out": 0,
        }),
        // panelist 1 — started only (no completion → success stays null)
        serde_json::json!({
            "type": "unit_started",
            "run_id": run_id,
            "step_id": "panel-step",
            "index": 1,
            "unit_key": "reviewer-b",
            "agent": "reviewer-b",
            "transcript_path": "/tmp/panel_reviewer_b.jsonl",
        }),
        // DUPLICATE of the seeded checkpoint — must NOT produce a 2nd entry.
        serde_json::json!({
            "type": "unit_started",
            "run_id": run_id,
            "step_id": "second-step",
            "index": 0,
            "unit_key": "src/lib.rs",
            "agent": "file-reviewer",
            "transcript_path": "/tmp/SHOULD_NOT_WIN.jsonl",
        }),
    ];
    let body: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(store.events_path(run_id), body).unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/graph"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let units = body["units"].as_array().expect("units array");

    // 1 checkpoint (second-step) + 2 events-only (panel-step). The duplicate
    // events entry for second-step is deduped away → 3 total, not 4.
    assert_eq!(
        units.len(),
        3,
        "expected checkpoint + 2 panel units, no duplicate; got {units:#?}"
    );

    // Checkpoint wins for second-step: its transcript path is the checkpoint's.
    let second = units
        .iter()
        .find(|u| u["step_id"].as_str() == Some("second-step"))
        .expect("second-step checkpoint present");
    assert_eq!(
        second["transcript_path"].as_str(),
        Some(format!("/tmp/{run_id}_second-step_unit0.jsonl").as_str()),
        "checkpoint transcript must win over the events entry"
    );

    // Panel units carry their transcript paths.
    let panel: Vec<&serde_json::Value> = units
        .iter()
        .filter(|u| u["step_id"].as_str() == Some("panel-step"))
        .collect();
    assert_eq!(panel.len(), 2, "both panelist units present");

    let p0 = panel
        .iter()
        .find(|u| u["index"].as_u64() == Some(0))
        .unwrap();
    assert_eq!(p0["item"].as_str(), Some("reviewer-a"));
    assert_eq!(
        p0["transcript_path"].as_str(),
        Some("/tmp/panel_reviewer_a.jsonl")
    );
    assert_eq!(
        p0["success"].as_bool(),
        Some(true),
        "completed panelist should carry success=true"
    );

    let p1 = panel
        .iter()
        .find(|u| u["index"].as_u64() == Some(1))
        .unwrap();
    assert_eq!(p1["item"].as_str(), Some("reviewer-b"));
    assert_eq!(
        p1["transcript_path"].as_str(),
        Some("/tmp/panel_reviewer_b.jsonl")
    );
    assert!(
        p1["success"].is_null(),
        "started-but-not-completed panelist should have null success"
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
        body["workflow"]["steps"].as_array().map(|a| a.len()),
        Some(3),
        "workflow.steps should still have 3 entries"
    );
}
