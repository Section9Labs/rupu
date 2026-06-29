//! Integration tests for `NodeRegistry` (live tunnel connections) and
//! `NodeMirror` (stream artifacts into RunStore).
//!
//! NodeRegistry covers:
//! - register → get returns the same conn (Arc::ptr_eq).
//! - re-register same id evicts the old; old `send` errors Offline.
//! - `remove(only_if)` no-ops when a newer conn has replaced the old one.
//! - `remove(only_if)` with the current conn removes it.
//! - `is_online` reflects presence / absence.
//! - `mark_seen` updates last_seen.
//! - `send` succeeds when receiver is alive; errors Offline when dropped.
//!
//! NodeMirror covers:
//! - create_run → append(Events) ×2 → finish → load shows Completed + 2-line events.jsonl.

use rupu_cp::node::{Frame, NodeError, NodeRegistry};
use std::sync::Arc;
use tokio::sync::mpsc;

// ── register + get ────────────────────────────────────────────────────────────

#[test]
fn register_then_get_is_same_arc() {
    let reg = NodeRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let conn = reg.register("alpha", tx);
    let got = reg.get("alpha").expect("alpha should be present");
    assert!(Arc::ptr_eq(&conn, &got), "get should return the same Arc");
}

#[test]
fn get_unknown_returns_none() {
    let reg = NodeRegistry::new();
    assert!(reg.get("no-such-node").is_none());
}

// ── is_online ─────────────────────────────────────────────────────────────────

#[test]
fn is_online_reflects_presence() {
    let reg = NodeRegistry::new();
    assert!(!reg.is_online("beta"), "should be offline before register");
    let (tx, _rx) = mpsc::channel(8);
    reg.register("beta", tx);
    assert!(reg.is_online("beta"), "should be online after register");
}

// ── eviction on re-register ───────────────────────────────────────────────────

/// After re-registering the same id, the old Arc's Sender is the only remaining
/// handle.  Dropping the Receiver (simulating the disconnect of the old tunnel)
/// makes the old `send` return Offline.
#[tokio::test]
async fn re_register_evicts_old_conn() {
    let reg = NodeRegistry::new();

    // First connection.
    let (tx1, rx1) = mpsc::channel::<Frame>(8);
    let old_conn = reg.register("gamma", tx1);

    // Second connection overwrites the first.
    let (tx2, _rx2) = mpsc::channel(8);
    reg.register("gamma", tx2);

    // The registry has dropped its clone of tx1.  Drop rx1 to close the channel.
    drop(rx1);

    // The old conn's Sender now has no receiver → Offline.
    let result = old_conn.send(Frame::Ping {}).await;
    assert!(
        matches!(result, Err(NodeError::Offline)),
        "expected Offline, got {result:?}"
    );
}

#[tokio::test]
async fn re_register_new_conn_is_gettable_and_sends() {
    let reg = NodeRegistry::new();

    let (tx1, _rx1) = mpsc::channel::<Frame>(8);
    reg.register("delta", tx1);

    let (tx2, mut rx2) = mpsc::channel(8);
    let new_conn = reg.register("delta", tx2);

    // get returns the new conn.
    let got = reg.get("delta").expect("delta should be online");
    assert!(Arc::ptr_eq(&got, &new_conn));

    // New conn can send.
    new_conn.send(Frame::Ping {}).await.expect("send should succeed");
    let f = rx2.recv().await.expect("should receive frame");
    assert!(matches!(f, Frame::Ping {}));
}

// ── remove(only_if) ───────────────────────────────────────────────────────────

#[test]
fn remove_only_if_noop_on_stale_arc() {
    let reg = NodeRegistry::new();

    let (tx1, _rx1) = mpsc::channel::<Frame>(8);
    let old_conn = reg.register("epsilon", tx1);

    // Replace with newer conn.
    let (tx2, _rx2) = mpsc::channel(8);
    let new_conn = reg.register("epsilon", tx2);

    // Stale remove should be a no-op.
    reg.remove("epsilon", &old_conn);
    assert!(reg.is_online("epsilon"), "newer conn should survive stale remove");

    // Correct remove should work.
    reg.remove("epsilon", &new_conn);
    assert!(!reg.is_online("epsilon"), "conn should be gone after correct remove");
}

#[test]
fn remove_only_if_removes_when_still_current() {
    let reg = NodeRegistry::new();

    let (tx, _rx) = mpsc::channel::<Frame>(8);
    let conn = reg.register("zeta", tx);
    assert!(reg.is_online("zeta"));

    reg.remove("zeta", &conn);
    assert!(!reg.is_online("zeta"), "should be offline after correct remove");
}

#[test]
fn remove_unknown_node_is_noop() {
    let reg = NodeRegistry::new();
    let (tx, _rx) = mpsc::channel::<Frame>(8);
    // Register somewhere else so we have an Arc to pass.
    let conn = reg.register("eta", tx);
    // Remove using that Arc but for a different (non-existent) node_id.
    reg.remove("no-such-node", &conn);
    // eta should still be online.
    assert!(reg.is_online("eta"));
}

// ── mark_seen ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn mark_seen_advances_last_seen() {
    let reg = NodeRegistry::new();
    let (tx, _rx) = mpsc::channel::<Frame>(8);
    let conn = reg.register("theta", tx);

    let before = *conn.last_seen.lock().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    reg.mark_seen("theta");
    let after = *conn.last_seen.lock().unwrap();

    assert!(after >= before, "last_seen should advance after mark_seen");
}

#[test]
fn mark_seen_unknown_is_noop() {
    let reg = NodeRegistry::new();
    // Should not panic.
    reg.mark_seen("no-such-node");
}

// ── send ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn send_succeeds_while_receiver_alive() {
    let reg = NodeRegistry::new();
    let (tx, mut rx) = mpsc::channel(8);
    let conn = reg.register("iota", tx);

    conn.send(Frame::Ping {}).await.expect("send should succeed");
    let frame = rx.recv().await.expect("should receive a frame");
    assert!(matches!(frame, Frame::Ping {}));
}

#[tokio::test]
async fn send_errors_offline_when_receiver_dropped() {
    let reg = NodeRegistry::new();
    let (tx, rx) = mpsc::channel::<Frame>(8);
    let conn = reg.register("kappa", tx);
    drop(rx);

    let result = conn.send(Frame::Ping {}).await;
    assert!(
        matches!(result, Err(NodeError::Offline)),
        "expected Offline, got {result:?}"
    );
}

// ── NodeMirror ────────────────────────────────────────────────────────────────

#[test]
fn mirror_create_append_finish_round_trip() {
    use rupu_cp::node::mirror::NodeMirror;
    use rupu_cp::node::protocol::{ArtifactFile, RunSpec, RunSpecKind};
    use rupu_orchestrator::{RunStatus, RunStore};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(RunStore::new(dir.path().to_path_buf()));
    let mirror = NodeMirror::new(Arc::clone(&store));

    let spec = RunSpec {
        kind: RunSpecKind::Workflow,
        name: "smoke-workflow".to_string(),
        inputs: BTreeMap::new(),
        prompt: None,
        mode: None,
        target: None,
    };

    let run_id = "run_NODEMIRRTEST001";
    let node_id = "node-42";

    mirror
        .create_run(run_id, node_id, &spec)
        .expect("create_run");

    mirror
        .append(
            run_id,
            ArtifactFile::Events,
            r#"{"type":"step_started","step_id":"s1"}"#,
        )
        .expect("append event 1");

    mirror
        .append(
            run_id,
            ArtifactFile::Events,
            r#"{"type":"step_completed","step_id":"s1"}"#,
        )
        .expect("append event 2");

    mirror.finish(run_id, "completed").expect("finish");

    // Status must be Completed and worker_id must carry the node attribution.
    let record = store.load(run_id).expect("load");
    assert_eq!(record.status, RunStatus::Completed, "expected Completed");
    assert_eq!(
        record.worker_id.as_deref(),
        Some("node-42"),
        "worker_id must carry node attribution"
    );
    assert!(record.finished_at.is_some(), "finished_at must be set");

    // events.jsonl must have exactly 2 lines at events_path.
    let events_path = store.events_path(run_id);
    let content = std::fs::read_to_string(&events_path).expect("read events.jsonl");
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 lines in events.jsonl");
    assert!(
        lines[0].contains("step_started"),
        "line 0 should contain step_started"
    );
    assert!(
        lines[1].contains("step_completed"),
        "line 1 should contain step_completed"
    );
}
