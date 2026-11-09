//! Bounded outbound Streamable HTTP tools client. No runtime grants, CP
//! operator adapter, provider calls or automatic server discovery live here.
//! Callers select a trusted host configuration and retain client-bound tools.
mod sse;
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    Client, Response,
};
use rupu_config::mcp::{Registry, ToolEffect};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;

pub const PROTOCOL_VERSION: &str = "2025-11-25";
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOOL_BYTES: usize = 256 * 1024;
const SESSION: &str = "mcp-session-id";
const VERSION: &str = "mcp-protocol-version";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid MCP configuration or selection")]
    Configuration,
    #[error("MCP credential is missing or invalid")]
    Credential,
    #[error("MCP transport failed")]
    Transport,
    #[error("MCP request timed out")]
    Timeout,
    #[error("MCP response exceeded a bounded limit")]
    Limit,
    #[error("invalid MCP response: {0}")]
    Protocol(&'static str),
    #[error("MCP server returned HTTP {0}")]
    Http(u16),
    #[error("MCP server returned JSON-RPC error {0}")]
    Rpc(i64),
    #[error("MCP session expired; reconnect before another operation")]
    Expired,
    #[error("MCP tool catalog changed; rediscover on a new connection")]
    Stale,
    #[error("MCP tool belongs to another connection")]
    WrongOwner,
    #[error("the Rupu operator server cannot be attached as a session tool server")]
    OperatorServer,
    #[error("MCP call outcome is uncertain; the call was not retried")]
    Uncertain,
}

/// Resolves only an explicitly configured MCP credential reference. Values
/// never enter registry serialization, tool metadata, Debug or error strings.
pub trait Credentials: Send + Sync {
    fn bearer(&self, reference: &str) -> Result<String, Error>;
}
pub struct EnvironmentCredentials;
impl Credentials for EnvironmentCredentials {
    fn bearer(&self, reference: &str) -> Result<String, Error> {
        std::env::var(reference).map_err(|_| Error::Credential)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Deadlines {
    pub metadata: Duration,
    pub call: Duration,
}
impl Default for Deadlines {
    fn default() -> Self {
        Self {
            metadata: Duration::from_secs(30),
            call: Duration::from_secs(120),
        }
    }
}
impl Deadlines {
    fn validate(self) -> Result<(), Error> {
        if self.metadata.is_zero()
            || self.call.is_zero()
            || self.metadata > Duration::from_secs(120)
            || self.call > Duration::from_secs(120)
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
}

/// Explicit capture ownership for one initialized client. Create a new client
/// for each run/owner; no process-global sink or first-run client cache exists.
/// Discovery outside a run may explicitly supply a NullSink.
pub struct Capture {
    pub context: rupu_netflow::FlowCtx,
    pub sink: Arc<dyn rupu_netflow::FlowSink>,
}

/// Immutable discovery handle. Private owner prevents another client/server
/// from substituting a same-named tool. No URL or credential is exposed.
#[derive(Clone)]
pub struct DiscoveredTool {
    owner: Arc<()>,
    server_id: Arc<str>,
    local_effect: ToolEffect,
    metadata: Arc<Value>,
    name: String,
}
impl std::fmt::Debug for DiscoveredTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveredTool")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}
impl DiscoveredTool {
    pub fn server_id(&self) -> &str {
        &self.server_id
    }
    /// Trusted host policy captured at connect, never a remote annotation.
    pub fn local_effect(&self) -> ToolEffect {
        self.local_effect
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn metadata(&self) -> &Value {
        &self.metadata
    }
}
#[derive(Debug)]
pub struct CallResult {
    pub content: Vec<Value>,
    pub structured_content: Option<Value>,
    pub is_error: bool,
}

// Declared after the serial guard: cancellation latches uncertainty BEFORE
// releasing that mutex, so another caller cannot overtake an abandoned POST.
struct InFlight<'a> {
    uncertain: &'a AtomicBool,
    armed: bool,
}
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.uncertain.store(true, Ordering::SeqCst);
        }
    }
}

pub struct HttpToolsClient {
    http: reqwest_middleware::ClientWithMiddleware,
    server_id: Arc<str>,
    tool_effects: BTreeMap<String, ToolEffect>,
    endpoint: url::Url,
    headers: HeaderMap,
    session: Option<HeaderValue>,
    initialized: bool,
    owner: Arc<()>,
    next_id: AtomicU64,
    invalidated: AtomicBool,
    expired: AtomicBool,
    uncertain: AtomicBool,
    serial: Mutex<()>,
    discovered: std::sync::Mutex<Option<Vec<DiscoveredTool>>>,
    deadlines: Deadlines,
}
impl std::fmt::Debug for HttpToolsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpToolsClient")
            .field("initialized", &self.initialized)
            .finish_non_exhaustive()
    }
}
impl HttpToolsClient {
    /// This explicit operation may connect and resolve the chosen MCP secret.
    /// Merely loading/inspecting host configuration never does either.
    pub async fn connect(
        registry: &Registry,
        server_id: &str,
        credentials: &dyn Credentials,
        deadlines: Deadlines,
        capture: Capture,
    ) -> Result<Self, Error> {
        deadlines.validate()?;
        let config = registry
            .enabled_server(server_id)
            .map_err(|_| Error::Configuration)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        if let Some(reference) = &config.bearer_env {
            let secret = credentials.bearer(reference)?;
            if secret.is_empty() || secret.len() > 8192 {
                return Err(Error::Credential);
            }
            let mut value = HeaderValue::from_str(&format!("Bearer {secret}"))
                .map_err(|_| Error::Credential)?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(10));
        let mut flow_context = capture.context;
        flow_context.origin = rupu_netflow::Origin::Mcp(server_id.to_owned());
        let http = rupu_netflow::http::client_with(flow_context, builder, capture.sink)
            .map_err(|_| Error::Transport)?;
        let mut client = Self {
            http,
            server_id: Arc::from(server_id),
            tool_effects: config.tool_effects.clone(),
            endpoint: url::Url::parse(&config.url).map_err(|_| Error::Configuration)?,
            headers,
            session: None,
            initialized: false,
            owner: Arc::new(()),
            next_id: AtomicU64::new(1),
            invalidated: AtomicBool::new(false),
            expired: AtomicBool::new(false),
            uncertain: AtomicBool::new(false),
            serial: Mutex::new(()),
            discovered: std::sync::Mutex::new(None),
            deadlines,
        };
        let (reply, mut session) = client
            .request(
                "initialize",
                json!({
                    "protocolVersion":PROTOCOL_VERSION,"capabilities":{},
                    "clientInfo":{"name":"rupu-session-tools","version":env!("CARGO_PKG_VERSION")}
                }),
                deadlines.metadata,
            )
            .await?;
        if reply["protocolVersion"].as_str() != Some(PROTOCOL_VERSION) {
            return Err(Error::Protocol("unsupported version"));
        }
        let server_name = reply
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .ok_or(Error::Protocol("missing server identity"))?;
        if server_name.is_empty()
            || server_name.len() > 256
            || !reply
                .pointer("/serverInfo/version")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty() && v.len() <= 256)
        {
            return Err(Error::Protocol("invalid server identity"));
        }
        if matches!(server_name, "rupu" | "rupu-cp") {
            return Err(Error::OperatorServer);
        }
        if !reply
            .pointer("/capabilities/tools")
            .is_some_and(Value::is_object)
        {
            return Err(Error::Protocol("server has no tools capability"));
        }
        if let Some(value) = &mut session {
            value.set_sensitive(true);
        }
        client.session = session;
        client.initialized = true;
        client
            .notification("notifications/initialized", None, deadlines.metadata)
            .await?;
        Ok(client)
    }

    fn usable(&self) -> Result<(), Error> {
        if self.uncertain.load(Ordering::SeqCst) {
            Err(Error::Uncertain)
        } else if self.expired.load(Ordering::SeqCst) {
            Err(Error::Expired)
        } else if self.invalidated.load(Ordering::SeqCst) {
            Err(Error::Stale)
        } else {
            Ok(())
        }
    }
    pub async fn discover(&self) -> Result<Vec<DiscoveredTool>, Error> {
        let _guard = self.serial.lock().await;
        self.usable()?;
        if let Some(tools) = self
            .discovered
            .lock()
            .map_err(|_| Error::Protocol("catalog lock"))?
            .clone()
        {
            return Ok(tools);
        }
        let tools = tokio::time::timeout(self.deadlines.metadata, self.discover_inner())
            .await
            .map_err(|_| Error::Timeout)??;
        *self
            .discovered
            .lock()
            .map_err(|_| Error::Protocol("catalog lock"))? = Some(tools.clone());
        Ok(tools)
    }
    async fn discover_inner(&self) -> Result<Vec<DiscoveredTool>, Error> {
        let mut tools = Vec::new();
        let mut names = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        let mut cursor = None;
        let mut metadata_bytes = 0;
        for _ in 0..32 {
            let params = cursor
                .as_ref()
                .map(|c| json!({"cursor":c}))
                .unwrap_or_else(|| json!({}));
            let (reply, _) = self
                .request("tools/list", params, self.deadlines.metadata)
                .await?;
            let list = reply["tools"]
                .as_array()
                .ok_or(Error::Protocol("missing tools array"))?;
            for metadata in list {
                if tools.len() == 256 {
                    return Err(Error::Limit);
                }
                let bytes = serde_json::to_vec(metadata)
                    .map_err(|_| Error::Protocol("tool metadata"))?
                    .len();
                metadata_bytes += bytes;
                if bytes > MAX_TOOL_BYTES || metadata_bytes > MAX_BYTES {
                    return Err(Error::Limit);
                }
                let name = metadata["name"]
                    .as_str()
                    .ok_or(Error::Protocol("missing tool name"))?;
                if name.is_empty()
                    || name.len() > 256
                    || name.trim() != name
                    || name.chars().any(char::is_control)
                    || !metadata["inputSchema"].is_object()
                    || metadata["inputSchema"]["type"] != "object"
                    || metadata
                        .get("description")
                        .is_some_and(|d| d.as_str().is_none_or(|s| s.len() > 16_384))
                {
                    return Err(Error::Protocol("invalid tool metadata"));
                }
                if let Some(execution) = metadata.get("execution") {
                    if !execution.is_object() {
                        return Err(Error::Protocol("invalid tool execution metadata"));
                    }
                    if let Some(support) = execution.get("taskSupport") {
                        match support.as_str() {
                            Some("forbidden" | "optional") => {}
                            Some("required") => {
                                return Err(Error::Protocol("task-required tools are unsupported"))
                            }
                            _ => return Err(Error::Protocol("invalid task support")),
                        }
                    }
                }
                if !names.insert(name.to_owned()) {
                    return Err(Error::Protocol("duplicate tool name"));
                }
                tools.push(DiscoveredTool {
                    owner: self.owner.clone(),
                    server_id: self.server_id.clone(),
                    local_effect: self.tool_effects.get(name).copied().unwrap_or_default(),
                    name: name.into(),
                    metadata: Arc::new(metadata.clone()),
                });
            }
            self.usable()?;
            match reply.get("nextCursor") {
                None => return Ok(tools),
                Some(value) => {
                    let next = value
                        .as_str()
                        .filter(|c| !c.is_empty() && c.len() <= 4096)
                        .ok_or(Error::Protocol("invalid cursor"))?;
                    if !cursors.insert(next.to_owned()) {
                        return Err(Error::Protocol("repeated cursor"));
                    }
                    cursor = Some(next.to_owned());
                }
            }
        }
        Err(Error::Limit)
    }
    pub async fn call(&self, tool: &DiscoveredTool, arguments: Value) -> Result<CallResult, Error> {
        if !Arc::ptr_eq(&self.owner, &tool.owner) {
            return Err(Error::WrongOwner);
        }
        if !arguments.is_object()
            || serde_json::to_vec(&arguments)
                .map_err(|_| Error::Configuration)?
                .len()
                > MAX_TOOL_BYTES
        {
            return Err(Error::Configuration);
        }
        let _guard = self.serial.lock().await;
        self.usable()?;
        let mut flight = InFlight {
            uncertain: &self.uncertain,
            armed: true,
        };
        let result = self
            .request(
                "tools/call",
                json!({"name":tool.name,"arguments":arguments}),
                self.deadlines.call,
            )
            .await;
        // A valid remote error/refusal is an authoritative response, not
        // evidence of rollback or permission to retry the same operation.
        if matches!(
            &result,
            Err(Error::Rpc(_) | Error::Http(400..=499) | Error::Expired)
        ) {
            flight.armed = false;
        }
        let (reply, _) = result.map_err(|e| match e {
            Error::Transport
            | Error::Timeout
            | Error::Protocol(_)
            | Error::Limit
            | Error::Http(300..=399)
            | Error::Http(500..=599) => self.mark_uncertain(),
            other => other,
        })?;
        let content = reply
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| self.mark_uncertain())?;
        if content.len() > 512 || content.iter().any(|block| !valid_content(block)) {
            return Err(self.mark_uncertain());
        }
        let is_error = match reply.get("isError") {
            None => false,
            Some(v) => v.as_bool().ok_or_else(|| self.mark_uncertain())?,
        };
        let structured_content = reply.get("structuredContent").cloned();
        if structured_content.as_ref().is_some_and(|v| !v.is_object()) {
            return Err(self.mark_uncertain());
        }
        flight.armed = false;
        Ok(CallResult {
            content: content.clone(),
            structured_content,
            is_error,
        })
    }

    fn mark_uncertain(&self) -> Error {
        self.uncertain.store(true, Ordering::SeqCst);
        Error::Uncertain
    }

    fn builder(&self, method: reqwest::Method) -> reqwest_middleware::RequestBuilder {
        let mut request = self
            .http
            .request(method, self.endpoint.clone())
            .headers(self.headers.clone());
        if let Some(session) = &self.session {
            request = request.header(SESSION, session.clone());
        }
        if self.initialized {
            request = request.header(VERSION, PROTOCOL_VERSION);
        }
        request
    }
    async fn notification(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<(), Error> {
        let mut body = json!({"jsonrpc":"2.0","method":method});
        if let Some(params) = params {
            body["params"] = params;
        }
        self.send_auxiliary(body, timeout).await
    }
    async fn send_auxiliary(&self, body: Value, timeout: Duration) -> Result<(), Error> {
        tokio::time::timeout(timeout, async {
            let response = self
                .builder(reqwest::Method::POST)
                .json(&body)
                .send()
                .await
                .map_err(|_| Error::Transport)?;
            if response.status().as_u16() != 202 {
                return Err(Error::Http(response.status().as_u16()));
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<(Value, Option<HeaderValue>), Error> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        match tokio::time::timeout(timeout, self.exchange(body, id)).await {
            Ok(result) => result,
            Err(_) => {
                if self.initialized {
                    let _ = self
                        .notification(
                            "notifications/cancelled",
                            Some(json!({"requestId":id,"reason":"deadline"})),
                            Duration::from_secs(1),
                        )
                        .await;
                }
                Err(Error::Timeout)
            }
        }
    }
    async fn exchange(&self, body: Value, id: u64) -> Result<(Value, Option<HeaderValue>), Error> {
        let response = self
            .builder(reqwest::Method::POST)
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        if response.status().as_u16() == 404 && self.session.is_some() {
            self.expired.store(true, Ordering::SeqCst);
            return Err(Error::Expired);
        }
        if !response.status().is_success() {
            return Err(Error::Http(response.status().as_u16()));
        }
        let session = response.headers().get(SESSION).cloned();
        if let Some(value) = &session {
            let bytes = value.as_bytes();
            if bytes.is_empty()
                || bytes.len() > 256
                || !bytes.iter().all(|b| (0x21..=0x7e).contains(b))
            {
                return Err(Error::Protocol("invalid session header"));
            }
            if self
                .session
                .as_ref()
                .is_some_and(|expected| expected != value)
            {
                return Err(Error::Protocol("session changed"));
            }
        }
        let payload = self.read_response(response, id).await?;
        Ok((payload, session))
    }
    async fn read_response(&self, response: Response, id: u64) -> Result<Value, Error> {
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if content_type != "application/json" && content_type != "text/event-stream" {
            return Err(Error::Protocol("unsupported content type"));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        let mut decoder = sse::Decoder::default();
        let mut total = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Error::Transport)?;
            total = total.checked_add(chunk.len()).ok_or(Error::Limit)?;
            if total > MAX_BYTES {
                return Err(Error::Limit);
            }
            if content_type == "application/json" {
                bytes.extend_from_slice(&chunk);
            } else {
                for message in decoder.push(&chunk)? {
                    if let Some(reply) = self.message(message, id).await? {
                        return Ok(reply);
                    }
                }
            }
        }
        if content_type == "text/event-stream" {
            for message in decoder.finish()? {
                if let Some(reply) = self.message(message, id).await? {
                    return Ok(reply);
                }
            }
        }
        if content_type == "application/json" {
            let message =
                serde_json::from_slice(&bytes).map_err(|_| Error::Protocol("invalid JSON"))?;
            if let Some(reply) = self.message(message, id).await? {
                return Ok(reply);
            }
        }
        Err(Error::Protocol("response ended without a matching result"))
    }
    async fn message(&self, message: Value, id: u64) -> Result<Option<Value>, Error> {
        if message["jsonrpc"] != "2.0" || !message.is_object() {
            return Err(Error::Protocol("invalid envelope"));
        }
        if let Some(method) = message.get("method") {
            let method = method.as_str().ok_or(Error::Protocol("invalid method"))?;
            if let Some(server_id) = message.get("id") {
                if !server_id.is_string() && !server_id.is_number() {
                    return Err(Error::Protocol("invalid request id"));
                }
                let response = if method == "ping" {
                    json!({"jsonrpc":"2.0","id":server_id,"result":{}})
                } else {
                    json!({"jsonrpc":"2.0","id":server_id,"error":{"code":-32601,"message":"Unsupported client capability"}})
                };
                self.send_auxiliary(response, self.deadlines.metadata)
                    .await?;
            } else if method == "notifications/tools/list_changed" {
                self.invalidated.store(true, Ordering::SeqCst);
            }
            return Ok(None);
        }
        if message.get("id") != Some(&json!(id))
            || message.get("result").is_some() == message.get("error").is_some()
        {
            return Err(Error::Protocol("wrong response id or result shape"));
        }
        if let Some(error) = message.get("error") {
            if !error.is_object() || !error.get("message").is_some_and(Value::is_string) {
                return Err(Error::Protocol("invalid RPC error"));
            }
            return Err(Error::Rpc(
                error["code"]
                    .as_i64()
                    .ok_or(Error::Protocol("invalid RPC error"))?,
            ));
        }
        Ok(message.get("result").cloned())
    }
    /// Explicit bounded teardown; does not run from Drop or cancel another call.
    pub async fn close(&self) -> Result<(), Error> {
        let _guard = self.serial.lock().await;
        self.expired.store(true, Ordering::SeqCst);
        if self.session.is_none() {
            return Ok(());
        }
        tokio::time::timeout(self.deadlines.metadata, async {
            let response = self
                .builder(reqwest::Method::DELETE)
                .send()
                .await
                .map_err(|_| Error::Transport)?;
            if response.status().is_success() || response.status().as_u16() == 405 {
                Ok(())
            } else {
                Err(Error::Http(response.status().as_u16()))
            }
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

fn valid_content(block: &Value) -> bool {
    let string = |key: &str| block.get(key).is_some_and(Value::is_string);
    match block.get("type").and_then(Value::as_str) {
        Some("text") => string("text"),
        Some("image" | "audio") => string("data") && string("mimeType"),
        Some("resource_link") => string("uri") && string("name"),
        Some("resource") => block.get("resource").is_some_and(|resource| {
            resource.get("uri").is_some_and(Value::is_string)
                && (resource.get("text").is_some_and(Value::is_string)
                    || resource.get("blob").is_some_and(Value::is_string))
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
