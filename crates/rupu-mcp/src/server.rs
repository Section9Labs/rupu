//! [`CatalogServer`]: JSON-RPC 2.0 over a [`Transport`], serving a resolved
//! grant of the `rupu-tools` catalog.

use crate::error::{McpError, ServeError};
use crate::transport::Transport;
use rupu_tools::call::{call, CallOutcome};
use rupu_tools::{
    AliasScope, AllowAlways, GrantInputs, PermissionMode, PermissionPolicy, Tool, ToolCatalog,
    ToolContext, Unavailable,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// What `rupu mcp serve` serves without `--tools`: the connector tools and
/// the findings tools — the set the MCP server exposed before it became a
/// transport over the catalog.
pub const MCP_DEFAULT_GRANT: &[&str] = &["scm.*", "issues.*", "github.*", "gitlab.*", "findings.*"];

/// Serves the tools of one grant, each called in `ctx` under `policy`.
pub struct CatalogServer<T: Transport + 'static> {
    /// Canonical name → body, for every tool the grant offers.
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
    ctx: ToolContext,
    policy: PermissionPolicy,
    always: Mutex<AllowAlways>,
    transport: T,
}

impl<T: Transport + 'static> CatalogServer<T> {
    /// A server for the grant `tools` (W2 grammar; `None` =
    /// [`MCP_DEFAULT_GRANT`]) over the services `ctx` provides, calling each
    /// tool under `mode` with no operator to prompt (ask = allow, with the
    /// MCP client's own confirmation UX in front of it).
    ///
    /// Returns the tools the grant names but `ctx` can't serve (a launcher,
    /// a message bus, …): not listed, and for the caller to report.
    pub fn new(
        transport: T,
        ctx: ToolContext,
        mode: PermissionMode,
        tools: Option<&[String]>,
    ) -> Result<(Self, Vec<Unavailable>), ServeError> {
        let default: Vec<String> = MCP_DEFAULT_GRANT.iter().map(|s| s.to_string()).collect();
        let grant = ToolCatalog::builtin().resolve_grant(GrantInputs {
            declared: Some(tools.unwrap_or(&default)),
            step_actions: &[],
            ambient: &[],
            available: &ctx.services.provided(false),
            alias_scope: AliasScope::Everywhere,
        })?;
        let mut served = BTreeMap::new();
        for name in grant.entries.keys() {
            // `provided` and `bodies::body` keep the same accounting, so a
            // granted tool always has a body here; if not, refuse to start
            // rather than list a tool that can't run.
            let body = rupu_tools::bodies::body(name, &ctx)
                .ok_or_else(|| ServeError::NoBody(name.to_string()))?;
            served.insert(*name, body);
        }
        Ok((
            Self {
                tools: served,
                ctx,
                policy: PermissionPolicy::unattended(mode),
                always: Mutex::new(AllowAlways::default()),
                transport,
            },
            grant.unavailable,
        ))
    }

    /// The canonical names this server lists, in catalog order.
    pub fn tool_names(&self) -> Vec<&'static str> {
        ToolCatalog::all()
            .iter()
            .map(|d| d.name)
            .filter(|n| self.tools.contains_key(n))
            .collect()
    }

    /// Serve until the transport closes.
    pub async fn run(self) -> Result<(), McpError> {
        loop {
            let msg = match self.transport.recv().await? {
                Some(m) => m,
                None => return Ok(()),
            };
            // A JSON-RPC notification (no `id`, e.g. `notifications/
            // initialized`) is never answered.
            let Some(id) = msg.get("id").cloned() else {
                debug!(method = ?msg.get("method"), "notification");
                continue;
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let response = match method {
                "initialize" => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "serverInfo": { "name": "rupu", "version": env!("CARGO_PKG_VERSION") },
                        "capabilities": { "tools": {} },
                    },
                }),
                "tools/list" => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": self.list() },
                }),
                "tools/call" => {
                    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                    let result = match self.call(name, arguments).await {
                        Ok(text) => json!({ "content": [{"type": "text", "text": text}] }),
                        Err(text) => {
                            warn!(tool = %name, error = %text, "tool call failed");
                            json!({
                                "isError": true,
                                "content": [{"type": "text", "text": text}]
                            })
                        }
                    };
                    json!({ "jsonrpc": "2.0", "id": id, "result": result })
                }
                other => McpError::UnknownMethod(other.to_string()).to_jsonrpc(id),
            };
            self.transport.send(response).await?;
        }
    }

    /// `tools/list`: each served tool with the schema its body offers in
    /// this context.
    fn list(&self) -> Vec<Value> {
        self.tool_names()
            .into_iter()
            .map(|n| {
                let t = &self.tools[n];
                json!({
                    "name": n,
                    "description": t.description(),
                    "inputSchema": t.input_schema(),
                })
            })
            .collect()
    }

    /// `tools/call`: the name (canonical or a legacy alias) must be one this
    /// server serves.
    async fn call(&self, name: &str, arguments: Value) -> Result<String, String> {
        let Some(d) = ToolCatalog::builtin().resolve_name(name, AliasScope::Everywhere) else {
            return Err(format!("unknown tool: {name}"));
        };
        let Some(tool) = self.tools.get(d.name) else {
            return Err(format!(
                "tool `{name}` is not served here (not in this server's --tools grant, or \
                 it needs a service `rupu mcp serve` can't provide)"
            ));
        };
        let mut always = self.always.lock().await;
        match call(&**tool, arguments, &self.ctx, &self.policy, &mut always).await {
            CallOutcome::Done(text) => Ok(text),
            CallOutcome::Denied(reason) => Err(format!("permission denied: {reason}")),
            CallOutcome::Failed(text) => Err(text),
        }
    }
}
