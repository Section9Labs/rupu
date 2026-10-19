//! [`CatalogServer`]: JSON-RPC 2.0 over a [`Transport`], serving a resolved
//! grant of the `rupu-tools` catalog.
//!
//! Every `tools/call` runs as its own task, so a slow call (a `bash`, a
//! connector) never holds up a `ping` or another call, and a
//! `notifications/cancelled` naming its request id aborts it (no reply is
//! sent for a cancelled request).

use crate::error::{McpError, ServeError};
use crate::transport::Transport;
use rupu_tools::call::{call_shared, CallOutcome};
use rupu_tools::{
    AliasScope, AllowAlways, GrantInputs, PermissionMode, PermissionPolicy, Tool, ToolCatalog,
    ToolContext, Unavailable,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};
use tracing::{debug, warn};

/// What `rupu mcp serve` serves without `--tools`: the connector tools and
/// the findings tools — the set the MCP server exposed before it became a
/// transport over the catalog.
pub const MCP_DEFAULT_GRANT: &[&str] = &["scm.*", "issues.*", "github.*", "gitlab.*", "findings.*"];

/// The protocol revisions this server speaks, newest first. All of them use
/// the `initialize` handshake and the same `tools/*` wire shapes; a later
/// revision belongs here only when that still holds (2026-07-28 dropped the
/// handshake, so it does not).
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The revision to answer an `initialize` with: the client's, when this
/// server speaks it, else the newest this server speaks (the client then
/// decides whether it can go on).
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| PROTOCOL_VERSIONS.iter().find(|v| **v == r))
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

/// Serves the tools of one grant, each called in `ctx` under `policy`.
pub struct CatalogServer<T: Transport + 'static> {
    inner: Arc<Inner>,
    transport: Arc<T>,
}

struct Inner {
    /// Canonical name → body, for every tool the grant offers.
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
    ctx: ToolContext,
    policy: PermissionPolicy,
    /// Shared by concurrent calls; locked only while a call is decided.
    always: Mutex<AllowAlways>,
    in_flight: Mutex<InFlight>,
}

/// The `tools/call`s still running, by request id, so a cancel can abort
/// one.
#[derive(Default)]
struct InFlight {
    next: u64,
    /// Request id (its JSON text) → the call's generation and task. The
    /// generation keeps a finished call from removing a newer call that
    /// reused its id.
    calls: HashMap<String, (u64, AbortHandle)>,
}

/// Removes a call from [`InFlight`] when its task ends, finished or
/// aborted.
struct Registered {
    inner: Arc<Inner>,
    key: String,
    generation: u64,
}

impl Drop for Registered {
    fn drop(&mut self) {
        let mut in_flight = self
            .inner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if in_flight.calls.get(&self.key).map(|(g, _)| *g) == Some(self.generation) {
            in_flight.calls.remove(&self.key);
        }
    }
}

/// How one message is answered.
enum Reply {
    /// Right away.
    Now(Value),
    /// When the call task ends; never, if it is cancelled.
    Call(JoinHandle<Value>),
}

impl Reply {
    async fn resolve(self) -> Option<Value> {
        match self {
            Reply::Now(v) => Some(v),
            Reply::Call(h) => h.await.ok(),
        }
    }
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
            connector_cap: None,
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
                inner: Arc::new(Inner {
                    tools: served,
                    ctx,
                    policy: PermissionPolicy::unattended(mode),
                    always: Mutex::new(AllowAlways::default()),
                    in_flight: Mutex::new(InFlight::default()),
                }),
                transport: Arc::new(transport),
            },
            grant.unavailable,
        ))
    }

    /// The canonical names this server lists, in catalog order.
    pub fn tool_names(&self) -> Vec<&'static str> {
        self.inner.tool_names()
    }

    /// Serve until the transport closes, then let the calls still running
    /// finish and answer them. A transport error ends the server and aborts
    /// what is running.
    pub async fn run(self) -> Result<(), McpError> {
        let mut replies = JoinSet::new();
        let result = self.serve(&mut replies).await;
        if result.is_err() {
            self.inner.abort_all();
        }
        while replies.join_next().await.is_some() {}
        result
    }

    async fn serve(&self, replies: &mut JoinSet<()>) -> Result<(), McpError> {
        loop {
            while replies.try_join_next().is_some() {}
            let msg = match self.transport.recv().await {
                Ok(Some(m)) => m,
                Ok(None) => return Ok(()),
                // One bad line is answered, never fatal: the next request
                // still gets its reply.
                Err(e @ McpError::Parse(_)) => {
                    warn!(error = %e, "malformed JSON-RPC line");
                    self.transport.send(e.to_jsonrpc(Value::Null)).await?;
                    continue;
                }
                Err(e) => return Err(e),
            };
            match msg {
                Value::Array(batch) if batch.is_empty() => {
                    let e = McpError::InvalidRequest("empty batch".into());
                    self.transport.send(e.to_jsonrpc(Value::Null)).await?;
                }
                // A batch is answered with one array of the replies its
                // requests get, in order, once all of them are in.
                Value::Array(batch) => {
                    let pending: Vec<Reply> =
                        batch.into_iter().filter_map(|m| self.dispatch(m)).collect();
                    if pending.is_empty() {
                        continue;
                    }
                    let transport = self.transport.clone();
                    replies.spawn(async move {
                        let mut out = Vec::with_capacity(pending.len());
                        for reply in pending {
                            out.extend(reply.resolve().await);
                        }
                        if !out.is_empty() {
                            send_logged(&*transport, Value::Array(out)).await;
                        }
                    });
                }
                msg => match self.dispatch(msg) {
                    None => {}
                    Some(Reply::Now(v)) => self.transport.send(v).await?,
                    Some(call) => {
                        let transport = self.transport.clone();
                        replies.spawn(async move {
                            if let Some(v) = call.resolve().await {
                                send_logged(&*transport, v).await;
                            }
                        });
                    }
                },
            }
        }
    }

    /// One message: how it is answered, or `None` for a notification or a
    /// client's response.
    fn dispatch(&self, msg: Value) -> Option<Reply> {
        let Value::Object(msg) = msg else {
            let e = McpError::InvalidRequest("not a JSON-RPC object".into());
            return Some(Reply::Now(e.to_jsonrpc(Value::Null)));
        };
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response to a request of ours (we send none) needs no answer.
            if msg.contains_key("result") || msg.contains_key("error") {
                return None;
            }
            let e = McpError::InvalidRequest("no method".into());
            return Some(Reply::Now(e.to_jsonrpc(id.unwrap_or(Value::Null))));
        };
        // A JSON-RPC notification (no `id`) is never answered.
        let Some(id) = id else {
            match method {
                "notifications/cancelled" => {
                    if let Some(request_id) = params.get("requestId") {
                        self.inner.cancel(request_id);
                    }
                }
                _ => debug!(method, "notification"),
            }
            return None;
        };
        let ok =
            |result: Value| Reply::Now(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        Some(match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                ok(json!({
                    "protocolVersion": negotiate_protocol_version(requested),
                    "serverInfo": { "name": "rupu", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": { "tools": {} },
                }))
            }
            "ping" => ok(json!({})),
            "tools/list" => ok(json!({ "tools": self.inner.list() })),
            "tools/call" => Reply::Call(self.spawn_call(id, params)),
            other => Reply::Now(McpError::UnknownMethod(other.to_string()).to_jsonrpc(id)),
        })
    }

    /// Start one `tools/call` as its own task, registered under its request
    /// id until it ends.
    fn spawn_call(&self, id: Value, params: Value) -> JoinHandle<Value> {
        let key = id.to_string();
        let inner = self.inner.clone();
        // Held across the spawn, so the task can't end (and deregister)
        // before it is registered.
        let mut in_flight = self
            .inner
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let generation = in_flight.next;
        in_flight.next += 1;
        let registered = Registered {
            inner: inner.clone(),
            key: key.clone(),
            generation,
        };
        let task = tokio::spawn(async move {
            let _registered = registered;
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let result = match inner.call(name, arguments).await {
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
        });
        let older = in_flight
            .calls
            .insert(key, (generation, task.abort_handle()));
        if older.is_some() {
            // A client must not reuse a running request's id; if it does,
            // the newer call is the one a cancel reaches.
            warn!("a tools/call reused the id of a call still running");
        }
        task
    }
}

async fn send_logged<T: Transport>(transport: &T, msg: Value) {
    if let Err(e) = transport.send(msg).await {
        warn!(error = %e, "mcp reply not sent");
    }
}

impl Inner {
    fn tool_names(&self) -> Vec<&'static str> {
        ToolCatalog::all()
            .iter()
            .map(|d| d.name)
            .filter(|n| self.tools.contains_key(n))
            .collect()
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

    /// `notifications/cancelled`: abort the call running under
    /// `request_id`, if any (it may have finished already).
    fn cancel(&self, request_id: &Value) {
        let key = request_id.to_string();
        let call = self
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .remove(&key);
        match call {
            Some((_, task)) => {
                debug!(request_id = %key, "tools/call cancelled");
                task.abort();
            }
            None => debug!(request_id = %key, "cancel for no running call"),
        }
    }

    fn abort_all(&self) {
        let calls = std::mem::take(
            &mut self
                .in_flight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .calls,
        );
        for (_, task) in calls.into_values() {
            task.abort();
        }
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
        match call_shared(&**tool, arguments, &self.ctx, &self.policy, &self.always).await {
            CallOutcome::Done(text) => Ok(text),
            CallOutcome::Denied(reason) => Err(format!("permission denied: {reason}")),
            CallOutcome::Failed(text) => Err(text),
        }
    }
}
