//! WebSocket tunnel endpoint: `GET /api/node/connect`.
//!
//! A remote `rupu node` dials here, authenticates with its enrollment token,
//! and stays connected.  The CP pushes `Run`/`Cancel`/`Ping` frames down and
//! receives `Artifact`/`RunFinished`/`Pong` frames up, writing artifacts into
//! the central [`RunStore`] via the [`NodeMirror`].
//!
//! ## Handshake
//!
//! 1. Node connects and sends `Frame::Hello { node_id, auth: Auth::Token { .. }, .. }`.
//! 2. CP looks up the host by `node_id`, verifies the token (constant-time).
//! 3. On failure: send `Close` and return.
//! 4. On success: `NodeRegistry::register`, send `Frame::Welcome`, then run pumps.
//!
//! ## Pumps
//!
//! * **Write pump:** drains the mpsc `Receiver<Frame>` and serialises frames to
//!   the WS sink.  Also owns the periodic keepalive: every
//!   [`KEEPALIVE_INTERVAL`] it injects a `Frame::Ping`.
//! * **Read pump:** receives messages from the WS stream and dispatches:
//!   - `Frame::Artifact` → [`NodeMirror::append`]
//!   - `Frame::RunFinished` → [`NodeMirror::finish`]
//!   - `Frame::Pong` → [`NodeRegistry::mark_seen`]
//!   - Malformed JSON → log and **continue** (do not tear down the tunnel).
//!   - `Close` / WS error → break.
//!
//! Both pumps run under a single `tokio::select!`; when either exits the
//! other is cancelled, after which [`NodeRegistry::remove`] is called with
//! the `Arc<NodeConn>` guard so a newer reconnect is never clobbered.

#![deny(clippy::all)]

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tracing::{info, warn};

use rupu_workspace::{verify_node_token, HostTransport};

use crate::node::protocol::{Auth, Frame};
use crate::state::AppState;

/// How often the CP sends a `Frame::Ping` to keep the tunnel alive.
///
/// Task 7 may make this configurable; the constant is named so the value
/// is easy to find and change.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Outbound frame channel capacity per connected node.
///
/// Prevents unbounded memory growth when a slow node falls behind.
const CHANNEL_CAPACITY: usize = 256;

/// Maximum time to wait for the initial `Frame::Hello` after a WS connection
/// is established.  Unauthenticated clients that hold the socket open without
/// sending Hello are forcibly disconnected after this deadline.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

// ── Router ────────────────────────────────────────────────────────────────────

/// Returns the router fragment that mounts `GET /api/node/connect`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/node/connect", get(ws_handler))
}

// ── Handler ───────────────────────────────────────────────────────────────────

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

// ── Socket lifecycle ──────────────────────────────────────────────────────────

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // ── 1. Receive Hello (with deadline) ─────────────────────────────────────
    //
    // Unauthenticated clients must send Hello within HELLO_TIMEOUT; otherwise
    // they could hold sockets open indefinitely.
    let first = match tokio::time::timeout(HELLO_TIMEOUT, ws_rx.next()).await {
        Err(_elapsed) => {
            warn!("node_tunnel: Hello timeout; closing socket");
            let _ = ws_tx.send(Message::Close(None)).await;
            return;
        }
        Ok(Some(Ok(msg))) => msg,
        Ok(Some(Err(e))) => {
            warn!(error = %e, "node_tunnel: ws error before Hello");
            return;
        }
        Ok(None) => {
            warn!("node_tunnel: connection closed before Hello");
            return;
        }
    };

    let text = match first {
        Message::Text(t) => t,
        Message::Close(_) => return,
        other => {
            warn!(kind = ?std::mem::discriminant(&other),
                  "node_tunnel: expected Text for Hello");
            return;
        }
    };

    let frame: Frame = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "node_tunnel: could not parse Hello frame");
            let _ = ws_tx.send(Message::Close(None)).await;
            return;
        }
    };

    let (node_id, token) = match frame {
        Frame::Hello {
            node_id,
            auth: Auth::Token { token },
            ..
        } => (node_id, token),
        other => {
            warn!(frame_type = ?std::mem::discriminant(&other),
                  "node_tunnel: first frame was not Hello");
            let _ = ws_tx.send(Message::Close(None)).await;
            return;
        }
    };

    // ── 2. Authenticate ───────────────────────────────────────────────────────
    //
    // Look up the host by scanning the list for a `Tunnel { node_id }` match.
    // `list_hosts()` is cheap (one disk read + sort for a handful of hosts).
    let host = state.hosts.list_hosts().into_iter().find(|h| {
        matches!(&h.transport, HostTransport::Tunnel { node_id: nid } if *nid == node_id)
    });

    let host = match host {
        Some(h) => h,
        None => {
            warn!(node_id, "node_tunnel: no tunnel host enrolled for node_id");
            let _ = ws_tx.send(Message::Close(None)).await;
            return;
        }
    };

    if !verify_node_token(&host, &token) {
        warn!(node_id, "node_tunnel: token verification failed");
        let _ = ws_tx.send(Message::Close(None)).await;
        return;
    }

    // ── 3. Register + Welcome ─────────────────────────────────────────────────
    let (tx, mut rx) = mpsc::channel::<Frame>(CHANNEL_CAPACITY);
    let conn = state.node_registry.register(&node_id, tx);

    let welcome = match serde_json::to_string(&Frame::Welcome {}) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "node_tunnel: could not serialize Welcome");
            state.node_registry.remove(&node_id, &conn);
            return;
        }
    };
    if let Err(e) = ws_tx.send(Message::Text(welcome)).await {
        warn!(error = %e, node_id, "node_tunnel: could not send Welcome");
        state.node_registry.remove(&node_id, &conn);
        return;
    }

    info!(node_id, "node_tunnel: node connected");

    // ── 4. Pumps ──────────────────────────────────────────────────────────────

    // Clone handles needed by each pump.
    let node_id_r = node_id.clone();
    let registry_r = Arc::clone(&state.node_registry);
    let mirror = Arc::clone(&state.node_mirror);

    // Write pump: mpsc rx → WS sink, with periodic keepalive.
    let write_pump = async move {
        let mut interval = tokio::time::interval(KEEPALIVE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Consume the first tick (fires immediately) so the first keepalive
        // goes out after one full interval.
        interval.tick().await;

        loop {
            tokio::select! {
                frame = rx.recv() => {
                    let Some(f) = frame else { break };
                    match serde_json::to_string(&f) {
                        Ok(s) => {
                            if let Err(e) = ws_tx.send(Message::Text(s)).await {
                                warn!(error = %e, "node_tunnel: write pump send error");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "node_tunnel: could not serialize outbound frame");
                        }
                    }
                }
                _ = interval.tick() => {
                    match serde_json::to_string(&Frame::Ping {}) {
                        Ok(s) => {
                            if let Err(e) = ws_tx.send(Message::Text(s)).await {
                                warn!(error = %e, "node_tunnel: keepalive Ping send error");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "node_tunnel: could not serialize keepalive Ping");
                        }
                    }
                }
            }
        }
    };

    // Read pump: WS stream → mirror/registry dispatch.
    let read_pump = async move {
        while let Some(msg_result) = ws_rx.next().await {
            let msg = match msg_result {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, node_id = node_id_r,
                          "node_tunnel: read pump ws error");
                    break;
                }
            };

            match msg {
                Message::Text(t) => {
                    let frame: Frame = match serde_json::from_str(&t) {
                        Ok(f) => f,
                        Err(e) => {
                            // Malformed frame: log and continue — do NOT
                            // tear down the tunnel (per T4 reviewer note).
                            warn!(error = %e, node_id = node_id_r,
                                  "node_tunnel: malformed inbound frame (ignored)");
                            continue;
                        }
                    };

                    match frame {
                        Frame::Artifact { run_id, file, line } => {
                            if let Err(e) =
                                mirror.append(&run_id, &node_id_r, file, &line)
                            {
                                warn!(error = %e, run_id,
                                      "node_tunnel: mirror.append failed");
                            }
                        }
                        Frame::RunFinished { run_id, status } => {
                            if let Err(e) =
                                mirror.finish(&run_id, &node_id_r, &status)
                            {
                                warn!(error = %e, run_id,
                                      "node_tunnel: mirror.finish failed");
                            }
                        }
                        Frame::Pong {} => {
                            registry_r.mark_seen(&node_id_r);
                        }
                        // WS-level Ping handled automatically by axum; a
                        // Frame::Ping from node is unexpected but harmless.
                        Frame::Ping {} => {}
                        other => {
                            warn!(
                                frame_type = ?std::mem::discriminant(&other),
                                node_id = node_id_r,
                                "node_tunnel: unexpected inbound frame type"
                            );
                        }
                    }
                }
                // WS-protocol-level ping/pong: axum handles echo automatically.
                Message::Ping(_) | Message::Pong(_) => {}
                // Close or binary: shut down the read pump.
                Message::Close(_) | Message::Binary(_) => break,
            }
        }
    };

    // Run both pumps concurrently; clean up on whichever exits first.
    tokio::select! {
        _ = write_pump => {}
        _ = read_pump => {}
    }

    // ── 5. Cleanup ────────────────────────────────────────────────────────────
    //
    // `remove` uses `Arc::ptr_eq` so a newer reconnect (different Arc) is never
    // clobbered by a stale disconnect handler.
    state.node_registry.remove(&node_id, &conn);
    info!(node_id, "node_tunnel: node disconnected");
}
