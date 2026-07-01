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
pub mod http;
pub mod local;
pub mod registry;
pub mod ssh;
pub mod tunnel;
pub mod workspace_stage;
