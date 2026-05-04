//! Transport abstraction. JSON-RPC framing (one JSON message per line)
//! is handled by the server kernel; transports just shuffle bytes.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin, Stdout};
use tokio::sync::{mpsc, Mutex as TokioMutex};

use crate::error::McpError;

#[async_trait]
pub trait Transport: Send + Sync {
    async fn recv(&self) -> Result<Option<Value>, McpError>;
    async fn send(&self, msg: Value) -> Result<(), McpError>;
}

/// In-process transport: a pair of mpsc channels. Used by the agent
/// runtime; no stdio, no serialization overhead.
#[derive(Clone)]
pub struct InProcessTransport {
    inbox: Arc<TokioMutex<mpsc::UnboundedReceiver<Value>>>,
    outbox: mpsc::UnboundedSender<Value>,
}

impl InProcessTransport {
    pub fn pair() -> (Self, Self) {
        let (client_tx, server_rx) = mpsc::unbounded_channel::<Value>();
        let (server_tx, client_rx) = mpsc::unbounded_channel::<Value>();
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
}

#[async_trait]
impl Transport for InProcessTransport {
    async fn recv(&self) -> Result<Option<Value>, McpError> {
        Ok(self.inbox.lock().await.recv().await)
    }
    async fn send(&self, msg: Value) -> Result<(), McpError> {
        self.outbox
            .send(msg)
            .map_err(|e| McpError::Transport(anyhow::anyhow!("inprocess send: {e}")))
    }
}

/// Stdio transport: newline-delimited JSON over stdin/stdout (the
/// canonical MCP wire format for spawned servers).
pub struct StdioTransport {
    stdin: Arc<TokioMutex<BufReader<Stdin>>>,
    stdout: Arc<TokioMutex<Stdout>>,
}

impl StdioTransport {
    pub fn new() -> Self {
        Self {
            stdin: Arc::new(TokioMutex::new(BufReader::new(tokio::io::stdin()))),
            stdout: Arc::new(TokioMutex::new(tokio::io::stdout())),
        }
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn recv(&self) -> Result<Option<Value>, McpError> {
        let mut buf = String::new();
        let n = self
            .stdin
            .lock()
            .await
            .read_line(&mut buf)
            .await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdin read: {e}")))?;
        if n == 0 {
            return Ok(None);
        }
        let v: Value = serde_json::from_str(buf.trim())
            .map_err(|e| McpError::InvalidArgs(format!("malformed JSON-RPC: {e}")))?;
        Ok(Some(v))
    }
    async fn send(&self, msg: Value) -> Result<(), McpError> {
        let line = serde_json::to_string(&msg)
            .map_err(|e| McpError::Transport(anyhow::anyhow!("serialize: {e}")))?;
        let mut out = self.stdout.lock().await;
        out.write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdout write: {e}")))?;
        out.write_all(b"\n")
            .await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdout write: {e}")))?;
        out.flush()
            .await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdout flush: {e}")))?;
        Ok(())
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
}
