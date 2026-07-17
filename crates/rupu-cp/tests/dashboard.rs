//! Integration tests for `GET /api/dashboard` (Task 7: fan-out across hosts).
//!
//! The endpoint now fans `dashboard_summary()` out across every registered
//! host and merges only the hosts that actually reported. A host that cannot
//! report (offline, or `Unsupported`) must surface in `hosts[]` as
//! `offline` / `unavailable` rather than contributing zeroed counts.

// ---------------------------------------------------------------------------
// Spawn helpers (mirrors tests/host_reads.rs; helpers are duplicated per file
// — there is no shared `tests/common/` module in this crate).
// ---------------------------------------------------------------------------

struct TestServer {
    base_url: String,
}

/// Spin up a read-only local-only server.
async fn spawn_server(dir: &std::path::Path) -> TestServer {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{addr}"),
    }
}

/// Spin up a server with one remote host pre-registered via the registry.
async fn spawn_server_with_remote(dir: &std::path::Path, mock_base_url: &str) -> TestServer {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    state
        .hosts
        .add_host("mock-remote", mock_base_url, None)
        .expect("add_host should succeed");
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{addr}"),
    }
}

// ---------------------------------------------------------------------------
// Seeders for the host_id tagging test (mirrors host_reads.rs /
// federation_e2e.rs — helpers are duplicated per file, no shared
// `tests/common/`).
// ---------------------------------------------------------------------------

/// Build a minimal, manually-triggered `RunRecord` (no `event`, no
/// `source_wake_id` — see `RunRecord::trigger_str()` in
/// `crates/rupu-orchestrator/src/runs.rs`).
fn seed_run(
    id: &str,
    status: rupu_orchestrator::runs::RunStatus,
) -> rupu_orchestrator::runs::RunRecord {
    rupu_orchestrator::runs::RunRecord {
        id: id.into(),
        workflow_name: "dash-wf".into(),
        status,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: "ws_dash".into(),
        workspace_path: std::path::PathBuf::from("/tmp/dash-proj"),
        transcript_dir: std::path::PathBuf::from("/tmp/dash-proj/.rupu/transcripts"),
        started_at: chrono::Utc::now(),
        finished_at: None,
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

/// Seed a cycle with one `RunLaunched` event referencing `run_id`.
///
/// `collect_cycle_rollups` (`host/local.rs`) reads cycles via
/// `AutoflowHistoryStore::list_recent`, which reads back only what `save()`
/// wrote — NOT the separate append-only event log `append_cycle_event`
/// writes to. The run ids it harvests (`run_streams::harvest_run_ids`) come
/// from the in-memory `record.events` field, so the event must be pushed onto
/// the record *before* `save()`, mirroring `host/local.rs`'s own
/// `collect_cycle_rollups_reads_a_fixture_written_by_the_real_store` test.
fn seed_autoflow_cycle_with_run(global_dir: &std::path::Path, run_id: &str) {
    use rupu_runtime::{
        AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
        AutoflowHistoryStore,
    };
    let store_root = global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);
    let now = chrono::Utc::now();
    let mut cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, now);
    cycle.events.push(AutoflowCycleEvent {
        kind: AutoflowCycleEventKind::RunLaunched,
        workflow: Some("dash-wf".into()),
        run_id: Some(run_id.into()),
        ..Default::default()
    });
    store.save(&cycle).unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dashboard_reflects_seeded_active_run_and_cycle_in_aggregate_fields() {
    // The redesign replaced per-row lists (active_runs / cycles / recent_manual,
    // each tagged with host_id) with aggregate-only key points: a seeded run
    // must surface through `active`/`active_longest`/`throughput_buckets`, and
    // a seeded cycle through `cycles.total` — never as a row array.
    let dir = tempfile::tempdir().unwrap();

    // A standalone manual, non-terminal run.
    let run_store = rupu_orchestrator::runs::RunStore::new(dir.path().join("runs"));
    run_store
        .create(
            seed_run(
                "dash_active_run",
                rupu_orchestrator::runs::RunStatus::Running,
            ),
            "name: dash-wf\nsteps: []\n",
        )
        .unwrap();

    // A cycle referencing a *different* run id.
    seed_autoflow_cycle_with_run(dir.path(), "dash_cycle_run");

    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=all", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        body["active"]["running"], 1,
        "the seeded running run must be tallied into active.running: {body}"
    );

    let active_longest = &body["active_longest"];
    assert_eq!(
        active_longest["run_id"], "dash_active_run",
        "the only non-terminal run must be reported as active_longest: {body}"
    );

    assert!(
        body["throughput_buckets"]
            .as_array()
            .expect("throughput_buckets array")
            .iter()
            .any(|b| b["manual"].as_u64().unwrap_or(0) >= 1),
        "the seeded manual run must be tallied into a throughput bucket: {body}"
    );

    assert!(
        body["cycles"]["total"].as_u64().unwrap_or(0) >= 1,
        "the seeded cycle must be tallied into cycles.total: {body}"
    );

    // Neither the old row DTOs nor a per-row host_id concept exist any more.
    assert!(body.get("active_runs").is_none());
    assert!(body.get("recent_manual").is_none());
    assert!(
        body["cycles"].is_object(),
        "cycles must be a scalar, not an array: {body}"
    );
}

#[tokio::test]
async fn dashboard_reports_per_host_freshness_and_never_zeroes_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert!(!hosts.is_empty(), "local must always appear");
    let local = &hosts[0];
    assert_eq!(local["host_id"], "local");
    assert_eq!(local["state"], "ok");
    assert!(
        local["captured_at"].as_str().unwrap().contains('T'),
        "captured_at must be RFC-3339 for the freshness strip"
    );
}

#[tokio::test]
async fn dashboard_rejects_unknown_range() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/dashboard?range=bogus", srv.base_url))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "an unparseable range must 400, not silently default"
    );
}

#[tokio::test]
async fn dashboard_unavailable_host_renders_unavailable_not_zero() {
    // A host that cannot report is NOT a host with no runs. Register an
    // unreachable remote and assert it surfaces as a distinct state.
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().unwrap();
    let remote = hosts
        .iter()
        .find(|h| h["host_id"] != "local")
        .expect("the unreachable remote must still appear in the freshness strip");
    assert_ne!(
        remote["state"], "ok",
        "an unreachable host must not report ok"
    );
    assert!(
        remote["captured_at"].is_null(),
        "an unreachable host has no captured_at — it never reported"
    );
}

#[tokio::test]
async fn dashboard_unknown_host_returns_404() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/dashboard?host=nope", srv.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "an unknown host id must 404");
}

#[tokio::test]
async fn dashboard_body_parses_as_a_bare_dashboard_summary() {
    // HttpHostConnector::dashboard_summary proxies this endpoint and parses the
    // body as a bare DashboardSummary. If this ever stops holding, every HTTP
    // host in a fan-out silently reports `offline`.
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value = reqwest::get(format!("{}/api/dashboard?range=30d", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let parsed: Result<rupu_cp::host::dashboard_summary::DashboardSummary, _> =
        serde_json::from_value(body.clone());
    assert!(
        parsed.is_ok(),
        "body must parse as DashboardSummary: {:?}",
        parsed.err()
    );
    assert!(
        body["captured_at"].is_string(),
        "captured_at must be TOP-LEVEL, not nested"
    );
    assert!(
        body["hosts"].is_array(),
        "hosts[] must still be present alongside it"
    );
}

#[tokio::test]
async fn dashboard_scoped_to_host_local_returns_only_local() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value =
        reqwest::get(format!("{}/api/dashboard?host=local", srv.base_url))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert_eq!(
        hosts.len(),
        1,
        "?host=local must not also probe the registered remote"
    );
    assert_eq!(hosts[0]["host_id"], "local");
}
