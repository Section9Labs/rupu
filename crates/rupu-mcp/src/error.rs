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

    /// A non-SCM tool failed while executing. `Dispatch` carries an
    /// `ScmError` and so cannot represent a tool that never touches an SCM
    /// connector — `findings.record` writes to the local ledger.
    #[error("tool failed: {0}")]
    Tool(String),

    /// `Registry::repo_for`/`issues_for`/`github_extras_for`/
    /// `gitlab_extras_for` failed to pick an account — ambiguous
    /// (`NoRuleMatched`), unconfigured (`NoAccounts`), or a bad
    /// explicit `account` argument (`UnknownAccount`). Distinct from
    /// `Dispatch`: the connector was never reached, so this is a
    /// request-shape problem, closer to `InvalidArgs` than to an SCM
    /// API failure.
    #[error("account resolution failed: {0}")]
    Account(#[from] rupu_scm::AccountError),

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
            Self::Account(_) => -32004,
            Self::Tool(_) => -32005,
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
