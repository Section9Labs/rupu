//! Attribution context. Bound when a client is CONSTRUCTED, never
//! discovered at request time — `tokio` task-locals and `tracing` spans
//! both lose context across `tokio::spawn`, which would silently
//! mis-attribute flows. See the design spec §4.

use serde::{Deserialize, Serialize};

/// Which subsystem opened the connection.
///
/// This enum is a claim about what the subsystem can capture, so it lists
/// only egress that can actually occur. `Mcp` now denotes the outbound
/// configured-tools client, not the inbound operator server. The operator
/// server's connector tools still tag their own calls `Scm`. `Webhook`
/// remains absent because that server makes no outbound HTTP. `Subprocess`
/// is emitted by the subprocess-capture backend.
///
/// "Can occur" is not "is captured", though: `Update`, `Cp` and `System`
/// CAN be constructed (every one of their production call sites does), but
/// every one of those sites wires its client to `Arc::new(NullSink)`, so
/// none of that traffic ever reaches a ledger, at any scope — see this
/// crate's own top-of-file doc comment ("Login/OAuth, the update checker,
/// and CP's own fleet traffic are deliberately captured nowhere") and
/// `rupu-cp`'s `ScopeDisclosure.tsx` for the full accounting of where each
/// variant's flows actually land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Origin {
    /// LLM provider, by provider name (`anthropic`, `openai`, …).
    Provider(String),
    /// SCM / issue connector, by platform (`github`, `gitlab`, …).
    Scm(String),
    /// Outbound MCP tools client, by trusted configured server ID.
    Mcp(String),
    /// A process the agent started through the `bash` tool, by process
    /// name (`curl`, `git`, `nmap`…), or `"unknown"` when the owning
    /// process could not be resolved. Flows tagged this way are
    /// `Fidelity::Socket` and carry `FlowCtx.tool_call_id`.
    Subprocess(String),
    Update,
    Cp,
    System,
}

/// Attribution for every flow a client produces.
///
/// Provider and SCM callers traditionally attribute by their per-run ledger
/// sink, leaving `run_id` unset. The outbound MCP client additionally accepts
/// an explicit owner context. In both cases the sink must belong to that
/// owner; never reuse another run's client/sink. Long-lived, run-less clients
/// (the update checker,
/// CP's host registry / fleet traffic, auth/oauth token exchange) are
/// wired to `NullSink` and produce no flow at all, not a
/// `run_id: None` one — see `rupu-cp/src/api/netflow.rs`'s module doc
/// and `crates/rupu-cp/web/src/components/netflow/ScopeDisclosure.tsx`
/// for the full accounting of what is and is not captured.
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
    /// The `bash` tool call that caused this flow. Set only on
    /// `Fidelity::Socket` flows; `None` for every HTTP flow and for
    /// `FlowCtx::system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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
            tool_call_id: None,
            origin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_enumerates_only_egress_that_can_occur() {
        // Only real outbound paths have variants; MCP now has a client.
        for json in [
            r#"{"kind":"provider","name":"anthropic"}"#,
            r#"{"kind":"scm","name":"github"}"#,
            r#"{"kind":"mcp","name":"fixture"}"#,
            r#"{"kind":"subprocess","name":"curl"}"#,
            r#"{"kind":"update"}"#,
            r#"{"kind":"cp"}"#,
            r#"{"kind":"system"}"#,
        ] {
            serde_json::from_str::<Origin>(json).expect("known variant must parse");
        }

        // Retired variants must no longer deserialize.
        assert!(serde_json::from_str::<Origin>(r#"{"kind":"webhook"}"#).is_err());
    }
}
