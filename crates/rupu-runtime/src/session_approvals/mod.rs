//! Per-call Ask approvals: private worker channels and durable operator decisions.
//! Never an operator MCP adapter and never a source of additional tool grants.
mod archive;
pub mod authority;
pub mod broker;
#[cfg(test)]
mod broker_tests;
pub mod client;
pub mod protocol;
pub mod store;
pub mod view;
