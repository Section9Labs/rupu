//! Integration tests for `LocalHostConnector` — verifies the local host[0]
//! parity: same JSON shape as the HTTP API, info reachable, not-found error.

use chrono::Utc;
use rupu_cp::host::{
    connector::{HostConnector, HostConnectorError, RunKind, RunListQuery},
    local::LocalHostConnector,
};
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

// ── helpers ──────────────────────────────────────────────────────────────────

fn seed_run(id: &str, status: RunStatus) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "test-workflow".into(),
        status,
        inputs: BTreeMap::from([("prompt".into(), "hello".into())]),
        event: None,
        workspace_id: "ws_test".into(),
        workspace_path: PathBuf::from("/tmp/test-proj"),
        transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
        started_at: Utc::now(),
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
    }
}

fn make_connector(tmp: &tempfile::TempDir, store: Arc<RunStore>) -> LocalHostConnector {
    LocalHostConnector::new(None, None, None, None, store, tmp.path().to_path_buf())
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `list_runs(RunKind::All)` returns the same row shape the `/api/runs`
/// handler produces — one row per seeded run, id matches.
#[tokio::test]
async fn local_connector_lists_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let run_id = "local_list_run_01";
    store
        .create(
            seed_run(run_id, RunStatus::Completed),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();

    let connector = make_connector(&tmp, store);
    let rows = connector
        .list_runs(RunListQuery {
            kind: RunKind::All,
            offset: 0,
            limit: 50,
            lifecycle: None,
        })
        .await
        .expect("list_runs should succeed");

    assert_eq!(rows.len(), 1, "expected exactly 1 row");
    assert_eq!(
        rows[0]["id"].as_str(),
        Some(run_id),
        "row id should match seeded run"
    );
    // Spot-check the same fields the HTTP API returns.
    assert!(rows[0].get("status").is_some(), "row should have 'status'");
    assert!(rows[0].get("usage").is_some(), "row should have 'usage'");
}

/// `list_runs(RunKind::Workflow)` excludes event-triggered runs (same filter
/// as `GET /api/runs/workflows`).
#[tokio::test]
async fn local_connector_list_runs_workflow_only_excludes_event_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));

    let manual = seed_run("local_wf_manual", RunStatus::Completed);
    store
        .create(manual, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let mut event_run = seed_run("local_wf_event", RunStatus::Completed);
    event_run.event = Some(serde_json::json!({"type": "push"}));
    store
        .create(event_run, "name: test-workflow\nsteps: []\n")
        .unwrap();

    let connector = make_connector(&tmp, store);
    let rows = connector
        .list_runs(RunListQuery {
            kind: RunKind::Workflow,
            offset: 0,
            limit: 50,
            lifecycle: None,
        })
        .await
        .expect("list_runs(Workflow) should succeed");

    assert_eq!(rows.len(), 1, "only the manual run should appear");
    assert_eq!(rows[0]["id"].as_str(), Some("local_wf_manual"));
}

/// `get_run` returns the `{ run, steps, usage }` shape.
#[tokio::test]
async fn local_connector_get_run_returns_expected_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let run_id = "local_get_run_01";
    store
        .create(
            seed_run(run_id, RunStatus::Completed),
            "name: test-workflow\nsteps: []\n",
        )
        .unwrap();

    let connector = make_connector(&tmp, store);
    let detail = connector
        .get_run(run_id)
        .await
        .expect("get_run should succeed");

    assert_eq!(
        detail["run"]["id"].as_str(),
        Some(run_id),
        "run.id should match"
    );
    assert!(
        detail.get("steps").is_some(),
        "detail should include 'steps'"
    );
    assert!(
        detail.get("usage").is_some(),
        "detail should include 'usage'"
    );
}

/// `get_run` on a missing id returns `HostConnectorError::NotFound`.
#[tokio::test]
async fn local_connector_get_run_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let connector = make_connector(&tmp, store);

    let result = connector.get_run("no-such-run").await;
    assert!(
        matches!(result, Err(HostConnectorError::NotFound(_))),
        "expected NotFound, got {result:?}"
    );
}

/// `info()` reports reachable=true and a non-empty version string.
#[tokio::test]
async fn local_connector_info_is_reachable_with_version() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let connector = make_connector(&tmp, store);

    let info = connector.info().await.expect("info should succeed");
    assert!(info.reachable, "local connector should always be reachable");
    assert!(
        info.version.as_deref().is_some_and(|v| !v.is_empty()),
        "version should be a non-empty string"
    );
}
