//! [`McpServer`]: JSON-RPC 2.0 over a [`Transport`] (or per HTTP request),
//! serving the tools of its [`ToolSource`]s.
//!
//! Every `tools/call` runs as its own task, so a slow call (a `bash`, a
//! connector) never holds up a `ping` or another call, and a
//! `notifications/cancelled` naming its request id aborts it (no reply is
//! sent for a cancelled request).

use crate::error::{McpError, ServeError};
use crate::filter::ToolFilter;
use crate::source::{CallCx, Progress, ToolDef, ToolResult, ToolSource};
use crate::transport::Transport;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};
use tracing::{debug, warn};

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

async fn send_logged<T: Transport>(transport: &T, msg: Value) {
    if let Err(e) = transport.send(msg).await {
        warn!(error = %e, "mcp reply not sent");
    }
}

// ---------------------------------------------------------------------------
// McpServer: the protocol core over pluggable tool sources.
// ---------------------------------------------------------------------------

/// What the transport knows about the request a message arrived on.
#[derive(Debug, Clone, Default)]
pub struct HandleCx<'a> {
    pub session: Option<&'a str>,
    /// The request's `Host` header.
    pub host: Option<&'a str>,
    /// Where the transport takes server-to-client notifications. A
    /// `tools/call` carrying `params._meta.progressToken` reports progress
    /// here; with `None` no call gets a progress channel.
    pub notify: Option<mpsc::UnboundedSender<Value>>,
}

/// JSON-RPC 2.0 over any transport, serving the tools of its
/// [`ToolSource`]s. `handle` takes one message (or batch) and returns the
/// reply, so an HTTP transport can call it per request; `run` drives a
/// [`Transport`] (tests).
pub struct McpServer {
    core: Arc<Core>,
}

struct Core {
    /// Listed tools, in source order.
    defs: Vec<ToolDef>,
    /// Tool name → the source index that serves it.
    owner: HashMap<String, usize>,
    sources: Vec<Arc<dyn ToolSource>>,
    in_flight: Mutex<SessionFlight>,
}

/// In-flight `tools/call`s keyed by (session, request id).
#[derive(Default)]
struct SessionFlight {
    next: u64,
    calls: HashMap<(String, String), (u64, AbortHandle)>,
}

struct Deregister {
    core: Arc<Core>,
    key: (String, String),
    generation: u64,
}

impl Drop for Deregister {
    fn drop(&mut self) {
        let mut f = self
            .core
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if f.calls.get(&self.key).map(|(g, _)| *g) == Some(self.generation) {
            f.calls.remove(&self.key);
        }
    }
}

impl McpServer {
    /// Lists every tool of `sources` that `filter` keeps. A name served by
    /// two sources is `ServeError::Duplicate(name)`.
    pub fn new(sources: Vec<Arc<dyn ToolSource>>, filter: &ToolFilter) -> Result<Self, ServeError> {
        let mut defs = Vec::new();
        let mut owner: HashMap<String, usize> = HashMap::new();
        let all: Vec<_> = sources.iter().map(|s| s.tools()).collect();
        let names: Vec<String> = all.iter().flatten().map(|d| d.name.clone()).collect();
        let kept = filter.apply(&names)?;
        for (i, tools) in all.into_iter().enumerate() {
            for def in tools {
                if !kept.contains(&def.name) {
                    continue;
                }
                if owner.insert(def.name.clone(), i).is_some() {
                    return Err(ServeError::Duplicate(def.name));
                }
                defs.push(def);
            }
        }
        Ok(Self {
            core: Arc::new(Core {
                defs,
                owner,
                sources,
                in_flight: Mutex::new(SessionFlight::default()),
            }),
        })
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.core.defs.iter().map(|d| d.name.clone()).collect()
    }

    /// One JSON-RPC message or batch → the reply (`None` for notifications,
    /// client responses, and a cancelled call). `session` keys in-flight
    /// calls, so a cancel only reaches its own session's call.
    ///
    /// A `tools/call` runs as its own task: if the caller drops this future
    /// (a client disconnect), the call keeps running to completion. That is
    /// deliberate, so a disconnect never aborts a write half-way; only
    /// `notifications/cancelled` stops a call.
    pub async fn handle(&self, msg: Value, session: Option<&str>) -> Option<Value> {
        self.handle_with(
            msg,
            HandleCx {
                session,
                ..HandleCx::default()
            },
        )
        .await
    }

    /// [`McpServer::handle`] with the request's context.
    pub async fn handle_with(&self, msg: Value, cx: HandleCx<'_>) -> Option<Value> {
        let session = cx.session.map(str::to_string);
        let host = cx.host.map(str::to_string);
        let notify = cx.notify;
        match msg {
            Value::Array(batch) if batch.is_empty() => {
                Some(McpError::InvalidRequest("empty batch".into()).to_jsonrpc(Value::Null))
            }
            Value::Array(batch) => {
                // Started together (a cancel in a batch can reach a call
                // in it), answered in order.
                let tasks: Vec<_> = batch
                    .into_iter()
                    .map(|m| {
                        let id = m.get("id").cloned();
                        (
                            id,
                            tokio::spawn(self.core.clone().one(
                                m,
                                session.clone(),
                                host.clone(),
                                notify.clone(),
                            )),
                        )
                    })
                    .collect();
                let mut out = Vec::new();
                for (id, t) in tasks {
                    match t.await {
                        Ok(Some(v)) => out.push(v),
                        Ok(None) => {}
                        // A cancelled call is never answered.
                        Err(e) if e.is_cancelled() => {}
                        Err(e) => {
                            warn!(error = %e, "mcp request panicked");
                            if let Some(id) = id {
                                let e = McpError::Transport(anyhow::anyhow!("request panicked"));
                                out.push(e.to_jsonrpc(id));
                            }
                        }
                    }
                }
                (!out.is_empty()).then_some(Value::Array(out))
            }
            msg => self.core.clone().one(msg, session, host, notify).await,
        }
    }

    /// Serve a transport until it closes (tests; `InProcessTransport`).
    /// What is running when it closes finishes and is answered; a transport
    /// error aborts it.
    pub async fn run<T: Transport + 'static>(
        self: Arc<Self>,
        transport: T,
    ) -> Result<(), McpError> {
        let transport = Arc::new(transport);
        let mut replies = JoinSet::new();
        let result = loop {
            while replies.try_join_next().is_some() {}
            match transport.recv().await {
                Ok(Some(msg)) => {
                    let this = self.clone();
                    let t = transport.clone();
                    replies.spawn(async move {
                        // Notifications reach the transport before the reply:
                        // the forwarder ends once every sender is dropped,
                        // which is when the call has ended.
                        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
                        let fwd = tokio::spawn({
                            let t = t.clone();
                            async move {
                                while let Some(n) = rx.recv().await {
                                    send_logged(&*t, n).await;
                                }
                            }
                        });
                        let reply = this
                            .handle_with(
                                msg,
                                HandleCx {
                                    notify: Some(tx),
                                    ..HandleCx::default()
                                },
                            )
                            .await;
                        let _ = fwd.await;
                        if let Some(v) = reply {
                            send_logged(&*t, v).await;
                        }
                    });
                }
                Ok(None) => break Ok(()),
                Err(e @ McpError::Parse(_)) => {
                    warn!(error = %e, "malformed JSON-RPC line");
                    if let Err(e) = transport.send(e.to_jsonrpc(Value::Null)).await {
                        break Err(e);
                    }
                }
                Err(e) => break Err(e),
            }
        };
        if result.is_err() {
            self.core.abort_all();
        }
        while replies.join_next().await.is_some() {}
        result
    }
}

impl Core {
    /// One non-batch message.
    async fn one(
        self: Arc<Self>,
        msg: Value,
        session: Option<String>,
        host: Option<String>,
        notify: Option<mpsc::UnboundedSender<Value>>,
    ) -> Option<Value> {
        let Value::Object(msg) = msg else {
            let e = McpError::InvalidRequest("not a JSON-RPC object".into());
            return Some(e.to_jsonrpc(Value::Null));
        };
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response to a request of ours (we send none) needs no answer.
            if msg.contains_key("result") || msg.contains_key("error") {
                return None;
            }
            let e = McpError::InvalidRequest("no method".into());
            return Some(e.to_jsonrpc(id.unwrap_or(Value::Null)));
        };
        let Some(id) = id else {
            match method {
                "notifications/cancelled" => {
                    if let Some(request_id) = params.get("requestId") {
                        self.cancel(session.as_deref(), request_id);
                    }
                }
                _ => debug!(method, "notification"),
            }
            return None;
        };
        let ok = |result: Value| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                ok(json!({
                    "protocolVersion": negotiate_protocol_version(requested),
                    "serverInfo": { "name": "rupu", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": { "tools": {} },
                }))
            }
            "ping" => ok(json!({})),
            "tools/list" => ok(json!({ "tools": self.list() })),
            "tools/call" => self.tools_call(id, params, session, host, notify).await,
            other => Some(McpError::UnknownMethod(other.to_string()).to_jsonrpc(id)),
        }
    }

    fn list(&self) -> Vec<Value> {
        self.defs
            .iter()
            .map(|d| {
                json!({
                    "name": d.name,
                    "description": d.description,
                    "inputSchema": d.input_schema,
                    "annotations": d.hints.to_annotations(),
                })
            })
            .collect()
    }

    /// Run the call as its own task, registered under (session, id) until
    /// it ends; a cancel aborts it and then no reply is sent.
    async fn tools_call(
        self: &Arc<Self>,
        id: Value,
        params: Value,
        session: Option<String>,
        host: Option<String>,
        notify: Option<mpsc::UnboundedSender<Value>>,
    ) -> Option<Value> {
        let progress = notify.and_then(|tx| {
            let token = params.pointer("/_meta/progressToken")?.clone();
            Some(Progress::new(token, tx))
        });
        let key = (session.clone().unwrap_or_default(), id.to_string());
        let core = self.clone();
        let reply_id = id.clone();
        let task = {
            // Held across the spawn, so the task can't end (and deregister)
            // before it is registered.
            let mut flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
            let generation = flight.next;
            flight.next += 1;
            let guard = Deregister {
                core: core.clone(),
                key: key.clone(),
                generation,
            };
            let task = tokio::spawn(async move {
                let _guard = guard;
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = core
                    .call(
                        name,
                        args,
                        CallCx {
                            progress,
                            session,
                            host,
                        },
                    )
                    .await;
                json!({ "jsonrpc": "2.0", "id": id, "result": result })
            });
            if flight
                .calls
                .insert(key, (generation, task.abort_handle()))
                .is_some()
            {
                warn!("a tools/call reused the id of a call still running");
            }
            task
        };
        match task.await {
            Ok(v) => Some(v),
            // A cancel: no reply.
            Err(e) if e.is_cancelled() => None,
            Err(e) => {
                warn!(error = %e, "tool call panicked");
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": reply_id,
                    "result": {
                        "isError": true,
                        "content": [{"type": "text", "text": "tool call panicked"}],
                    },
                }))
            }
        }
    }

    async fn call(&self, name: &str, args: Value, cx: CallCx) -> Value {
        let result = match self.owner.get(name) {
            Some(&i) => self.sources[i].call(name, args, &cx).await,
            None => None,
        };
        match result {
            // `structuredContent` must be an object (clients reject anything
            // else); any other JSON value goes as the text block only.
            Some(ToolResult::Json(v)) if v.is_object() => json!({
                "content": [{"type": "text", "text": v.to_string()}],
                "structuredContent": v,
            }),
            Some(ToolResult::Json(v)) => json!({
                "content": [{"type": "text", "text": v.to_string()}],
            }),
            Some(ToolResult::Annotated { value, notes, meta }) => {
                let mut content = vec![json!({"type": "text", "text": value.to_string()})];
                content.extend(
                    notes
                        .into_iter()
                        .map(|n| json!({"type": "text", "text": n})),
                );
                let mut out = json!({ "content": content, "_meta": meta });
                if value.is_object() {
                    out["structuredContent"] = value;
                }
                out
            }
            Some(ToolResult::Resource(r)) => json!({ "content": [r.to_content()] }),
            Some(ToolResult::Text(t)) => json!({ "content": [{"type": "text", "text": t}] }),
            Some(ToolResult::Error(t)) => {
                warn!(tool = %name, error = %t, "tool call failed");
                json!({ "isError": true, "content": [{"type": "text", "text": t}] })
            }
            None => json!({
                "isError": true,
                "content": [{"type": "text", "text": format!("unknown tool: {name}")}],
            }),
        }
    }

    fn cancel(&self, session: Option<&str>, request_id: &Value) {
        let key = (session.unwrap_or("").to_string(), request_id.to_string());
        let call = self
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .remove(&key);
        match call {
            Some((_, task)) => {
                debug!(request_id = %key.1, "tools/call cancelled");
                task.abort();
            }
            None => debug!(request_id = %key.1, "cancel for no running call"),
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
}
