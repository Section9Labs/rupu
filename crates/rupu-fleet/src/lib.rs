//! File-backed message substrate for rupu's message spaces (spec F1a): the
//! canonical message log, the participant registry, a shared board (claims /
//! posts / directives) and per-participant mailboxes. Pure store crate — no
//! agent or provider dependency. Copies the atomicity and TTL-lease patterns
//! from `rupu-workspace`'s autoflow claim store. A run reaches its space
//! through its [`Bus`], the one writer API.

pub mod address;
mod board;
mod bus;
mod error;
mod log;
mod mailbox;
mod participants;
mod types;

pub use address::{Address, AddressError};
pub use board::Board;
pub use bus::{Bus, BusCaps, ClaimTable, OperatorMessage, DEFAULT_LOG_MAX_MB};
pub use error::{BusError, FleetError};
pub use log::{
    Delivery, LogLine, LogMessage, LogPage, MessageKind, OperatorVia, Refs, SendReceipt,
    Undelivered,
};
pub use mailbox::Mailbox;
pub use participants::{
    Member, Participant, ParticipantEvent, Resolution, Roster, Sender, Undeliverable,
};
pub use types::{
    BoardPost, ClaimGuard, ClaimOutcome, Directive, DirectiveEvent, FleetMessage, PostKind,
};

#[cfg(test)]
mod tests;
