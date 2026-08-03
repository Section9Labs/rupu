//! Attribution context. Bound when a client is CONSTRUCTED, never
//! discovered at request time — `tokio` task-locals and `tracing` spans
//! both lose context across `tokio::spawn`, which would silently
//! mis-attribute flows. See the design spec §4.

use serde::{Deserialize, Serialize};

/// Which subsystem opened the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Origin {
    /// LLM provider, by provider name (`anthropic`, `openai`, …).
    Provider(String),
    /// SCM / issue connector, by platform (`github`, `gitlab`, …).
    Scm(String),
    /// MCP server, by configured server name.
    Mcp(String),
    Webhook,
    Update,
    Cp,
    System,
}

/// Attribution for every flow a client produces.
///
/// `run_id` is `None` for process-global clients (the update checker,
/// CP's host registry). Those flows reach the ledger but no transcript —
/// that is the honest shape of the data, not a gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowCtx {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub origin: Origin,
}

impl FlowCtx {
    /// A context with no run attribution, for process-global clients.
    pub fn system(origin: Origin) -> Self {
        Self {
            run_id: None,
            step_id: None,
            agent: None,
            workspace_id: None,
            origin,
        }
    }
}
