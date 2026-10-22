//! Services a run's assembly attaches: the message space (spec F1a), and
//! the operator's side of it (spec F1b), and a session's pending
//! compaction.

pub mod board;
pub mod messaging;
pub mod operator;
pub mod session_compaction;
