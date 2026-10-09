//! `findings.*`: the findings ledger of the run's workspace — one
//! implementation for the agent loop, `action:` steps and `rupu mcp serve`
//! (spec W4 §3.2). Each tool reads where to write from the call's
//! [`ToolContext`] ([`crate::ledger`]).

pub mod query;
pub mod report;
pub mod tag;
pub mod verify;
