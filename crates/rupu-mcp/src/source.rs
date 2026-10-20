//! Tool sources: what [`crate::McpServer`] serves. A source owns a set of
//! tools; the server owns the protocol.

use async_trait::async_trait;
use serde_json::{json, Value};

/// Informational hints about a tool (MCP `annotations`). They never gate a
/// call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolHints {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl ToolHints {
    pub fn to_annotations(&self) -> Value {
        json!({
            "readOnlyHint": self.read_only,
            "destructiveHint": self.destructive,
            "idempotentHint": self.idempotent,
            "openWorldHint": self.open_world,
        })
    }
}

/// One tool a source offers.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub hints: ToolHints,
}

/// What a tool call produced.
#[derive(Debug, Clone)]
pub enum ToolResult {
    /// `content` = one text block with the JSON; `structuredContent` = the
    /// value too when it is an object (the protocol allows nothing else).
    Json(Value),
    /// Plain text content.
    Text(String),
    /// `isError: true` with this text.
    Error(String),
    /// A JSON result plus notes the caller must see: `content` is the JSON
    /// text block then one text block per note; `structuredContent` follows
    /// `Json`'s rule; `_meta` is `meta`.
    Annotated {
        value: Value,
        notes: Vec<String>,
        meta: Value,
    },
    /// A byte body, embedded as a `resource` content block.
    Resource(EmbeddedResource),
}

/// A file-like result: what `ToolResult::Resource` embeds.
#[derive(Debug, Clone)]
pub struct EmbeddedResource {
    /// e.g. `rupu://api/findings/F1/export?format=pdf`.
    pub uri: String,
    pub mime_type: String,
    /// From `Content-Disposition`'s filename; rides in `resource._meta.name`.
    pub name: Option<String>,
    pub bytes: bytes::Bytes,
}

impl EmbeddedResource {
    /// The `resource` content block: [`EmbeddedResource::to_contents`] as a
    /// tool result's content.
    pub fn to_content(&self) -> Value {
        json!({ "type": "resource", "resource": self.to_contents() })
    }

    /// `{uri, mimeType, text | blob}`, the shape `resources/read` answers
    /// with: `text` when the MIME type is textual and the bytes are UTF-8,
    /// else a padded standard base64 `blob`.
    pub fn to_contents(&self) -> Value {
        let mut res = json!({ "uri": self.uri, "mimeType": self.mime_type });
        let text = is_textual(&self.mime_type)
            .then(|| std::str::from_utf8(&self.bytes).ok())
            .flatten();
        match text {
            Some(t) => res["text"] = json!(t),
            None => {
                use base64::Engine as _;
                res["blob"] = json!(base64::engine::general_purpose::STANDARD.encode(&self.bytes));
            }
        }
        if let Some(name) = &self.name {
            res["_meta"] = json!({ "name": name });
        }
        res
    }
}

fn is_textual(mime: &str) -> bool {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    essence.starts_with("text/")
        || essence == "application/json"
        || essence.ends_with("+json")
        || essence.ends_with("+xml")
        || essence == "application/yaml"
        || essence == "application/toml"
}

/// A call's progress channel: present only when the request carried
/// `params._meta.progressToken` and the transport can deliver notifications.
#[derive(Debug, Clone)]
pub struct Progress {
    token: Value,
    tx: tokio::sync::mpsc::UnboundedSender<Value>,
}

impl Progress {
    pub(crate) fn new(token: Value, tx: tokio::sync::mpsc::UnboundedSender<Value>) -> Self {
        Self { token, tx }
    }

    /// Sends `notifications/progress {progressToken, progress, total?,
    /// message?}`; a closed channel is ignored.
    pub fn report(&self, progress: f64, total: Option<f64>, message: Option<&str>) {
        let mut params = json!({ "progressToken": self.token, "progress": progress });
        if let Some(t) = total {
            params["total"] = json!(t);
        }
        if let Some(m) = message {
            params["message"] = json!(m);
        }
        let _ = self.tx.send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": params,
        }));
    }
}

/// Per-call context.
#[derive(Debug, Clone, Default)]
pub struct CallCx {
    /// Where the call reports progress, when the client asked for it.
    pub progress: Option<Progress>,
    /// The MCP session the call arrived on, if the transport has sessions.
    pub session: Option<String>,
    /// The `Host` header of the request the call arrived on.
    pub host: Option<String>,
}

#[async_trait]
pub trait ToolSource: Send + Sync {
    fn tools(&self) -> Vec<ToolDef>;
    /// `None` when `name` is not this source's tool.
    async fn call(&self, name: &str, args: Value, cx: &CallCx) -> Option<ToolResult>;
}

/// One resource a [`ResourceSource`] lists.
#[derive(Debug, Clone)]
pub struct ResourceDef {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// A parameterized URI (RFC 6570) a [`ResourceSource`] answers, for
/// `resources/templates/list`.
#[derive(Debug, Clone)]
pub struct ResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub description: String,
    /// Every resource the template expands to has this type, when they share
    /// one.
    pub mime_type: Option<String>,
}

/// Read-only resources (`resources/list`, `resources/read`).
#[async_trait]
pub trait ResourceSource: Send + Sync {
    fn resources(&self) -> Vec<ResourceDef>;
    fn templates(&self) -> Vec<ResourceTemplate>;
    /// `None` when `uri` is not this source's resource.
    async fn read(&self, uri: &str) -> Option<EmbeddedResource>;
}

/// One argument a prompt takes. Arguments are strings (the protocol's).
#[derive(Debug, Clone)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

/// One prompt a [`PromptSource`] lists.
#[derive(Debug, Clone)]
pub struct PromptDef {
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
}

/// A prompt rendered for its arguments: `prompts/get` answers with
/// `description` and one user message holding `text`.
#[derive(Debug, Clone)]
pub struct PromptReply {
    pub description: String,
    pub text: String,
}

/// Prompts (`prompts/list`, `prompts/get`).
#[async_trait]
pub trait PromptSource: Send + Sync {
    fn prompts(&self) -> Vec<PromptDef>;
    /// `None` when `name` is not this source's prompt. The server has
    /// checked `args` first: every required argument is present, and every
    /// value is a string.
    async fn get(
        &self,
        name: &str,
        args: &std::collections::BTreeMap<String, String>,
    ) -> Option<PromptReply>;
}
