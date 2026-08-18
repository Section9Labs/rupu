//! Network egress observability for rupu.
//!
//! Phase 1 captures rupu's OWN outbound HTTP — provider APIs and SCM
//! connectors, wired to a per-run sink. Login/OAuth, the update checker,
//! and CP's own fleet traffic are deliberately captured nowhere (wired to
//! [`sink::NullSink`]) — see [`ctx::Origin`]'s doc for the full accounting,
//! including why `Mcp`/`Webhook` variants don't exist at all: `rupu-mcp`
//! dispatches into SCM connectors, which tag their own calls `Scm`, and
//! `rupu-webhook` is an inbound server that makes no outbound HTTP. It does
//! NOT cover traffic from the agent's `bash` subprocesses; that arrives
//! with the microVM backend (spec §9). Every record carries a [`Fidelity`]
//! so no view ever claims coverage it does not have.
//!
//! # Retention
//!
//! A ledger is never deleted automatically — one file per run
//! accumulates in `<netflow_dir>/<run_id>.jsonl` (see [`NetflowPaths`])
//! for as long as an operator leaves it there. `rupu netflow prune
//! --older-than <duration>` (`rupu-cli`'s `cmd::netflow` module) is the
//! retention tool, mirroring `rupu transcript prune`; nothing in this
//! crate or `rupu-cli` schedules it, so an installation that never runs
//! it keeps every ledger forever. A prune sweep never touches a run
//! that might still be Running/Pending, the directory's own
//! self-ignoring `.gitignore`, or the legacy pre-per-run `flows.jsonl`
//! — see that module's doc comment for the full accept/reject and
//! liveness reasoning.
//!
//! # Adding an HTTP client
//!
//! Don't. Call [`http::client_with`] instead — a raw `reqwest::Client`
//! bypasses capture entirely and `clippy.toml` denies it workspace-wide.
//! Pass a tuned `reqwest::ClientBuilder` for custom timeouts/proxies/etc;
//! every builder option survives. There is deliberately no process-global
//! sink — callers supply their run's `Arc<dyn FlowSink>` explicitly.

pub mod asn;
pub mod ctx;
#[cfg(feature = "http")]
pub mod http;
pub mod ledger;
pub mod record;
pub mod sink;

pub use asn::{AsnInfo, AsnTable};
pub use ctx::{FlowCtx, Origin};
pub use ledger::{
    global_netflow_dir, is_per_run_ledger_path, netflow_dir, project_local_netflow_dir,
    NetflowPaths, NetflowWriter, NetflowWriterHandle, LEGACY_LEDGER_FILENAME,
};
pub use record::{Fidelity, FlowId, FlowRecord, LedgerLine, Outcome};
pub use sink::{FanoutSink, FlowSink, MemorySink, NullSink};
