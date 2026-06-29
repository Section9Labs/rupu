//! End-to-end tests for the bucket transport (Slice 2b).
//!
//! Task 5 coverage (this file, shared with Task 8):
//! - `poll_bucket_run` correctly mirrors events, run.json, and finishes a run.
//! - Re-running `poll_bucket_run` with the same `consumed` set does NOT
//!   double-append (idempotency).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use object_store::memory::InMemory;
use rupu_cp::host::bucket::{Bucket, ObjectStoreBucket, poll_bucket_run};
use rupu_cp::node::protocol::{RunSpec, RunSpecKind};
use rupu_cp::node::NodeMirror;
use rupu_orchestrator::{RunStatus, RunStore};
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
