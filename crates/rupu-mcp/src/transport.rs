//! Transport abstraction. Both transports carry newline-delimited JSON
//! lines and decode them with [`decode_line`]; the server kernel handles
//! JSON-RPC itself.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex as TokioMutex};

use crate::error::McpError;

#[async_trait]
pub trait Transport: Send + Sync {
    /// The next message; `Ok(None)` once the peer closes. A line that is not
    /// JSON is `Err(McpError::Parse)`, after which the transport can still
    /// be read.
    async fn recv(&self) -> Result<Option<Value>, McpError>;
    async fn send(&self, msg: Value) -> Result<(), McpError>;
}

/// One input line: `None` for a blank line (skipped), else the JSON value
/// or why it isn't one.
pub fn decode_line(line: &str) -> Option<Result<Value, McpError>> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    Some(serde_json::from_str(line).map_err(|e| McpError::Parse(e.to_string())))
}

fn encode(msg: &Value) -> Result<String, McpError> {
    serde_json::to_string(msg).map_err(|e| McpError::Transport(anyhow::anyhow!("serialize: {e}")))
}

/// In-process transport: a pair of channels of JSON lines, for driving a
/// server from the same process (tests).
#[derive(Clone)]
pub struct InProcessTransport {
    inbox: Arc<TokioMutex<mpsc::UnboundedReceiver<String>>>,
    outbox: mpsc::UnboundedSender<String>,
}

impl InProcessTransport {
    pub fn pair() -> (Self, Self) {
        let (client_tx, server_rx) = mpsc::unbounded_channel::<String>();
        let (server_tx, client_rx) = mpsc::unbounded_channel::<String>();
        let client = Self {
            inbox: Arc::new(TokioMutex::new(client_rx)),
            outbox: client_tx,
        };
        let server = Self {
            inbox: Arc::new(TokioMutex::new(server_rx)),
            outbox: server_tx,
        };
        (client, server)
    }

    /// Send one raw line as is, e.g. one that isn't JSON.
    pub fn send_raw(&self, line: &str) -> Result<(), McpError> {
        self.outbox
            .send(line.to_string())
            .map_err(|e| McpError::Transport(anyhow::anyhow!("inprocess send: {e}")))
    }
}

#[async_trait]
impl Transport for InProcessTransport {
    async fn recv(&self) -> Result<Option<Value>, McpError> {
        let mut inbox = self.inbox.lock().await;
        loop {
            let Some(line) = inbox.recv().await else {
                return Ok(None);
            };
            if let Some(decoded) = decode_line(&line) {
                return decoded.map(Some);
            }
        }
    }
    async fn send(&self, msg: Value) -> Result<(), McpError> {
        self.send_raw(&encode(&msg)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inprocess_transport_round_trips_messages() {
        let (client, server) = InProcessTransport::pair();
        client
            .send(serde_json::json!({"hello": "from-client"}))
            .await
            .unwrap();
        let received = server.recv().await.unwrap().unwrap();
        assert_eq!(received["hello"], "from-client");

        server
            .send(serde_json::json!({"hello": "from-server"}))
            .await
            .unwrap();
        let received = client.recv().await.unwrap().unwrap();
        assert_eq!(received["hello"], "from-server");
    }

    #[tokio::test]
    async fn blank_lines_are_skipped_and_a_bad_line_does_not_end_the_stream() {
        let (client, server) = InProcessTransport::pair();
        client.send_raw("").unwrap();
        client.send_raw("   ").unwrap();
        client.send_raw("{not json").unwrap();
        client.send_raw(r#"{"ok":1}"#).unwrap();
        assert!(matches!(server.recv().await, Err(McpError::Parse(_))));
        assert_eq!(server.recv().await.unwrap().unwrap()["ok"], 1);
    }
}
