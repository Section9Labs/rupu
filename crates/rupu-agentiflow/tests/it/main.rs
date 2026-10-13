//! `rupu-agentiflow` integration tests, linked as ONE test binary. Every
//! top-level `tests/*.rs` file is its own binary that links the whole
//! dependency graph, so each file here is a module instead. Add new tests as a
//! module below, not as a new `tests/*.rs` — `rupu-cli`'s
//! `tests/it/test_layout.rs` enforces this.

mod flow_parity;
mod flow_toolset_parity;
mod message_root;
mod operator_messages;
mod reaper;
mod round_prompt_scope;
