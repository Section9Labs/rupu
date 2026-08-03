//! Network egress observability for rupu.
//!
//! Phase 1 captures rupu's OWN outbound HTTP — provider APIs, SCM
//! connectors, MCP, webhooks, the update checker. It does NOT cover
//! traffic from the agent's `bash` subprocesses; that arrives with the
//! microVM backend (spec §9). Every record carries a [`Fidelity`] so no
//! view ever claims coverage it does not have.

pub mod ctx;
pub mod record;

pub use ctx::{FlowCtx, Origin};
pub use record::{Fidelity, FlowId, FlowRecord, LedgerLine, Outcome};
