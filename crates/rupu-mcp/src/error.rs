//! MCP error type — converts to JSON-RPC error responses.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    /// A JSON-RPC method this server does not implement.
    #[error("unknown method: {0}")]
    UnknownMethod(String),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("transport: {0}")]
    Transport(#[source] anyhow::Error),
}

impl McpError {
    /// JSON-RPC 2.0 error code per MCP convention.
    pub fn code(&self) -> i32 {
        match self {
            Self::UnknownMethod(_) => -32601, // method not found
            Self::InvalidArgs(_) => -32602,   // invalid params
            Self::Transport(_) => -32603,     // internal error
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

/// Why a [`crate::CatalogServer`] can't start.
#[derive(Debug, Error)]
pub enum ServeError {
    /// `--tools` names a tool or namespace the catalog doesn't have.
    #[error(transparent)]
    Grant(#[from] rupu_tools::GrantError),
    /// The grant offers a tool this crate can't build (a service-accounting
    /// bug in `rupu_tools`).
    #[error("`{0}` is granted but has no implementation")]
    NoBody(String),
}
