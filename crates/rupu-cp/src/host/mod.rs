//! Host abstraction layer for rupu-cp.
//!
//! [`connector`] defines the [`HostConnector`] trait and associated types;
//! [`local`] provides the in-process implementation (`LocalHostConnector`)
//! that delegates to the per-capability port traits and the `RunStore`.
//!
//! Later tasks will add `HttpHostConnector` (Task 3), a `HostRegistry` (Task 4),
//! and wiring into `AppState` (Task 5).

pub mod connector;
pub mod http;
pub mod local;
