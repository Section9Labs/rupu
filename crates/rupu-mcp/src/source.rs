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
    /// `structuredContent` = the value; `content` = one text block with the
    /// same JSON.
    Json(Value),
    /// Plain text content.
    Text(String),
    /// `isError: true` with this text.
    Error(String),
}

/// Per-call context.
#[derive(Debug, Clone, Default)]
pub struct CallCx {
    /// The MCP session the call arrived on, if the transport has sessions.
    pub session: Option<String>,
}

#[async_trait]
pub trait ToolSource: Send + Sync {
    fn tools(&self) -> Vec<ToolDef>;
    /// `None` when `name` is not this source's tool.
    async fn call(&self, name: &str, args: Value, cx: &CallCx) -> Option<ToolResult>;
}
