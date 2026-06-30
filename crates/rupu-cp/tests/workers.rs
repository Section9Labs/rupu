//! Integration test for `GET /api/workers` — verifies that worker records are
//! enriched with run-activity attribution (active/total counts + last_run_at).

use chrono::Utc;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use rupu_runtime::{WorkerCapabilities, WorkerKind, WorkerRecord};
use rupu_workspace::worker_store::WorkerStore;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn seed_run(
    id: &str,
    status: RunStatus,
    worker_id: Option<&str>,
    started_at: chrono::DateTime<Utc>,
) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "test-workflow".into(),
        status,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_test".into(),
        workspace_path: PathBuf::from("/tmp/test-proj"),
        transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
        started_at,
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
        worker_id: worker_id.map(str::to_string),
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

/// Seed a worker record + three runs (two for this worker, one for another) and
/// assert the endpoint returns the right per-worker counts and `last_run_at`.
#[tokio::test]
async fn workers_endpoint_enriches_with_run_activity() {
    let tmp = tempfile::tempdir().unwrap();

    // A worker record on disk, at `<global>/autoflows/workers/`.
    let store = WorkerStore {
        root: tmp.path().join("autoflows").join("workers"),
    };
    let worker = WorkerRecord {
        version: WorkerRecord::VERSION,
        worker_id: "worker_local_team-mini_cli".into(),
        kind: WorkerKind::Cli,
        name: "team-mini".into(),
        host: "team-mini.local".into(),
        capabilities: WorkerCapabilities {
            backends: vec!["local_worktree".into()],
            scm_hosts: vec!["github".into()],
            permission_modes: vec!["bypass".into()],
        },
        registered_at: "2026-05-09T16:00:00Z".into(),
        last_seen_at: "2026-05-09T16:10:00Z".into(),
    };
    store.save(&worker).unwrap();

    // Two runs attributed to this worker (one terminal/older, one active/newer),
    // plus one run for a *different* worker that must NOT be attributed.
    let run_store = RunStore::new(tmp.path().join("runs"));
    let older = Utc::now() - chrono::Duration::hours(1);
    let newer = Utc::now();
    run_store
        .create(
            seed_run("run_done", RunStatus::Completed, Some(&worker.worker_id), older),
            "name: x\n",
        )
        .unwrap();
    run_store
        .create(
            seed_run("run_live", RunStatus::Running, Some(&worker.worker_id), newer),
            "name: x\n",
        )
        .unwrap();
    run_store
        .create(
            seed_run("run_other", RunStatus::Running, Some("worker_other"), newer),
            "name: x\n",
        )
        .unwrap();

    let addr = spawn_server(tmp.path()).await;
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/workers"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let arr = body.as_array().expect("response is an array");
    // Only the saved worker has a record; the other-worker run has no record.
    assert_eq!(arr.len(), 1, "exactly one worker record expected");
    let w = &arr[0];
    assert_eq!(w["worker_id"], "worker_local_team-mini_cli");
    // Flattened base record fields are still present.
    assert_eq!(w["name"], "team-mini");
    assert_eq!(w["kind"], "cli");
    // Activity attribution.
    assert_eq!(w["active_run_count"], 1);
    assert_eq!(w["total_run_count"], 2);
    // last_run_at is the newer (Running) run's started_at.
    assert!(w["last_run_at"].is_string(), "last_run_at should be set");
}

/// A worker with no runs reports zero counts and a null `last_run_at`.
#[tokio::test]
async fn worker_without_runs_reports_zero_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WorkerStore {
        root: tmp.path().join("autoflows").join("workers"),
    };
    store
        .save(&WorkerRecord {
            version: WorkerRecord::VERSION,
            worker_id: "worker_idle".into(),
            kind: WorkerKind::AutoflowServe,
            name: "idle".into(),
            host: "idle.local".into(),
            capabilities: WorkerCapabilities::default(),
            registered_at: "2026-05-09T16:00:00Z".into(),
            last_seen_at: "2026-05-09T16:10:00Z".into(),
        })
        .unwrap();

    let addr = spawn_server(tmp.path()).await;
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/workers"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let w = &body.as_array().unwrap()[0];
    assert_eq!(w["active_run_count"], 0);
    assert_eq!(w["total_run_count"], 0);
    assert!(w["last_run_at"].is_null());
}
