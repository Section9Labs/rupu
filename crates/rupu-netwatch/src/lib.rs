//! Subprocess network capture for rupu.
//!
//! This crate observes the connections made by the processes a `bash` tool
//! call spawns and reports them to the run's flow sink. The attribution
//! state machine, [`tracker::Tracker`], is pure: it is synchronous, does no
//! IO and knows nothing about any operating system. Backends translate
//! their native socket events into [`SocketSnapshot`]s and an opaque
//! [`OwnerId`]; the tracker returns [`Emission`]s for the caller to deliver.

pub mod linux;
pub mod macos;
pub mod tracker;
pub mod types;
pub mod unsupported;

pub use types::*;
