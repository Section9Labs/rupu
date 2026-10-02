//! File-backed fleet comms substrate for agentiflows: a shared board
//! (claims / posts / directives) and per-participant mailboxes. Pure store
//! crate — no agent or provider dependency. Copies the atomicity and TTL-lease
//! patterns from `rupu-workspace`'s autoflow claim store.

mod board;
mod error;
mod types;

pub use board::Board;
pub use error::FleetError;
pub use types::{BoardPost, ClaimGuard, ClaimOutcome, Directive, FleetMessage, PostKind};
