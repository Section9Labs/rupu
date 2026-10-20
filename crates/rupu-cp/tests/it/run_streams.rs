// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use chrono::Utc;
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
    AutoflowHistoryStore,
};

/// Construct an AppState rooted at `dir` and spin up an axum test server.
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

/// Seed a minimal cycle with known counts into a store rooted at
/// `<global_dir>/autoflows/history` (the path the handler uses).
fn seed_cycle(global_dir: &std::path::Path) -> AutoflowCycleRecord {
    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let now = Utc::now();
    let mut cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, now);
    cycle.workflow_count = 3;
    cycle.ran_cycles = 2;
    cycle.skipped_cycles = 1;
    cycle.failed_cycles = 0;
    cycle.finished_at = now.to_rfc3339();
    // Attach an event that carries a run_id so we can verify harvesting.
    cycle.events.push(AutoflowCycleEvent {
        kind: AutoflowCycleEventKind::RunLaunched,
        run_id: Some("run_autoflow_seeded_01".into()),
        ..Default::default()
    });

    store.save(&cycle).unwrap();
    cycle
}

#[tokio::test]
async fn list_autoflows_returns_seeded_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let seeded = seed_cycle(tmp.path());

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 1, "expected exactly one cycle; got {}", arr.len());

    let row = &arr[0];
    assert_eq!(
        row["cycle_id"].as_str(),
        Some(seeded.cycle_id.as_str()),
        "cycle_id mismatch"
    );
    assert_eq!(row["mode"].as_str(), Some("tick"), "mode should be 'tick'");
    assert_eq!(
        row["workflow_count"].as_u64(),
        Some(3),
        "workflow_count mismatch"
    );
    assert_eq!(
        row["ran_cycles"].as_u64(),
        Some(2),
        "ran_cycles mismatch"
    );
    assert_eq!(
        row["skipped_cycles"].as_u64(),
        Some(1),
        "skipped_cycles mismatch"
    );
    assert_eq!(
        row["failed_cycles"].as_u64(),
        Some(0),
        "failed_cycles mismatch"
    );

    // The run_id from the embedded event should be surfaced.
    let run_ids = row["run_ids"].as_array().expect("run_ids should be an array");
    assert_eq!(run_ids.len(), 1, "expected one run_id; got {run_ids:?}");
    assert_eq!(
        run_ids[0].as_str(),
        Some("run_autoflow_seeded_01"),
        "run_id value mismatch"
    );
}

/// Seed an autoflow `RunLaunched` event (carrying a workflow name + run_id)
/// into the events store at `<global_dir>/autoflows/history`.
fn seed_run_launched_event(global_dir: &std::path::Path) -> AutoflowCycleRecord {
    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let now = Utc::now();
    let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Serve, now);
    store
        .append_cycle_event(
            &cycle,
            AutoflowCycleEvent {
                kind: AutoflowCycleEventKind::RunLaunched,
                workflow: Some("triage-and-fix".into()),
                issue_display_ref: Some("Section9Labs/rupu#42".into()),
                run_id: Some("run_autoflow_event_01".into()),
                ..Default::default()
            },
            now,
        )
        .unwrap();
    cycle
}

#[tokio::test]
async fn list_autoflow_events_returns_seeded_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let cycle = seed_run_launched_event(tmp.path());

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows/events"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert_eq!(arr.len(), 1, "expected exactly one event; got {}", arr.len());

    let row = &arr[0];
    assert_eq!(
        row["kind"].as_str(),
        Some("run_launched"),
        "kind should be 'run_launched'"
    );
    assert_eq!(
        row["workflow"].as_str(),
        Some("triage-and-fix"),
        "workflow name mismatch"
    );
    assert_eq!(
        row["run_id"].as_str(),
        Some("run_autoflow_event_01"),
        "run_id mismatch"
    );
    assert_eq!(
        row["issue_display_ref"].as_str(),
        Some("Section9Labs/rupu#42"),
        "issue_display_ref mismatch"
    );
    assert_eq!(
        row["cycle_id"].as_str(),
        Some(cycle.cycle_id.as_str()),
        "cycle_id mismatch"
    );
}

#[tokio::test]
async fn list_autoflow_events_empty_when_no_store_dir() {
    let tmp = tempfile::tempdir().unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows/events"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "missing store dir should return 200, not 500"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(
        arr.is_empty(),
        "no events seeded, response should be []; got {arr:?}"
    );
}

#[tokio::test]
async fn list_autoflows_empty_when_no_store_dir() {
    // Spin up a server with a global_dir that has NO autoflows/history subdir.
    let tmp = tempfile::tempdir().unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/autoflows"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "missing store dir should return 200, not 500"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(
        arr.is_empty(),
        "no cycles seeded, response should be []; got {arr:?}"
    );
}

// ---------------------------------------------------------------------------
// Agent runs tests
// ---------------------------------------------------------------------------

/// Write a minimal `<run_id>.meta.json` into `<global>/transcripts/`.
fn seed_standalone_meta(global_dir: &std::path::Path, run_id: &str) {
    let transcripts = global_dir.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    let meta = serde_json::json!({
        "version": 1,
        "run_id": run_id,
        "session_id": null,
        "workspace_path": "/tmp/repo",
        "backend_id": "local_checkout",
        "trigger_source": "run_cli"
    });
    let path = transcripts.join(format!("{run_id}.meta.json"));
    std::fs::write(path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
}

/// Write a `session.json` with one embedded run into
/// `<global>/sessions/<session_id>/session.json`.
fn seed_session_with_run(
    global_dir: &std::path::Path,
    session_id: &str,
    agent_name: &str,
    run_id: &str,
) {
    let session_dir = global_dir.join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let transcript_path = format!("/tmp/.rupu/transcripts/{run_id}.jsonl");
    let session = serde_json::json!({
        "version": 1,
        "session_id": session_id,
        "agent_name": agent_name,
        "runs": [
            {
                "run_id": run_id,
                "prompt": "do the thing",
                "transcript_path": transcript_path,
                "started_at": "2026-06-01T10:00:00Z",
                "status": "ok"
            }
        ]
    });
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::to_string_pretty(&session).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn list_agent_runs_returns_standalone_meta() {
    let tmp = tempfile::tempdir().unwrap();
    seed_standalone_meta(tmp.path(), "run_standalone_01");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(!arr.is_empty(), "expected at least one row; got []");

    let row = arr
        .iter()
        .find(|r| r["run_id"].as_str() == Some("run_standalone_01"))
        .expect("run_standalone_01 should be present");

    assert_eq!(row["source"].as_str(), Some("standalone"), "source mismatch");
    assert_eq!(
        row["trigger_source"].as_str(),
        Some("run_cli"),
        "trigger_source mismatch"
    );
}

#[tokio::test]
async fn list_agent_runs_returns_session_run() {
    let tmp = tempfile::tempdir().unwrap();
    seed_session_with_run(tmp.path(), "sess_01", "my-agent", "run_session_01");

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");

    let row = arr
        .iter()
        .find(|r| r["run_id"].as_str() == Some("run_session_01"))
        .expect("run_session_01 should be present");

    assert_eq!(row["source"].as_str(), Some("session"), "source mismatch");
    assert_eq!(
        row["agent"].as_str(),
        Some("my-agent"),
        "agent name mismatch"
    );
    assert_eq!(
        row["session_id"].as_str(),
        Some("sess_01"),
        "session_id mismatch"
    );
    assert_eq!(row["status"].as_str(), Some("ok"), "status mismatch");
    assert_eq!(
        row["started_at"].as_str(),
        Some("2026-06-01T10:00:00Z"),
        "started_at mismatch"
    );
}

#[tokio::test]
async fn list_agent_runs_empty_when_no_dirs() {
    // global_dir has no transcripts/ or sessions/ — should return [] not 500.
    let tmp = tempfile::tempdir().unwrap();

    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "missing dirs should return 200, not 500"
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(
        arr.is_empty(),
        "no data seeded, response should be []; got {arr:?}"
    );
}

/// What a `rupu run --run-id <id>` the CP launched leaves when its provider
/// can't be built (no credential): its metadata, the transcript
/// `AssembleError::record_unstarted` writes for the real missing-credential
/// error, and a failed run.json. `rupu-cli`'s
/// `run_id_launch_without_credentials_records_a_failed_run` pins that the
/// CLI writes exactly these.
async fn seed_unstarted_run(global: &std::path::Path, run_id: &str) {
    seed_standalone_meta(global, run_id);
    let transcript = global.join("transcripts").join(format!("{run_id}.jsonl"));
    let source = match rupu_runtime::provider_factory::build_for_provider(
        "anthropic",
        "claude-sonnet-4-6",
        None,
        &rupu_auth::in_memory::InMemoryResolver::new(),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )
    .await
    {
        Ok(_) => panic!("no credential: the provider must not build"),
        Err(e) => e,
    };
    let failure = rupu_runtime::assembly::AssembleError::ProviderBuild {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        source,
    }
    .record_unstarted(rupu_agent::UnstartedRun {
        launch_args: &[],
        transcript_path: &transcript,
        run_id,
        workspace_id: "ws_1",
        agent: "echo",
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        mode: rupu_tools::PermissionMode::Bypass,
        codename: None,
        customer: None,
        system_prompt: Some("you echo."),
        prompt: &rupu_agent::UserTurn::new("go".to_string()),
        launch_skills: &[],
        launch_preload_skills: &[],
    });

    let now = Utc::now();
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let record: rupu_orchestrator::RunRecord = serde_json::from_value(serde_json::json!({
        "id": run_id,
        "workflow_name": format!("{}echo", rupu_orchestrator::AGENT_RUN_PREFIX),
        "status": "failed",
        "inputs": {},
        "workspace_id": "ws_1",
        "workspace_path": "/tmp/repo",
        "transcript_dir": global.join("transcripts"),
        "started_at": now,
        "finished_at": now,
        "error_message": failure.to_string(),
    }))
    .unwrap();
    store.create(record, "").unwrap();
}

/// A `--run-id` launch that failed before its agent loop started (no
/// credential) is listed under its id as `failed`, and its transcript reads
/// the real error with the `rupu auth login` hint.
#[tokio::test]
async fn unstarted_agent_run_is_listed_failed_with_its_error() {
    let tmp = tempfile::tempdir().unwrap();
    let run_id = "run_unstarted_01";
    seed_unstarted_run(tmp.path(), run_id).await;
    // A run with no run.json (legacy, or a session turn) keeps no status.
    seed_standalone_meta(tmp.path(), "run_no_record_02");

    let addr = spawn_server(tmp.path()).await;
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/runs/agents"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = body.as_array().expect("an array of rows");
    let row = rows
        .iter()
        .find(|r| r["run_id"].as_str() == Some(run_id))
        .expect("the unstarted run is listed");
    assert_eq!(row["status"].as_str(), Some("failed"), "row: {row}");
    assert_eq!(row["agent"].as_str(), Some("echo"));
    let unrecorded = rows
        .iter()
        .find(|r| r["run_id"].as_str() == Some("run_no_record_02"))
        .expect("a run without a record is still listed");
    assert!(unrecorded["status"].is_null(), "no record, no status");

    let failed: serde_json::Value =
        reqwest::get(format!("http://{addr}/api/runs/agents?lifecycle=failed"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let failed_ids: Vec<_> = failed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["run_id"].as_str())
        .collect();
    assert_eq!(failed_ids, [run_id]);

    let transcript = row["transcript_path"].as_str().unwrap();
    let text = reqwest::Client::new()
        .get(format!("http://{addr}/api/transcript"))
        .query(&[("path", transcript)])
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        text.contains("rupu auth login --provider anthropic"),
        "the transcript carries the error and its hint: {text}"
    );
}
