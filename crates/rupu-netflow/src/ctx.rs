//! Attribution context. Bound when a client is CONSTRUCTED, never
//! discovered at request time — `tokio` task-locals and `tracing` spans
//! both lose context across `tokio::spawn`, which would silently
//! mis-attribute flows. See the design spec §4.

use serde::{Deserialize, Serialize};

/// Which subsystem opened the connection.
///
/// This enum is a claim about what the subsystem can capture, so it lists
/// only egress that can actually occur. `Mcp` and `Webhook` were removed
/// deliberately: `rupu-mcp` makes no outbound HTTP (it dispatches into
/// `rupu-scm`'s connectors, which tag their own calls `Scm`), and
/// `rupu-webhook` is an inbound server. A variant nothing can construct
/// is a promise of coverage that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Origin {
    /// LLM provider, by provider name (`anthropic`, `openai`, …).
    Provider(String),
    /// SCM / issue connector, by platform (`github`, `gitlab`, …).
    Scm(String),
    Update,
    Cp,
    System,
}

/// Attribution for every flow a client produces.
///
/// `run_id` is `None` for every production flow today — there is no
/// per-request call site anywhere in the workspace that sets it (the
/// netflow-per-run plan attributes by LEDGER FILE, not by this field: a
/// run's sink is scoped to one `NetflowPaths::for_run` file at
/// construction time, so which run a flow belongs to is answered by
/// which file it landed in, never by reading `ctx.run_id` back out of
/// the record). Long-lived, run-less clients (the update checker,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_enumerates_only_egress_that_can_occur() {
        // The enum is a claim about what this subsystem can capture.
        // `Mcp` and `Webhook` were removed because neither crate makes
        // outbound HTTP: rupu-mcp dispatches into rupu-scm's connectors
        // (already tagged Scm), and rupu-webhook is an inbound server.
        // A variant that can never be constructed is a false claim.
        for json in [
            r#"{"kind":"provider","name":"anthropic"}"#,
            r#"{"kind":"scm","name":"github"}"#,
            r#"{"kind":"update"}"#,
            r#"{"kind":"cp"}"#,
            r#"{"kind":"system"}"#,
        ] {
            serde_json::from_str::<Origin>(json).expect("known variant must parse");
        }

        // Retired variants must no longer deserialize.
        assert!(serde_json::from_str::<Origin>(r#"{"kind":"mcp","name":"x"}"#).is_err());
        assert!(serde_json::from_str::<Origin>(r#"{"kind":"webhook"}"#).is_err());
    }
}
