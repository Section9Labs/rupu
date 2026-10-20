//! Host abstraction layer for rupu-cp.
//!
//! [`connector`] defines the [`HostConnector`] trait and associated types;
//! [`local`] provides the in-process implementation (`LocalHostConnector`)
//! that delegates to the per-capability port traits and the `RunStore`.
//!
//! The runtime provides `HttpHostConnector` and `HostRegistry`; later tasks add
//! wiring into `AppState` (Task 5).

pub mod bucket;
pub mod connector;
pub mod dashboard_summary;
pub mod fleet_counts;
pub mod http;
pub mod lazy_tail;
pub mod listing_cache;
pub mod local;
pub mod messages;
pub mod probe_cache;
pub mod registry;
#[cfg(test)]
pub(crate) mod runtime_liveness;
pub mod skill_ship;
pub mod ssh;
pub mod summary_build;
/// The mirror cache layout lives in `rupu_orchestrator` with the usage
/// fold that resolves through it; re-exported at its old path.
pub use rupu_orchestrator::transcript_paths;
pub mod tunnel;
pub mod workspace_stage;
