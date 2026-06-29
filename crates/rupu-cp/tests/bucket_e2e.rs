//! End-to-end tests for the bucket transport (Slice 2b).
//!
//! Task 5 coverage (this file, shared with Task 8):
//! - `poll_bucket_run` correctly mirrors events, run.json, and finishes a run.
//! - Re-running `poll_bucket_run` with the same `consumed` set does NOT
//!   double-append (idempotency).
//!
//! Task 8 coverage:
//! - Full dead-drop path: dispatch → atomic claim → node writes results → poller
//!   mirrors → central run Completed → control envelope queued.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use object_store::memory::InMemory;
use rupu_cp::host::bucket::{
    Bucket, BucketHostConnector, ControlEnvelope, ObjectStoreBucket, poll_bucket_run,
};
use rupu_cp::host::connector::HostConnector;
use rupu_cp::launcher::LaunchRequest;
use rupu_cp::node::protocol::{RunSpec, RunSpecKind};
use rupu_cp::node::NodeMirror;
use rupu_orchestrator::{RunStatus, RunStore, StepKind};
use tempfile::tempdir;

// ── poll_bucket_run ───────────────────────────────────────────────────────────

/// Seed a bucket run, poll once, assert events/status, then poll again with
/// the same `consumed` set and assert NO double-append.
#[tokio::test]
async fn poll_bucket_run_mirrors_and_finishes() {
    let dir = tempdir().unwrap();
    let store = Arc::new(RunStore::new(dir.path().join("runs")));
    let mirror = NodeMirror::new(Arc::clone(&store));

    let run_id = "run_BUCKETPOLL0001";
    let host_id = "host_BUCKET01";

    // Create the mirror run so the NodeMirror knows about it.
    let spec = RunSpec {
        kind: RunSpecKind::Workflow,
        name: "deploy".to_string(),
        inputs: BTreeMap::new(),
        prompt: None,
        mode: None,
        target: None,
    };
    mirror.create_run(run_id, host_id, &spec).expect("create_run");

    // Build an in-memory bucket (no cloud required).
    let bucket = ObjectStoreBucket::new(Arc::new(InMemory::new()), "test/host_BUCKET01");

    // ── Seed the bucket as the node would ────────────────────────────────────

    // events.0001.jsonl — one valid event line.
    let event_line = r#"{"type":"step_started","step_id":"step1"}"#;
    bucket
        .put_result(run_id, "events.0001.jsonl", event_line.as_bytes())
        .await
        .unwrap();

    // run.json — serialise the current mirror record (worker_id = host_id).
    let current_record = store.load(run_id).unwrap();
    let run_json_bytes = serde_json::to_vec(&current_record).unwrap();
    bucket
        .put_result(run_id, "run.json", &run_json_bytes)
        .await
        .unwrap();

    // finished marker — the node writes this last.
    bucket.put_finished(run_id, "completed").await.unwrap();

    // ── First poll ───────────────────────────────────────────────────────────

    let mut consumed = HashSet::new();
    let done = poll_bucket_run(&bucket, &mirror, host_id, run_id, &mut consumed)
        .await
        .expect("first poll must succeed");

    assert!(done, "poll must return true when finished marker is present");

    // events.jsonl must have exactly 1 line.
    let events_path = store.events_path(run_id);
    let content = std::fs::read_to_string(&events_path).expect("events.jsonl must exist");
    let line_count_after_first = content.lines().count();
    assert_eq!(
        line_count_after_first, 1,
        "events.jsonl must have exactly 1 line after first poll"
    );

    // Run must be Completed.
    let record = store.load(run_id).unwrap();
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "run must be Completed after first poll"
    );

    // ── Second poll (idempotency) ────────────────────────────────────────────
    // Re-use the SAME consumed set.  All result keys are already in it, so
    // no new lines must be appended, even though the bucket still has the
    // same objects and the finished marker is still present.

    let done2 = poll_bucket_run(&bucket, &mirror, host_id, run_id, &mut consumed)
        .await
        .expect("second poll must succeed");

    assert!(done2, "second poll must still return true (finished marker persists)");

    // Line count must be unchanged — no double-append.
    let content2 = std::fs::read_to_string(&events_path).unwrap();
    let line_count_after_second = content2.lines().count();
    assert_eq!(
        line_count_after_second, line_count_after_first,
        "second poll must NOT double-append: events.jsonl line count must be unchanged"
    );
}

// ── bucket_dead_drop_e2e ──────────────────────────────────────────────────────

/// Full dead-drop path end-to-end, using a SINGLE shared in-memory bucket:
///
/// 1. `BucketHostConnector::launch_run` → job envelope in bucket + mirror Running.
/// 2. Simulated node: atomic `claim_job` (true then false) + write results +
///    `put_finished`.
/// 3. `poll_bucket_run` → mirror Completed + event line in events.jsonl.
/// 4. `cancel_run` → control envelope queued in bucket (kind "cancel").
#[tokio::test]
async fn bucket_dead_drop_e2e() {
    // ── Shared infrastructure ─────────────────────────────────────────────────

    let dir = tempdir().unwrap();
    let run_store = Arc::new(RunStore::new(dir.path().join("runs")));
    let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));

    let host_id = "host_BUCKET_E2E";

    // One shared bucket — used by the connector (CP side), the simulated node,
    // and the poller.  They all read/write the same in-memory store.
    let shared_bucket: Arc<dyn Bucket> = Arc::new(ObjectStoreBucket::new(
        Arc::new(InMemory::new()),
        &format!("test/{host_id}"),
    ));

    // ── Step 1: dispatch via BucketHostConnector ──────────────────────────────

    let connector = BucketHostConnector::new(
        host_id,
        Arc::clone(&shared_bucket),
        Arc::clone(&mirror),
        Arc::clone(&run_store),
        rupu_config::PricingConfig::default(),
    );

    let run_id = connector
        .launch_run(LaunchRequest {
            workflow: "deploy".into(),
            inputs: BTreeMap::new(),
            mode: Some("bypass".into()),
            target: None,
            working_dir: None,
        })
        .await
        .expect("launch_run must succeed");

    // Job envelope is in the bucket and contains the workflow name.
    let job_bytes = shared_bucket
        .get_job(&run_id)
        .await
        .expect("job envelope must exist after launch_run");
    let job_spec: serde_json::Value =
        serde_json::from_slice(&job_bytes).expect("job envelope must be valid JSON");
    assert_eq!(
        job_spec["name"].as_str(),
        Some("deploy"),
        "job envelope must carry the workflow name"
    );
    assert_eq!(
        job_spec["kind"].as_str(),
        Some("workflow"),
        "job envelope kind must be 'workflow'"
    );

    // Central mirror run is Running.
    let record = run_store.load(&run_id).expect("mirror run must exist");
    assert_eq!(
        record.status,
        RunStatus::Running,
        "mirror run must be Running immediately after launch_run"
    );

    // ── Step 2: simulate the node claiming and executing ──────────────────────

    // First claim wins (atomic).
    let first_claim = shared_bucket
        .claim_job(&run_id, "node-x")
        .await
        .expect("claim_job must not error");
    assert!(first_claim, "first claim_job must return true");

    // Second claim loses — the claim marker is already present.
    let second_claim = shared_bucket
        .claim_job(&run_id, "node-y")
        .await
        .expect("second claim_job must not error");
    assert!(!second_claim, "second claim_job must return false (already claimed)");

    // Node writes a valid events.jsonl line — a proper executor::Event JSON.
    let event = rupu_orchestrator::executor::Event::StepStarted {
        run_id: run_id.clone(),
        step_id: "step1".into(),
        kind: StepKind::Linear,
        agent: Some("test-agent".into()),
    };
    let event_line =
        serde_json::to_string(&event).expect("Event::StepStarted must serialize");
    shared_bucket
        .put_result(&run_id, "events.0001.jsonl", event_line.as_bytes())
        .await
        .expect("put_result events must succeed");

    // Node writes run.json (the node's view of the run record — identity fields
    // will be re-pinned by the poller's mirror.append(RunJson) call).
    let node_record = run_store.load(&run_id).expect("run record must exist");
    let run_json_bytes =
        serde_json::to_vec(&node_record).expect("RunRecord must serialize to JSON");
    shared_bucket
        .put_result(&run_id, "run.json", &run_json_bytes)
        .await
        .expect("put_result run.json must succeed");

    // Node writes the finished marker last.
    shared_bucket
        .put_finished(&run_id, "completed")
        .await
        .expect("put_finished must succeed");

    // ── Step 3: CP poller mirrors results ─────────────────────────────────────

    let mut consumed = HashSet::new();
    let done = poll_bucket_run(
        shared_bucket.as_ref(),
        &mirror,
        host_id,
        &run_id,
        &mut consumed,
    )
    .await
    .expect("poll_bucket_run must succeed");

    assert!(
        done,
        "poll_bucket_run must return true when finished marker is present"
    );

    // Central run is now Completed.
    let record = run_store
        .load(&run_id)
        .expect("mirror run must still exist after poll");
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "mirror run must be Completed after poll_bucket_run"
    );

    // The event line is present in events.jsonl.
    let events_path = run_store.events_path(&run_id);
    let events_content =
        std::fs::read_to_string(&events_path).expect("events.jsonl must exist after poll");
    let event_lines: Vec<&str> = events_content.lines().collect();
    assert_eq!(
        event_lines.len(),
        1,
        "events.jsonl must have exactly 1 line after poll"
    );
    assert!(
        event_lines[0].contains("step_started"),
        "event line must contain 'step_started'"
    );

    // ── Step 4: control — queue a cancel envelope ─────────────────────────────

    connector
        .cancel_run(&run_id)
        .await
        .expect("cancel_run must succeed");

    let controls = shared_bucket
        .list_control(&run_id)
        .await
        .expect("list_control must succeed");
    assert!(
        !controls.is_empty(),
        "cancel_run must write a control envelope into the bucket"
    );
    let cancel_env: ControlEnvelope = serde_json::from_slice(&controls[0].1)
        .expect("control envelope must be valid JSON");
    assert_eq!(
        cancel_env.kind, "cancel",
        "control envelope kind must be 'cancel'"
    );
    assert!(
        cancel_env.mode.is_none(),
        "cancel envelope must not carry a mode"
    );
    assert!(
        cancel_env.reason.is_none(),
        "cancel envelope must not carry a reason"
    );
}
