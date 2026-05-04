//! MCP error type — converts to JSON-RPC error responses.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("permission denied for tool {tool}: {reason}")]
    PermissionDenied { tool: String, reason: String },

    #[error("not wired in v0: {0}")]
    NotWiredInV0(String),

    #[error("tool dispatch failed: {0}")]
    Dispatch(#[from] rupu_scm::ScmError),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("transport: {0}")]
    Transport(#[source] anyhow::Error),
}

impl McpError {
    /// JSON-RPC 2.0 error code per MCP convention. -32xxx are reserved
    /// for protocol; we use -32001..-32099 for application errors.
    pub fn code(&self) -> i32 {
        match self {
            Self::UnknownTool(_) => -32601, // method not found
            Self::InvalidArgs(_) => -32602, // invalid params
            Self::PermissionDenied { .. } => -32001,
            Self::NotWiredInV0(_) => -32002,
            Self::Dispatch(_) => -32003,
            Self::Transport(_) => -32603, // internal error
        }
    }

    pub fn to_jsonrpc(&self, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        })
    }
}
