#![deny(clippy::all)]

//! Live tunnel connections: `NodeConn` + `NodeRegistry`.
//!
//! A node (remote rupu daemon) opens a WebSocket tunnel to the control-plane.
//! `NodeRegistry` tracks one active `NodeConn` per node-id.  Each `NodeConn`
//! wraps a tokio mpsc `Sender<Frame>` so the CP can push frames to the node
//! without holding any registry lock across `.await`.
//!
//! ## Lock discipline
//!
//! `NodeRegistry::conns` is a `std::sync::Mutex` (not tokio's).  All methods
//! that touch the map do so under a *short, synchronous* lock: clone the
//! `Arc<NodeConn>` out, release the lock, *then* await the send.  Never hold
//! the mutex guard across an `.await` point.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::mpsc::Sender;

use crate::node::protocol::Frame;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum NodeError {
    /// The node's receiver half has been dropped; the tunnel is gone.
    #[error("node is offline")]
    Offline,
}

// ── Artifact pulls ────────────────────────────────────────────────────────────

/// One message of an in-flight artifact pull, routed from the tunnel's read
/// pump to the connector waiting on it.
#[derive(Debug)]
pub enum PullMsg {
    Chunk {
        seq: u64,
        data: Vec<u8>,
    },
    /// End of the pull; `Some` carries the node's error.
    Done(Option<String>),
}

/// Per-pull channel depth: how many chunks the read pump may run ahead of a
/// waiter that is still writing earlier ones to disk.
const PULL_CHANNEL_CAPACITY: usize = 8;

// ── NodeConn ──────────────────────────────────────────────────────────────────

/// A live connection to a remote node.
///
/// `tx` is the write-end of the tunnel channel.  When the node disconnects the
/// read-end is dropped, causing `tx.send` to return an error; `send` maps that
/// to `NodeError::Offline`.
pub struct NodeConn {
    tx: Sender<Frame>,
    pub connected_at: DateTime<Utc>,
    pub last_seen: Mutex<DateTime<Utc>>,
    /// What the node advertised in `Hello.capabilities` for THIS connection
    /// (a reconnect after an upgrade/downgrade replaces the whole `NodeConn`).
    capabilities: Vec<String>,
    /// The node's `Hello.rupu_version`, for refusal messages.
    rupu_version: Option<String>,
    /// In-flight artifact pulls on this connection, by request id: where
    /// the read pump delivers each `ArtifactChunk` / `ArtifactPullDone`.
    pulls: Mutex<HashMap<String, tokio::sync::mpsc::Sender<PullMsg>>>,
}

impl NodeConn {
    fn new(tx: Sender<Frame>, capabilities: Vec<String>, rupu_version: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            tx,
            connected_at: now,
            last_seen: Mutex::new(now),
            capabilities,
            rupu_version,
            pulls: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the node advertised `capability` (e.g.
    /// [`crate::node::protocol::CAP_AGENT_FINDINGS_PROFILE`]) in its `Hello`.
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// The node's self-reported rupu version, when it sent one.
    pub fn rupu_version(&self) -> Option<&str> {
        self.rupu_version.as_deref()
    }

    /// Send a frame down the tunnel.
    ///
    /// Returns `Err(NodeError::Offline)` if the node's receiver has been dropped.
    pub async fn send(&self, f: Frame) -> Result<(), NodeError> {
        self.tx.send(f).await.map_err(|_| NodeError::Offline)
    }

    /// Register `req` and return the receiver its chunks arrive on.
    pub fn begin_pull(&self, req: &str) -> tokio::sync::mpsc::Receiver<PullMsg> {
        let (tx, rx) = tokio::sync::mpsc::channel(PULL_CHANNEL_CAPACITY);
        self.pulls
            .lock()
            .expect("pulls lock poisoned")
            .insert(req.to_string(), tx);
        rx
    }

    /// Forget `req` (the waiting side finished or gave up).
    pub fn end_pull(&self, req: &str) {
        self.pulls.lock().expect("pulls lock poisoned").remove(req);
    }

    /// Deliver `msg` to `req`'s waiter; dropped if nobody is waiting.
    ///
    /// Awaits room in the waiter's bounded channel (back-pressure on the read
    /// pump), but never indefinitely: a waiter drops its receiver on every
    /// exit, which fails the send at once.
    pub async fn route_pull(&self, req: &str, msg: PullMsg) {
        let tx = self
            .pulls
            .lock()
            .expect("pulls lock poisoned")
            .get(req)
            .cloned();
        if let Some(tx) = tx {
            let _ = tx.send(msg).await;
        }
    }

    /// The tunnel closed: end every in-flight pull. Each waiter's channel
    /// closes, so it fails at once instead of waiting out its idle timeout.
    pub fn close_pulls(&self) {
        self.pulls.lock().expect("pulls lock poisoned").clear();
    }

    /// How many pulls are registered on this connection.
    #[cfg(test)]
    pub(crate) fn pulls_in_flight(&self) -> usize {
        self.pulls.lock().expect("pulls lock poisoned").len()
    }
}

// ── NodeRegistry ──────────────────────────────────────────────────────────────

/// Registry of live node tunnel connections.
///
/// Holds at most one `Arc<NodeConn>` per node-id.  Registering a second
/// connection for the same id evicts (and drops) the prior one.
pub struct NodeRegistry {
    conns: Mutex<HashMap<String, Arc<NodeConn>>>,
}

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new connection for `node_id`.
    ///
    /// Any existing connection for that id is evicted: its `Arc<NodeConn>` is
    /// dropped (the mpsc `Sender` inside it is dropped), which causes the old
    /// tunnel's `Receiver` to drain and close.
    ///
    /// Returns the freshly created `Arc<NodeConn>`.
    pub fn register(&self, node_id: &str, tx: Sender<Frame>) -> Arc<NodeConn> {
        self.register_with_hello(node_id, tx, Vec::new(), None)
    }

    /// [`Self::register`], recording what the node's `Hello` advertised —
    /// the tunnel server's path. A node registered without capabilities
    /// supports none, so a launch needing one is refused (fail closed).
    pub fn register_with_hello(
        &self,
        node_id: &str,
        tx: Sender<Frame>,
        capabilities: Vec<String>,
        rupu_version: Option<String>,
    ) -> Arc<NodeConn> {
        let conn = Arc::new(NodeConn::new(tx, capabilities, rupu_version));
        let mut map = self.conns.lock().expect("NodeRegistry lock poisoned");
        // The old Arc is dropped here, closing the sender side of the old channel.
        map.insert(node_id.to_owned(), Arc::clone(&conn));
        conn
    }

    /// Return the current connection for `node_id`, or `None` if not online.
    pub fn get(&self, node_id: &str) -> Option<Arc<NodeConn>> {
        let map = self.conns.lock().expect("NodeRegistry lock poisoned");
        map.get(node_id).cloned()
    }

    /// Remove the connection for `node_id` **only if** it is still `only_if`.
    ///
    /// Uses `Arc::ptr_eq` so a *newer* reconnect (a different `Arc`) is never
    /// clobbered by a stale disconnect handler.
    pub fn remove(&self, node_id: &str, only_if: &Arc<NodeConn>) {
        let mut map = self.conns.lock().expect("NodeRegistry lock poisoned");
        if let Some(current) = map.get(node_id) {
            if Arc::ptr_eq(current, only_if) {
                map.remove(node_id);
            }
        }
    }

    /// Returns `true` if there is a live connection for `node_id`.
    pub fn is_online(&self, node_id: &str) -> bool {
        let map = self.conns.lock().expect("NodeRegistry lock poisoned");
        map.contains_key(node_id)
    }

    /// Update `last_seen` for `node_id` to now.  No-op if the node is unknown.
    pub fn mark_seen(&self, node_id: &str) {
        let map = self.conns.lock().expect("NodeRegistry lock poisoned");
        if let Some(conn) = map.get(node_id) {
            let mut last = conn.last_seen.lock().expect("last_seen lock poisoned");
            *last = Utc::now();
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// `register` then `get` returns the same `Arc`.
    #[test]
    fn register_then_get_returns_conn() {
        let reg = NodeRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        let conn = reg.register("node-1", tx);
        let got = reg.get("node-1").expect("should be present");
        assert!(Arc::ptr_eq(&conn, &got));
    }

    /// `is_online` reflects presence.
    #[test]
    fn is_online_reflects_presence() {
        let reg = NodeRegistry::new();
        assert!(!reg.is_online("node-2"));
        let (tx, _rx) = mpsc::channel(8);
        reg.register("node-2", tx);
        assert!(reg.is_online("node-2"));
    }

    /// Re-registering the same id evicts the old conn (old `send` errors Offline).
    #[tokio::test]
    async fn re_register_evicts_old_conn() {
        let reg = NodeRegistry::new();

        // First registration — keep the old conn handle.
        let (tx1, mut rx1) = mpsc::channel(8);
        let old = reg.register("node-3", tx1);

        // Second registration — evicts the first.
        let (tx2, _rx2) = mpsc::channel(8);
        reg.register("node-3", tx2);

        // The receiver of the *first* channel is still alive (_rx1 not dropped
        // yet).  Drain any buffered frames, then drop the receiver to close it.
        // The registry has dropped its clone of tx1, so there are no other
        // senders; the channel closes once we drop rx1 here.
        rx1.close();

        // old.send should now error because the registry dropped tx1.
        let result = old.send(Frame::Ping {}).await;
        assert!(
            matches!(result, Err(NodeError::Offline)),
            "expected Offline after eviction, got {result:?}"
        );
    }

    /// `remove(only_if)` is a no-op when a newer conn has replaced the old one.
    #[test]
    fn remove_only_if_noop_on_newer_conn() {
        let reg = NodeRegistry::new();

        let (tx1, _rx1) = mpsc::channel(8);
        let old = reg.register("node-4", tx1);

        // Replace with a newer conn.
        let (tx2, _rx2) = mpsc::channel(8);
        let new_conn = reg.register("node-4", tx2);

        // Try to remove using the *old* Arc — should be a no-op.
        reg.remove("node-4", &old);
        assert!(
            reg.is_online("node-4"),
            "newer conn should still be present after stale remove"
        );

        // Remove with the correct (new) Arc — should work.
        reg.remove("node-4", &new_conn);
        assert!(!reg.is_online("node-4"), "conn should be gone after correct remove");
    }

    /// `mark_seen` updates `last_seen` to a time ≥ `connected_at`.
    #[tokio::test]
    async fn mark_seen_updates_timestamp() {
        let reg = NodeRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        let conn = reg.register("node-5", tx);

        let before = *conn.last_seen.lock().unwrap();

        // Sleep a tiny bit so the clock advances.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        reg.mark_seen("node-5");

        let after = *conn.last_seen.lock().unwrap();
        assert!(
            after >= before,
            "last_seen should advance after mark_seen"
        );
    }

    /// `send` succeeds when the receiver is alive.
    #[tokio::test]
    async fn send_succeeds_when_receiver_alive() {
        let reg = NodeRegistry::new();
        let (tx, mut rx) = mpsc::channel(8);
        let conn = reg.register("node-6", tx);

        conn.send(Frame::Ping {}).await.expect("send should succeed");
        let frame = rx.recv().await.expect("should receive frame");
        assert!(matches!(frame, Frame::Ping {}));
    }

    /// `send` returns `NodeError::Offline` when the receiver is dropped.
    #[tokio::test]
    async fn send_errors_offline_when_receiver_dropped() {
        let (tx, rx) = mpsc::channel::<Frame>(8);
        let conn = Arc::new(NodeConn::new(tx, Vec::new(), None));
        drop(rx);
        let result = conn.send(Frame::Ping {}).await;
        assert!(matches!(result, Err(NodeError::Offline)));
    }

    fn bare_conn() -> NodeConn {
        let (tx, _rx) = mpsc::channel::<Frame>(8);
        NodeConn::new(tx, Vec::new(), None)
    }

    /// A pull's messages reach only its own waiter; a `req` nobody began is
    /// dropped.
    #[tokio::test]
    async fn route_pull_delivers_only_to_the_begun_request() {
        let conn = bare_conn();
        let mut a = conn.begin_pull("a");
        let mut b = conn.begin_pull("b");
        conn.route_pull(
            "a",
            PullMsg::Chunk {
                seq: 0,
                data: vec![1, 2],
            },
        )
        .await;
        conn.route_pull("nobody", PullMsg::Done(None)).await;
        assert!(matches!(
            a.try_recv(),
            Ok(PullMsg::Chunk { seq: 0, ref data }) if data == &[1, 2]
        ));
        assert!(a.try_recv().is_err());
        assert!(b.try_recv().is_err());
    }

    /// `end_pull` forgets the request: its waiter sees the channel close and
    /// later deliveries for it are dropped.
    #[tokio::test]
    async fn end_pull_forgets_the_request() {
        let conn = bare_conn();
        let mut rx = conn.begin_pull("a");
        conn.end_pull("a");
        assert_eq!(conn.pulls_in_flight(), 0);
        conn.route_pull("a", PullMsg::Done(None)).await;
        assert!(rx.recv().await.is_none());
    }

    /// The tunnel closing ends every in-flight pull at once.
    #[tokio::test]
    async fn close_pulls_ends_every_waiter() {
        let conn = bare_conn();
        let mut a = conn.begin_pull("a");
        let mut b = conn.begin_pull("b");
        conn.close_pulls();
        assert!(a.recv().await.is_none());
        assert!(b.recv().await.is_none());
        assert_eq!(conn.pulls_in_flight(), 0);
    }

    /// A waiter that dropped its receiver never blocks the router, however
    /// many messages still arrive for it.
    #[tokio::test]
    async fn route_pull_to_a_dropped_waiter_never_blocks() {
        let conn = bare_conn();
        drop(conn.begin_pull("a"));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for seq in 0..64 {
                conn.route_pull("a", PullMsg::Chunk { seq, data: vec![0] })
                    .await;
            }
        })
        .await
        .expect("route_pull blocked on a dropped waiter");
    }
}
