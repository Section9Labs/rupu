//! MCP server kernel — JSON-RPC 2.0 dispatch loop over a Transport.

use crate::error::McpError;
use crate::permission::McpPermission;
use crate::transport::{InProcessTransport, Transport};
use rupu_scm::Registry;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};

pub struct McpServer<T: Transport + 'static> {
    registry: Arc<Registry>,
    transport: T,
    permission: McpPermission,
}

impl<T: Transport + 'static> McpServer<T> {
    pub fn new(registry: Arc<Registry>, transport: T, permission: McpPermission) -> Self {
        Self {
            registry,
            transport,
            permission,
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
                    "result": { "tools": crate::tools::tool_catalog() },
                })),
                "tools/call" => {
                    let name = _params
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = _params.get("arguments").cloned().unwrap_or(Value::Null);
                    let dispatcher = crate::dispatcher::ToolDispatcher::new(
                        self.registry.clone(),
                        self.permission.clone(),
                    );
                    match dispatcher.call(&name, arguments).await {
                        Ok(text) => Ok(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{"type": "text", "text": text}] }
                        })),
                        Err(e) => {
                            warn!(tool = %name, error = %e, "tool dispatch failed");
                            Ok(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "isError": true,
                                    "content": [{"type": "text", "text": e.to_string()}]
                                }
                            }))
                        }
                    }
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

pub struct ServeHandle {
    pub join: JoinHandle<Result<(), McpError>>,
}

/// Spin up the MCP server in-process. Returns the client handle the
/// agent runtime uses to send `tools/call` requests, plus a JoinHandle
/// the caller drops at run end to tear down cleanly.
pub fn serve_in_process(
    registry: Arc<Registry>,
    permission: McpPermission,
) -> (InProcessTransport, ServeHandle) {
    let (client_t, server_t) = InProcessTransport::pair();
    let server = McpServer::new(registry, server_t, permission);
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
    use crate::permission::McpPermission;
    use crate::transport::Transport;

    #[tokio::test]
    async fn server_responds_to_initialize_and_tools_list() {
        let registry = Arc::new(Registry::empty());
        let (client, handle) = serve_in_process(registry, McpPermission::allow_all());

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
        assert!(
            tools.len() >= 17,
            "Tasks 11-13 add 17 tools (10 SCM + 5 issues + 2 extras); got {} entries",
            tools.len()
        );

        // tools/call with unknown tool returns isError true (no panic)
        client
            .send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": {"name": "scm.repo.typo", "arguments": {}}
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

    #[tokio::test]
    async fn unknown_tool_call_returns_error_is_error_true() {
        let registry = Arc::new(Registry::empty());
        let (client, handle) = serve_in_process(registry, McpPermission::allow_all());

        client
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "scm.repo.typo", "arguments": {}}
            }))
            .await
            .unwrap();
        let resp = client.recv().await.unwrap().unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("unknown tool"),
            "expected unknown-tool error, got: {text}"
        );

        drop(client);
        let _ = handle.join.await;
    }

    #[tokio::test]
    async fn permission_denied_for_tool_not_in_allowlist() {
        let registry = Arc::new(Registry::empty());
        let perm = McpPermission::new(
            rupu_tools::PermissionMode::Bypass,
            vec!["issues.*".into()], // only allows issues.* tools
        );
        let (client, handle) = serve_in_process(registry, perm);

        client
            .send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "scm.repos.list", "arguments": {}}
            }))
            .await
            .unwrap();
        let resp = client.recv().await.unwrap().unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("permission denied") || text.contains("PermissionDenied"),
            "expected permission-denied, got: {text}"
        );

        drop(client);
        let _ = handle.join.await;
    }
}
