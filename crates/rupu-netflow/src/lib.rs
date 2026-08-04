//! Network egress observability for rupu.
//!
//! Phase 1 captures rupu's OWN outbound HTTP — provider APIs, SCM
//! connectors, MCP, webhooks, the update checker. It does NOT cover
//! traffic from the agent's `bash` subprocesses; that arrives with the
//! microVM backend (spec §9). Every record carries a [`Fidelity`] so no
//! view ever claims coverage it does not have.
//!
//! # Adding an HTTP client
//!
//! Don't. Call [`http::client`] or [`http::client_from`] instead — a raw
//! `reqwest::Client` bypasses capture entirely and `clippy.toml` denies
//! it workspace-wide. If you need custom tuning, pass a tuned
//! `reqwest::ClientBuilder` to [`http::client_from`]; every builder
//! option survives.

pub mod asn;
pub mod ctx;
#[cfg(feature = "http")]
pub mod http;
pub mod ledger;
pub mod record;
pub mod sink;

pub use asn::{AsnInfo, AsnTable};
pub use ctx::{FlowCtx, Origin};
pub use ledger::{NetflowPaths, NetflowWriter, NetflowWriterHandle};
pub use record::{Fidelity, FlowId, FlowRecord, LedgerLine, Outcome};
pub use sink::{FanoutSink, FlowSink, MemorySink, NullSink};
