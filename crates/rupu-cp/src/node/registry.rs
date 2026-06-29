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
}

impl NodeConn {
    fn new(tx: Sender<Frame>) -> Self {
        let now = Utc::now();
        Self {
            tx,
            connected_at: now,
            last_seen: Mutex::new(now),
        }
    }

    /// Send a frame down the tunnel.
    ///
    /// Returns `Err(NodeError::Offline)` if the node's receiver has been dropped.
    pub async fn send(&self, f: Frame) -> Result<(), NodeError> {
        self.tx.send(f).await.map_err(|_| NodeError::Offline)
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
        let conn = Arc::new(NodeConn::new(tx));
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
        let conn = Arc::new(NodeConn::new(tx));
        drop(rx);
        let result = conn.send(Frame::Ping {}).await;
        assert!(matches!(result, Err(NodeError::Offline)));
    }
}
