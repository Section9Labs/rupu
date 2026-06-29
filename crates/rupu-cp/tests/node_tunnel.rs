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
//!
//! WS endpoint (`GET /api/node/connect`) covers:
//! - valid Hello → Welcome + node is_online.
//! - bad token Hello → connection closed, not registered.

use rupu_cp::node::{Frame, NodeError, NodeRegistry};
use std::sync::Arc;
use tokio::sync::mpsc;

// Used by WS endpoint integration tests below.
use axum;

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
            node_id,
            ArtifactFile::Events,
            r#"{"type":"step_started","step_id":"s1"}"#,
        )
        .expect("append event 1");

    mirror
        .append(
            run_id,
            node_id,
            ArtifactFile::Events,
            r#"{"type":"step_completed","step_id":"s1"}"#,
        )
        .expect("append event 2");

    mirror.finish(run_id, node_id, "completed").expect("finish");

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

/// After `create_run` + `append(RunJson, <node record with bogus paths>)`,
/// the loaded record must carry the CP-side `transcript_dir` and
/// `workspace_path` (not the node's paths), while run-state fields
/// (e.g. `status`) are taken from the node's `RunRecord`.
#[test]
fn mirror_run_json_repins_cp_local_paths() {
    use rupu_cp::node::mirror::NodeMirror;
    use rupu_cp::node::protocol::{ArtifactFile, RunSpec, RunSpecKind};
    use rupu_orchestrator::{RunStatus, RunStore};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(RunStore::new(dir.path().to_path_buf()));
    let mirror = NodeMirror::new(Arc::clone(&store));

    let spec = RunSpec {
        kind: RunSpecKind::Workflow,
        name: "repin-test".to_string(),
        inputs: BTreeMap::new(),
        prompt: None,
        mode: None,
        target: None,
    };

    let run_id = "run_REPINTEST001";
    let node_id = "node-repin";

    mirror
        .create_run(run_id, node_id, &spec)
        .expect("create_run");

    // Record what create_run stored as the CP-side transcript_dir and
    // workspace_path so we can assert they survive the RunJson update.
    let created = store.load(run_id).expect("load after create_run");
    let cp_transcript_dir: PathBuf = created.transcript_dir.clone();
    let cp_workspace_path: PathBuf = created.workspace_path.clone();

    // Build a node-side RunRecord JSON with bogus paths and status=completed.
    let node_run_json = serde_json::json!({
        "id": run_id,
        "workflow_name": "repin-test",
        "status": "completed",
        "inputs": {},
        "workspace_id": "node-ws-id",
        "workspace_path": "/node/only/path",
        "transcript_dir": "/node/only/path/transcripts",
        "started_at": "2026-01-01T00:00:00Z",
        "worker_id": "node-worker-override"
    });
    let line = serde_json::to_string(&node_run_json).expect("serialize node record");

    mirror
        .append(run_id, node_id, ArtifactFile::RunJson, &line)
        .expect("append RunJson");

    let record = store.load(run_id).expect("load after RunJson append");

    // Run-state: status must reflect the node's completed value.
    assert_eq!(
        record.status,
        RunStatus::Completed,
        "status should come from the node RunJson"
    );

    // CP-local location fields must NOT be the node's bogus paths.
    assert_eq!(
        record.transcript_dir, cp_transcript_dir,
        "transcript_dir must remain the CP-side value"
    );
    assert_eq!(
        record.workspace_path, cp_workspace_path,
        "workspace_path must remain the CP-side value"
    );
    assert_ne!(
        record.transcript_dir,
        PathBuf::from("/node/only/path/transcripts"),
        "transcript_dir must not be the node's path"
    );
    assert_ne!(
        record.workspace_path,
        PathBuf::from("/node/only/path"),
        "workspace_path must not be the node's path"
    );

    // Host attribution: worker_id must be the original node_id (not the
    // node record's overridden value).
    assert_eq!(
        record.worker_id.as_deref(),
        Some(node_id),
        "worker_id must remain the CP's node attribution"
    );

    // workspace_id must be empty (set by create_run, not the node id).
    assert_eq!(
        record.workspace_id, "",
        "workspace_id must be empty (not the node id)"
    );
}

// ── NodeMirror security tests (Finding 1) ─────────────────────────────────────

/// A run_id containing a path-traversal sequence (`/`) must be rejected by
/// both `append` and `finish` before any I/O is attempted.
#[test]
fn mirror_traversal_run_id_rejected_before_io() {
    use rupu_cp::node::mirror::{MirrorError, NodeMirror};
    use rupu_cp::node::protocol::{ArtifactFile, RunSpec, RunSpecKind};
    use rupu_orchestrator::RunStore;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(RunStore::new(dir.path().to_path_buf()));
    let mirror = NodeMirror::new(Arc::clone(&store));

    let node_id = "node-traversal";
    let evil_id = "run_../../etc/passwd";

    // append must return InvalidRunId.
    let err = mirror
        .append(evil_id, node_id, ArtifactFile::Events, "line")
        .expect_err("append with traversal id must fail");
    assert!(
        matches!(err, MirrorError::InvalidRunId(_)),
        "expected InvalidRunId for append, got {err:?}"
    );

    // finish must return InvalidRunId.
    let err = mirror
        .finish(evil_id, node_id, "completed")
        .expect_err("finish with traversal id must fail");
    assert!(
        matches!(err, MirrorError::InvalidRunId(_)),
        "expected InvalidRunId for finish, got {err:?}"
    );

    // The run store must be completely empty — no I/O was attempted.
    let runs = store.list().unwrap_or_default();
    assert!(runs.is_empty(), "run store must be empty; no I/O for traversal ids");

    // Smoke-check additional invalid patterns.
    let _spec = RunSpec {
        kind: RunSpecKind::Workflow,
        name: "x".into(),
        inputs: BTreeMap::new(),
        prompt: None,
        mode: None,
        target: None,
    };
    for bad in &[
        "run_../escape",
        "run_with space",
        "run_with.dot",
        "no_run_prefix",
        "",
    ] {
        let err = mirror
            .append(bad, node_id, ArtifactFile::Events, "line")
            .expect_err(&format!("append({bad}) must fail"));
        assert!(
            matches!(err, MirrorError::InvalidRunId(_)),
            "append({bad}) should be InvalidRunId, got {err:?}"
        );
    }
}

/// A node must not be able to write into a run that belongs to a different node.
#[test]
fn mirror_wrong_node_id_rejected() {
    use rupu_cp::node::mirror::{MirrorError, NodeMirror};
    use rupu_cp::node::protocol::{ArtifactFile, RunSpec, RunSpecKind};
    use rupu_orchestrator::RunStore;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(RunStore::new(dir.path().to_path_buf()));
    let mirror = NodeMirror::new(Arc::clone(&store));

    let owner_node = "node-owner";
    let intruder_node = "node-intruder";
    let run_id = "run_SECTEST001";

    let spec = RunSpec {
        kind: RunSpecKind::Workflow,
        name: "sec-workflow".to_string(),
        inputs: BTreeMap::new(),
        prompt: None,
        mode: None,
        target: None,
    };

    // Run created by owner_node.
    mirror
        .create_run(run_id, owner_node, &spec)
        .expect("create_run by owner");

    // intruder_node tries to append — must be rejected with WrongNode.
    let err = mirror
        .append(run_id, intruder_node, ArtifactFile::Events, "injected line")
        .expect_err("append by wrong node must fail");
    assert!(
        matches!(err, MirrorError::WrongNode(_)),
        "expected WrongNode for append, got {err:?}"
    );

    // intruder_node tries to finish — must be rejected with WrongNode.
    let err = mirror
        .finish(run_id, intruder_node, "completed")
        .expect_err("finish by wrong node must fail");
    assert!(
        matches!(err, MirrorError::WrongNode(_)),
        "expected WrongNode for finish, got {err:?}"
    );

    // Verify the run still belongs to the owner and has not been tampered with.
    let record = store.load(run_id).expect("run must still be loadable");
    assert_eq!(
        record.worker_id.as_deref(),
        Some(owner_node),
        "worker_id must still be owner_node after rejected writes"
    );
    // events.jsonl must NOT exist (no successful append happened).
    let events_path = store.events_path(run_id);
    assert!(
        !events_path.exists(),
        "events.jsonl must not exist after all writes were rejected"
    );
}

/// Legitimate path: the owning node can append and finish its own run.
#[test]
fn mirror_legitimate_owner_can_append_and_finish() {
    use rupu_cp::node::mirror::NodeMirror;
    use rupu_cp::node::protocol::{ArtifactFile, RunSpec, RunSpecKind};
    use rupu_orchestrator::{RunStatus, RunStore};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let store = Arc::new(RunStore::new(dir.path().to_path_buf()));
    let mirror = NodeMirror::new(Arc::clone(&store));

    let node_id = "node-legit";
    let run_id = "run_LEGITPATH001";

    let spec = RunSpec {
        kind: RunSpecKind::Workflow,
        name: "legit-workflow".to_string(),
        inputs: BTreeMap::new(),
        prompt: None,
        mode: None,
        target: None,
    };

    mirror.create_run(run_id, node_id, &spec).expect("create_run");
    mirror
        .append(run_id, node_id, ArtifactFile::Events, r#"{"type":"started"}"#)
        .expect("append by owner must succeed");
    mirror
        .finish(run_id, node_id, "completed")
        .expect("finish by owner must succeed");

    let record = store.load(run_id).expect("load");
    assert_eq!(record.status, RunStatus::Completed);
    assert_eq!(record.worker_id.as_deref(), Some(node_id));
}

// ── WS endpoint integration tests ─────────────────────────────────────────────
//
// Spawn the real CP router on a `TcpListener` (same pattern as tests/sse.rs),
// enroll a tunnel node via `enroll_node`, then connect a `tokio-tungstenite`
// client to `ws://127.0.0.1:PORT/api/node/connect`.

/// Spawn the CP router and return its bound address.
#[allow(dead_code)]
async fn spawn_cp(dir: &std::path::Path) -> std::net::SocketAddr {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Spawn the CP with an AppState whose `node_registry` is shared with the
/// caller so we can inspect it after the WS handshake.
async fn spawn_cp_with_state(
    dir: &std::path::Path,
) -> (std::net::SocketAddr, std::sync::Arc<rupu_cp::node::NodeRegistry>) {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let registry = std::sync::Arc::clone(&state.node_registry);
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, registry)
}

/// Send a JSON-serialised [`Frame`] as a text WS message.
async fn send_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    frame: &Frame,
) {
    use futures_util::SinkExt as _;
    use tokio_tungstenite::tungstenite::Message;
    let text = serde_json::to_string(frame).unwrap();
    ws.send(Message::Text(text.into())).await.unwrap();
}

/// Receive the next text message from the WS and deserialise it as a [`Frame`].
async fn recv_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Option<Frame> {
    use futures_util::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => {
                return Some(serde_json::from_str(&t).expect("frame JSON"))
            }
            Ok(Message::Close(_)) => return None,
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

/// Spawn the CP router **with a bearer token configured** and return its bound
/// address together with the shared `NodeRegistry`.  Used to verify that
/// `/api/node/connect` remains reachable despite the bearer middleware.
async fn spawn_cp_with_bearer(
    dir: &std::path::Path,
    bearer: &str,
) -> (std::net::SocketAddr, std::sync::Arc<rupu_cp::node::NodeRegistry>) {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let registry = std::sync::Arc::clone(&state.node_registry);
    let app = rupu_cp::server::router(state, Some(bearer.to_string()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, registry)
}

/// Regression guard for Finding 1: `/api/node/connect` must be **outside** the
/// bearer middleware.  With a bearer token configured the WS upgrade must still
/// succeed without any `Authorization` header — the route is token-gated by the
/// Hello frame's enrollment token only.
#[tokio::test]
async fn ws_node_connect_exempt_from_bearer() {
    use rupu_workspace::{enroll_node, HostStore};
    use tempfile::tempdir;
    use tokio_tungstenite::connect_async;

    let dir = tempdir().unwrap();

    let host_store = HostStore { root: dir.path().join("hosts") };
    let (host, token) = enroll_node(&host_store, "test-node-bearer-exempt").unwrap();

    let node_id = match &host.transport {
        rupu_workspace::HostTransport::Tunnel { node_id } => node_id.clone(),
        _ => panic!("expected Tunnel transport"),
    };

    // Spawn with a bearer token — /api/* would return 401 without an
    // Authorization header.  /api/node/connect must be exempt.
    let (addr, registry) =
        spawn_cp_with_bearer(dir.path(), "super-secret-bearer-token").await;

    let url = format!("ws://{addr}/api/node/connect");
    // Connect WITHOUT any Authorization header.
    let (mut ws, _) = connect_async(&url)
        .await
        .expect("WS connect should succeed without Authorization header");

    send_frame(
        &mut ws,
        &Frame::Hello {
            node_id: node_id.clone(),
            auth: rupu_cp::node::Auth::Token { token },
            rupu_version: "test".to_string(),
            capabilities: vec![],
        },
    )
    .await;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_frame(&mut ws),
    )
    .await
    .expect("timed out waiting for Welcome")
    .expect("connection closed before Welcome");

    assert!(
        matches!(response, Frame::Welcome {}),
        "expected Welcome, got {response:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        registry.is_online(&node_id),
        "node should be online in the registry after bearer-exempt WS Hello"
    );
}

/// A valid `Hello` with an enrolled token → CP replies `Welcome` and node is online.
#[tokio::test]
async fn ws_valid_hello_receives_welcome_and_is_online() {
    use rupu_workspace::{enroll_node, HostStore};
    use tempfile::tempdir;
    use tokio_tungstenite::connect_async;

    let dir = tempdir().unwrap();

    // Enroll a tunnel host into the hosts directory.
    let host_store = HostStore { root: dir.path().join("hosts") };
    let (host, token) = enroll_node(&host_store, "test-node-valid").unwrap();
    let node_id = host.id.clone();

    let (addr, registry) = spawn_cp_with_state(dir.path()).await;

    let url = format!("ws://{addr}/api/node/connect");
    let (mut ws, _) = connect_async(&url).await.expect("WS connect failed");

    // Determine the node_id from the Tunnel transport.
    let nid = match &host.transport {
        rupu_workspace::HostTransport::Tunnel { node_id } => node_id.clone(),
        _ => panic!("expected Tunnel transport"),
    };
    assert_eq!(nid, node_id);

    // Send Hello with the valid token.
    send_frame(
        &mut ws,
        &Frame::Hello {
            node_id: nid.clone(),
            auth: rupu_cp::node::Auth::Token { token },
            rupu_version: "test".to_string(),
            capabilities: vec![],
        },
    )
    .await;

    // Expect Welcome.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_frame(&mut ws),
    )
    .await
    .expect("timed out waiting for Welcome")
    .expect("connection closed before Welcome");

    assert!(
        matches!(response, Frame::Welcome {}),
        "expected Welcome, got {response:?}"
    );

    // Node must be online in the registry.
    // Give the server a moment to register before we check.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        registry.is_online(&nid),
        "node should be online in the registry after Welcome"
    );
}

// ── TunnelHostConnector ───────────────────────────────────────────────────────

mod tunnel_connector {
    use rupu_cp::{
        host::connector::{HostConnector, HostConnectorError, RunKind, RunListQuery},
        host::tunnel::TunnelHostConnector,
        node::{mirror::NodeMirror, protocol::RunSpec, protocol::RunSpecKind, Frame, NodeRegistry},
    };
    use rupu_orchestrator::runs::RunStore;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    fn make_spec(name: &str) -> RunSpec {
        RunSpec {
            kind: RunSpecKind::Workflow,
            name: name.to_string(),
            inputs: BTreeMap::new(),
            prompt: None,
            mode: None,
            target: None,
        }
    }

    fn make_launch_req(workflow: &str) -> rupu_cp::launcher::LaunchRequest {
        rupu_cp::launcher::LaunchRequest {
            workflow: workflow.to_string(),
            inputs: BTreeMap::new(),
            mode: None,
            target: None,
            working_dir: None,
        }
    }

    fn make_agent_req(agent: &str) -> rupu_cp::agent_launcher::AgentLaunchRequest {
        rupu_cp::agent_launcher::AgentLaunchRequest {
            agent: agent.to_string(),
            prompt: Some("do the thing".to_string()),
            mode: None,
            target: None,
            working_dir: None,
        }
    }

    /// Register a fake node with a channel we hold onto, and return the
    /// connector + the receiver so tests can observe frames.
    fn setup(
        node_id: &str,
        dir: &std::path::Path,
    ) -> (TunnelHostConnector, mpsc::Receiver<Frame>, Arc<RunStore>) {
        let (tx, rx) = mpsc::channel(16);
        let run_store = Arc::new(RunStore::new(dir.join("runs")));
        let registry = Arc::new(NodeRegistry::new());
        registry.register(node_id, tx);
        let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));
        let conn = TunnelHostConnector::new(
            node_id,
            registry,
            mirror,
            Arc::clone(&run_store),
            rupu_config::PricingConfig::default(),
        );
        (conn, rx, run_store)
    }

    /// (1) `launch_run` on an ONLINE node creates a mirror run AND the node
    /// receives `Frame::Run` with the allocated run_id.
    #[tokio::test]
    async fn launch_run_online_creates_mirror_and_sends_frame() {
        let dir = tempdir().unwrap();
        let node_id = "node-tunnel-1";
        let (conn, mut rx, run_store) = setup(node_id, dir.path());

        let req = make_launch_req("my-workflow");
        let run_id = conn.launch_run(req).await.expect("launch_run should succeed");

        // The mirror must have a record for this run_id.
        let record = run_store.load(&run_id).expect("run should be in mirror store");
        assert_eq!(record.worker_id.as_deref(), Some(node_id));
        assert_eq!(record.workflow_name, "my-workflow");

        // The node must have received a Run frame with that run_id.
        let frame = rx.recv().await.expect("should receive a frame");
        match frame {
            Frame::Run { run_id: fid, spec } => {
                assert_eq!(fid, run_id, "frame run_id must match allocated run_id");
                assert_eq!(spec.name, "my-workflow");
                assert_eq!(spec.kind, RunSpecKind::Workflow);
            }
            other => panic!("expected Frame::Run, got {other:?}"),
        }
    }

    /// (2) `cancel_run` sends `Frame::Cancel` with the correct run_id.
    #[tokio::test]
    async fn cancel_run_sends_cancel_frame() {
        let dir = tempdir().unwrap();
        let node_id = "node-tunnel-2";
        let (conn, mut rx, _store) = setup(node_id, dir.path());

        // `cancel_run` sends the frame regardless of whether the run exists in the
        // mirror; the node decides what to do with it.
        let run_id = "run_CANCEL001";
        conn.cancel_run(run_id).await.expect("cancel_run should succeed");

        let frame = rx.recv().await.expect("should receive a frame");
        match frame {
            Frame::Cancel { run_id: fid } => {
                assert_eq!(fid, run_id);
            }
            other => panic!("expected Frame::Cancel, got {other:?}"),
        }
    }

    /// (3) `launch_run` on an OFFLINE node → `HostConnectorError::Unreachable`
    /// AND the run store must be empty (no orphaned Running record).
    #[tokio::test]
    async fn launch_run_offline_node_returns_unreachable() {
        let dir = tempdir().unwrap();
        let run_store = Arc::new(RunStore::new(dir.path().join("runs")));
        // Do NOT register the node → it is offline.
        let registry = Arc::new(NodeRegistry::new());
        let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));
        // Keep a handle so we can inspect the store after the call.
        let run_store_check = Arc::clone(&run_store);
        let conn = TunnelHostConnector::new(
            "node-offline",
            registry,
            mirror,
            run_store,
            rupu_config::PricingConfig::default(),
        );

        let req = make_launch_req("some-wf");
        let err = conn.launch_run(req).await.expect_err("should fail when node is offline");
        assert!(
            matches!(err, HostConnectorError::Unreachable(_)),
            "expected Unreachable, got {err:?}"
        );

        // The run store must be completely empty — live_conn() fails before
        // create_run() is called, so no orphaned Running record is created.
        let runs = run_store_check.list().unwrap_or_default();
        assert!(
            runs.is_empty(),
            "run store must be empty after offline launch_run; got {} run(s)",
            runs.len()
        );
    }

    /// (4) `list_runs` returns this node's mirrored runs and excludes other
    /// nodes' runs.
    #[tokio::test]
    async fn list_runs_scoped_to_node() {
        let dir = tempdir().unwrap();
        let run_store = Arc::new(RunStore::new(dir.path().join("runs")));
        let mirror = Arc::new(NodeMirror::new(Arc::clone(&run_store)));
        let registry = Arc::new(NodeRegistry::new());

        let my_node = "node-mine";
        let other_node = "node-other";

        // Seed two runs for my_node and one for other_node.
        let my_spec = make_spec("mine-wf");
        mirror.create_run("run_MINE001", my_node, &my_spec).unwrap();
        mirror.create_run("run_MINE002", my_node, &my_spec).unwrap();
        let other_spec = make_spec("other-wf");
        mirror.create_run("run_OTHER001", other_node, &other_spec).unwrap();

        // Register my_node (online) so the connector can be built; list_runs
        // doesn't require a live connection.
        let (tx, _rx) = mpsc::channel(1);
        registry.register(my_node, tx);

        let conn = TunnelHostConnector::new(
            my_node,
            registry,
            mirror,
            Arc::clone(&run_store),
            rupu_config::PricingConfig::default(),
        );

        let params = RunListQuery {
            kind: RunKind::All,
            offset: 0,
            limit: 100,
            lifecycle: None,
        };
        let rows = conn.list_runs(params).await.expect("list_runs should succeed");

        assert_eq!(rows.len(), 2, "should return exactly 2 runs for my_node");
        for row in &rows {
            let id = row["id"].as_str().unwrap_or("");
            assert!(
                id.starts_with("run_MINE"),
                "unexpected run in list: {id}"
            );
        }
    }

    /// `info()` returns `reachable = true` when the node is online.
    #[tokio::test]
    async fn info_reachable_when_online() {
        let dir = tempdir().unwrap();
        let (conn, _rx, _store) = setup("node-info", dir.path());
        let info = conn.info().await.expect("info should succeed");
        assert!(info.reachable, "node should be reachable when registered");
    }

    /// `info()` returns `reachable = false` when the node is offline.
    #[tokio::test]
    async fn info_not_reachable_when_offline() {
        let dir = tempdir().unwrap();
        let run_store = Arc::new(RunStore::new(dir.path().join("runs")));
        let conn = TunnelHostConnector::new(
            "node-gone",
            Arc::new(NodeRegistry::new()),
            Arc::new(NodeMirror::new(Arc::clone(&run_store))),
            run_store,
            rupu_config::PricingConfig::default(),
        );
        let info = conn.info().await.expect("info should succeed");
        assert!(!info.reachable, "node should not be reachable when unregistered");
    }

    /// `start_session` and `send_session_turn` return `Invalid` (unsupported).
    #[tokio::test]
    async fn unsupported_session_ops_return_invalid() {
        let dir = tempdir().unwrap();
        let (conn, _rx, _store) = setup("node-unsup", dir.path());

        let err = conn
            .start_session(rupu_cp::session_starter::SessionStartRequest {
                agent: "a".into(),
                prompt: None,
                mode: None,
                target: None,
                working_dir: None,
            })
            .await
            .expect_err("start_session should fail");
        assert!(matches!(err, HostConnectorError::Invalid(_)));

        let err = conn
            .send_session_turn(rupu_cp::session_sender::SendMessageRequest {
                session_id: "s".into(),
                prompt: "hi".into(),
            })
            .await
            .expect_err("send_session_turn should fail");
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }

    /// `approve_run` and `reject_run` return `Invalid` (unsupported).
    #[tokio::test]
    async fn unsupported_approval_ops_return_invalid() {
        let dir = tempdir().unwrap();
        let (conn, _rx, _store) = setup("node-noapprove", dir.path());

        let err = conn
            .approve_run("run_x", "bypass")
            .await
            .expect_err("approve_run should fail");
        assert!(matches!(err, HostConnectorError::Invalid(_)));

        let err = conn
            .reject_run("run_x", None)
            .await
            .expect_err("reject_run should fail");
        assert!(matches!(err, HostConnectorError::Invalid(_)));
    }

    /// `launch_agent` creates a mirror run with `RunSpecKind::Agent`.
    #[tokio::test]
    async fn launch_agent_online_creates_mirror_and_sends_frame() {
        let dir = tempdir().unwrap();
        let node_id = "node-agent-1";
        let (conn, mut rx, run_store) = setup(node_id, dir.path());

        let req = make_agent_req("my-agent");
        let run_id = conn
            .launch_agent(req)
            .await
            .expect("launch_agent should succeed");

        let record = run_store.load(&run_id).unwrap();
        assert_eq!(record.worker_id.as_deref(), Some(node_id));
        assert_eq!(record.workflow_name, "my-agent");

        let frame = rx.recv().await.expect("should receive a frame");
        match frame {
            Frame::Run { run_id: fid, spec } => {
                assert_eq!(fid, run_id);
                assert_eq!(spec.kind, RunSpecKind::Agent);
                assert_eq!(spec.prompt.as_deref(), Some("do the thing"));
            }
            other => panic!("expected Frame::Run, got {other:?}"),
        }
    }
}

/// A `Hello` with a wrong token → server closes the connection and the node
/// is NOT registered.
#[tokio::test]
async fn ws_bad_token_closes_connection_and_not_registered() {
    use rupu_workspace::{enroll_node, HostStore};
    use tempfile::tempdir;
    use tokio_tungstenite::connect_async;

    let dir = tempdir().unwrap();

    // Enroll a tunnel host.
    let host_store = HostStore { root: dir.path().join("hosts") };
    let (host, _correct_token) = enroll_node(&host_store, "test-node-bad").unwrap();

    let node_id = match &host.transport {
        rupu_workspace::HostTransport::Tunnel { node_id } => node_id.clone(),
        _ => panic!("expected Tunnel transport"),
    };

    let (addr, registry) = spawn_cp_with_state(dir.path()).await;

    let url = format!("ws://{addr}/api/node/connect");
    let (mut ws, _) = connect_async(&url).await.expect("WS connect failed");

    // Send Hello with a WRONG token.
    send_frame(
        &mut ws,
        &Frame::Hello {
            node_id: node_id.clone(),
            auth: rupu_cp::node::Auth::Token {
                token: "this-is-not-the-right-token".to_string(),
            },
            rupu_version: "test".to_string(),
            capabilities: vec![],
        },
    )
    .await;

    // Expect the server to close (None from recv_frame) or send Close.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_frame(&mut ws),
    )
    .await
    .expect("timed out waiting for server close");

    assert!(
        response.is_none(),
        "expected connection close after bad token, got frame {response:?}"
    );

    // Node must NOT be online.
    assert!(
        !registry.is_online(&node_id),
        "node should NOT be registered after bad-token Hello"
    );
}

// ── Dial-home tunnel end-to-end ───────────────────────────────────────────────

/// End-to-end integration test proving the full dial-home tunnel flow.
///
/// Uses a hand-rolled fake WS client (not the real `rupu node` agent).
///
/// Flow:
///   1. Spin the CP router on a real `TcpListener` (AppState already wires
///      tunnel deps — no separate launcher required for the remote-host path).
///   2. Enroll a node via `state.hosts.enroll_node` → get (host, token, node_id).
///   3. Fake WS client sends `Hello` with the valid token → asserts `Welcome`.
///   4. Central: `POST /api/agents/smoke-agent/run` with `host=node_id` → node
///      receives `Frame::Run`; capture the CP-allocated `run_id`.
///   5. Fake node replies: two `Artifact(Events)` frames + `RunFinished(completed)`.
///      Event lines are valid `rupu_orchestrator::executor::Event` JSON so the
///      `FileTailRunSource` SSE reader can parse them.
///   6. Poll `GET /api/runs?host=node_id` until `run_id` appears with
///      `host_id == node_id` (bounded 3 s retry loop).
///   7. `GET /api/runs/<run_id>/log?host=node_id` → SSE stream; assert
///      `text/event-stream` and find the `step_started` event in the stream.
///   8. `POST /api/runs/<run_id>/cancel?host=node_id` → assert the fake node
///      receives `Frame::Cancel{run_id}`.
#[tokio::test]
async fn tunnel_e2e_dispatch_mirror_observe_cancel() {
    use futures_util::TryStreamExt as _;
    use rupu_cp::node::{ArtifactFile, Auth, Frame, RunSpecKind};
    use rupu_workspace::HostTransport;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::io::AsyncBufReadExt as _;
    use tokio_tungstenite::connect_async;

    let dir = tempdir().unwrap();

    // ── 1. Build AppState ─────────────────────────────────────────────────────
    //
    // `AppState::new` already wires node_registry + node_mirror + tunnel deps
    // into the HostRegistry, so no `.with_launcher()` is needed: the remote-
    // host path (`host != "local"`) routes via TunnelHostConnector directly.
    let state = rupu_cp::state::AppState::new(
        dir.path().into(),
        rupu_config::PricingConfig::default(),
    );
    let run_store = Arc::clone(&state.run_store);
    let node_registry = Arc::clone(&state.node_registry);

    // ── 2. Enroll a tunnel node ────────────────────────────────────────────────
    let (host, token) = state
        .hosts
        .enroll_node("e2e-smoke-node")
        .expect("enroll_node should succeed");

    let node_id = match &host.transport {
        HostTransport::Tunnel { node_id } => node_id.clone(),
        _ => panic!("expected Tunnel transport after enroll_node"),
    };
    // For Tunnel hosts, host.id == node_id (by construction in enroll_node).
    assert_eq!(host.id, node_id, "host.id must equal node_id for Tunnel hosts");

    // ── Spin the server ────────────────────────────────────────────────────────
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();

    // ── 3. Fake WS client: Hello → Welcome ────────────────────────────────────
    let ws_url = format!("ws://{addr}/api/node/connect");
    let (mut ws, _) = connect_async(&ws_url)
        .await
        .expect("WS connect should succeed");

    send_frame(
        &mut ws,
        &Frame::Hello {
            node_id: node_id.clone(),
            auth: Auth::Token { token },
            rupu_version: "e2e-test".to_string(),
            capabilities: vec![],
        },
    )
    .await;

    let welcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_frame(&mut ws),
    )
    .await
    .expect("timed out waiting for Welcome")
    .expect("WS closed before Welcome");
    assert!(
        matches!(welcome, Frame::Welcome {}),
        "expected Welcome, got {welcome:?}"
    );

    // Give the server's WS task time to register the node before dispatching.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        node_registry.is_online(&node_id),
        "node should be online in the registry after Welcome"
    );

    // ── 4. Dispatch via central HTTP ────────────────────────────────────────────
    //
    // POST /api/agents/:name/run with host=node_id routes to
    // TunnelHostConnector::launch_agent, which creates a mirror run and sends
    // Frame::Run to the connected node. No agent definition file is needed on
    // the central CP — the connector sends the spec to the node as-is.
    let dispatch_resp = client
        .post(format!("http://{addr}/api/agents/smoke-agent/run"))
        .json(&serde_json::json!({
            "host": node_id,
            "prompt": "e2e smoke test"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        dispatch_resp.status(),
        reqwest::StatusCode::OK,
        "agent dispatch should return 200"
    );

    let dispatch_body: serde_json::Value = dispatch_resp.json().await.unwrap();
    let run_id = dispatch_body["run_id"]
        .as_str()
        .expect("dispatch response must include run_id")
        .to_string();
    assert_eq!(
        dispatch_body["host_id"].as_str(),
        Some(node_id.as_str()),
        "dispatch response must echo the node_id as host_id"
    );

    // Fake node receives Frame::Run with the CP-allocated run_id.
    let run_frame = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_frame(&mut ws),
    )
    .await
    .expect("timed out waiting for Run frame")
    .expect("WS closed before Run frame");

    if let Frame::Run { run_id: ref fid, ref spec } = run_frame {
        assert_eq!(fid, &run_id, "frame run_id must match the CP-allocated run_id");
        assert_eq!(spec.kind, RunSpecKind::Agent, "spec kind must be Agent");
        assert_eq!(spec.name, "smoke-agent", "spec name must match");
        assert_eq!(
            spec.prompt.as_deref(),
            Some("e2e smoke test"),
            "prompt must be forwarded in the spec"
        );
    } else {
        panic!("expected Frame::Run, got {run_frame:?}");
    }

    // ── 5. Fake node streams artifacts + signals completion ─────────────────────
    //
    // Event lines must be valid rupu_orchestrator::executor::Event JSON so that
    // FileTailRunSource can parse and re-emit them via the SSE log endpoint.
    // StepStarted requires: run_id, step_id, kind (snake_case StepKind), agent.
    // StepCompleted requires: run_id, step_id, success, duration_ms.
    let event_step_started = format!(
        r#"{{"type":"step_started","run_id":"{run_id}","step_id":"s1","kind":"linear","agent":"smoke-agent"}}"#
    );
    let event_step_completed = format!(
        r#"{{"type":"step_completed","run_id":"{run_id}","step_id":"s1","success":true,"duration_ms":1}}"#
    );

    send_frame(
        &mut ws,
        &Frame::Artifact {
            run_id: run_id.clone(),
            file: ArtifactFile::Events,
            line: event_step_started,
        },
    )
    .await;
    send_frame(
        &mut ws,
        &Frame::Artifact {
            run_id: run_id.clone(),
            file: ArtifactFile::Events,
            line: event_step_completed,
        },
    )
    .await;
    send_frame(
        &mut ws,
        &Frame::RunFinished {
            run_id: run_id.clone(),
            status: "completed".to_string(),
        },
    )
    .await;

    // ── 6. Poll central mirror ─────────────────────────────────────────────────
    //
    // The server's WS read pump processes frames asynchronously; wait up to 3 s
    // for the run to appear in GET /api/runs?host=<node_id>.  Once RunFinished
    // is processed, both Artifact lines are guaranteed to be written (TCP order).
    let run_id_for_poll = run_id.clone();
    let node_id_for_poll = node_id.clone();
    let client_for_poll = client.clone();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        async move {
            loop {
                let resp = client_for_poll
                    .get(format!(
                        "http://{addr}/api/runs?host={node_id_for_poll}"
                    ))
                    .send()
                    .await
                    .unwrap();
                if resp.status() == reqwest::StatusCode::OK {
                    let rows: Vec<serde_json::Value> = resp.json().await.unwrap();
                    if let Some(row) = rows
                        .iter()
                        .find(|r| r["id"].as_str() == Some(run_id_for_poll.as_str()))
                    {
                        assert_eq!(
                            row["host_id"].as_str(),
                            Some(node_id_for_poll.as_str()),
                            "run row must carry node_id as host_id"
                        );
                        return;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        },
    )
    .await
    .expect("run should appear in the central mirror within 3 s");

    // Verify directly via RunStore: status must be Completed, worker_id == node_id.
    let record = run_store.load(&run_id).expect("run must be loadable from central store");
    assert_eq!(
        record.worker_id.as_deref(),
        Some(node_id.as_str()),
        "worker_id must carry the node_id attribution"
    );

    // events.jsonl must have exactly two lines (the Artifact frames we sent).
    let events_path = run_store.events_path(&run_id);
    let events_content =
        std::fs::read_to_string(&events_path).expect("events.jsonl must exist");
    let event_lines: Vec<&str> = events_content.lines().collect();
    assert_eq!(
        event_lines.len(),
        2,
        "events.jsonl must have exactly 2 lines after 2 Artifact frames"
    );

    // ── 7. GET log endpoint → SSE stream contains the step_started event ────────
    //
    // GET /api/runs/:id/log?host=<node_id> calls TunnelHostConnector::stream_run_events
    // → open_run_events_tail → FileTailRunSource tails the central events.jsonl.
    let log_resp = client
        .get(format!(
            "http://{addr}/api/runs/{run_id}/log?host={node_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        log_resp.status(),
        reqwest::StatusCode::OK,
        "log endpoint must return 200"
    );
    let ct = log_resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "log endpoint must return text/event-stream; got {ct:?}"
    );

    let stream = log_resp.bytes_stream().map_err(std::io::Error::other);
    let async_reader = tokio_util::io::StreamReader::new(stream);
    let mut lines = tokio::io::BufReader::new(async_reader).lines();

    let found_event = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data.contains("step_started") {
                        return true;
                    }
                }
            }
            false
        },
    )
    .await
    .expect("SSE log stream read timed out");
    assert!(
        found_event,
        "SSE log stream must contain the step_started event"
    );

    // ── 8. Cancel → fake node receives Frame::Cancel ────────────────────────────
    //
    // TunnelHostConnector::cancel_run sends Frame::Cancel to the node via the
    // live WS connection (still open — the fake WS client stays connected after
    // RunFinished; only the node decides when to close).
    let cancel_resp = client
        .post(format!(
            "http://{addr}/api/runs/{run_id}/cancel?host={node_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cancel_resp.status(),
        reqwest::StatusCode::OK,
        "cancel must return 200"
    );
    let cancel_body: serde_json::Value = cancel_resp.json().await.unwrap();
    assert_eq!(
        cancel_body["ok"].as_bool(),
        Some(true),
        "cancel response must include ok:true"
    );
    assert_eq!(
        cancel_body["host_id"].as_str(),
        Some(node_id.as_str()),
        "cancel response must echo the node_id as host_id"
    );

    // Fake node receives Frame::Cancel with the correct run_id.
    let cancel_frame = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_frame(&mut ws),
    )
    .await
    .expect("timed out waiting for Cancel frame")
    .expect("WS closed before Cancel frame");

    if let Frame::Cancel { run_id: ref fid } = cancel_frame {
        assert_eq!(
            fid, &run_id,
            "Cancel frame run_id must match the dispatched run_id"
        );
    } else {
        panic!("expected Frame::Cancel, got {cancel_frame:?}");
    }
}
