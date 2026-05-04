//! MCP server kernel — JSON-RPC 2.0 dispatch loop over a Transport.

use crate::error::McpError;
use crate::transport::{InProcessTransport, Transport};
use rupu_scm::Registry;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};

pub struct McpServer<T: Transport + 'static> {
    registry: Arc<Registry>,
    transport: T,
}

impl<T: Transport + 'static> McpServer<T> {
    pub fn new(registry: Arc<Registry>, transport: T) -> Self {
        Self {
            registry,
            transport,
        }
    }

    pub async fn run(self) -> Result<(), McpError> {
        loop {
            let msg = match self.transport.recv().await? {
                Some(m) => m,
                None => return Ok(()),
            };
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let _params = msg.get("params").cloned().unwrap_or(Value::Null);

            let response = match method {
                "initialize" => Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "serverInfo": { "name": "rupu", "version": env!("CARGO_PKG_VERSION") },
                        "capabilities": { "tools": {} },
                    },
                })),
                "tools/list" => Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": tool_catalog_placeholder() },
                })),
                "tools/call" => {
                    // Tool dispatch lands in Task 14 (ToolDispatcher).
                    // For now, surface a clear error so the test path
                    // is observable but doesn't pretend to work.
                    let _ = &self.registry;
                    warn!(method, "tools/call not yet wired (Task 14)");
                    Ok(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "isError": true,
                            "content": [{
                                "type": "text",
                                "text": "tools/call not yet wired (Plan 2 Task 14)"
                            }]
                        }
                    }))
                }
                other => {
                    debug!(method = other, "unknown method");
                    Err(McpError::UnknownTool(other.to_string()))
                }
            };

            match response {
                Ok(v) => self.transport.send(v).await?,
                Err(e) => self.transport.send(e.to_jsonrpc(id)).await?,
            }
        }
    }
}

/// Placeholder tool list for Task 9 — Task 11+ replaces with the
/// generated catalog. This empty array lets `tools/list` be a stable
/// (if minimal) JSON-RPC method response while Tasks 10-13 build the
/// real catalog.
fn tool_catalog_placeholder() -> Value {
    Value::Array(vec![])
}

pub struct ServeHandle {
    pub join: JoinHandle<Result<(), McpError>>,
}

/// Spin up the MCP server in-process. Returns the client handle the
/// agent runtime uses to send `tools/call` requests, plus a JoinHandle
/// the caller drops at run end to tear down cleanly.
pub fn serve_in_process(registry: Arc<Registry>) -> (InProcessTransport, ServeHandle) {
    let (client_t, server_t) = InProcessTransport::pair();
    let server = McpServer::new(registry, server_t);
    let join = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            error!(error = %e, "mcp server failed");
            return Err(e);
        }
        Ok(())
    });
    (client_t, ServeHandle { join })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Transport;

    #[tokio::test]
    async fn server_responds_to_initialize_and_tools_list() {
        let registry = Arc::new(Registry::empty());
        let (client, handle) = serve_in_process(registry);

        // initialize
        client
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
            }))
            .await
            .unwrap();
        let resp = client.recv().await.unwrap().unwrap();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["serverInfo"]["name"], "rupu");

        // tools/list
        client
            .send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/list",
            }))
            .await
            .unwrap();
        let resp = client.recv().await.unwrap().unwrap();
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(
            tools.len(),
            0,
            "Task 9 ships empty placeholder; Task 11+ adds entries"
        );

        // tools/call returns the not-yet-wired error gracefully (no panic)
        client
            .send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "scm.repos.list", "arguments": {}}
            }))
            .await
            .unwrap();
        let resp = client.recv().await.unwrap().unwrap();
        assert_eq!(resp["id"], 3);
        assert_eq!(resp["result"]["isError"], true);

        // unknown method → JSON-RPC error envelope
        client
            .send(json!({
                "jsonrpc": "2.0", "id": 4, "method": "bogus/method",
            }))
            .await
            .unwrap();
        let resp = client.recv().await.unwrap().unwrap();
        assert_eq!(resp["id"], 4);
        assert_eq!(resp["error"]["code"], -32601);

        drop(client);
        let _ = handle.join.await;
    }
}
