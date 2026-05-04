//! Adapter so MCP-backed tools satisfy the rupu_tools::Tool trait.
//!
//! Wraps a single MCP tool's name, description, and input schema, and
//! forwards `invoke(input, ctx)` to the `ToolDispatcher` shared with the
//! in-process MCP server.

use async_trait::async_trait;
use rupu_mcp::{McpError, ToolDispatcher};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::Value;
use std::sync::Arc;

pub struct McpToolAdapter {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
    dispatcher: Arc<ToolDispatcher>,
}

impl McpToolAdapter {
    pub fn new(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        dispatcher: Arc<ToolDispatcher>,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            dispatcher,
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        match self.dispatcher.call(self.name, input).await {
            Ok(text) => Ok(ToolOutput {
                stdout: text,
                error: None,
                duration_ms: 0,
                derived: None,
            }),
            Err(e) => match e {
                McpError::PermissionDenied { .. } => Err(ToolError::PermissionDenied),
                other => Err(ToolError::Execution(other.to_string())),
            },
        }
    }
}
