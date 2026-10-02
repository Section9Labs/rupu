//! `rupu-agent` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.
//!
//! `tests/fd_pressure.rs` deliberately stays its own binary: it lowers the
//! process's `RLIMIT_NOFILE`, which would starve every other test here. So
//! do `tests/terminating{,_compact_messages,_compaction,_overflow}.rs`: they
//! raise `credential_writes::request_termination()`'s process-wide flag,
//! which is never cleared and would make every other run here abort as
//! terminating.

mod coverage_integration;
mod engagement_e2e;
mod findings_full_profile;
mod findings_without_coverage;
mod loader;
mod mcp_attach;
mod permission_resolution;
mod prompt_pty;
mod runner_aborts;
mod runner_basic;
mod runner_model_limits;
mod runner_tools_in_request;
mod runner_usage_hook;
mod spec;
mod spec_auth_field;
mod tool_registry;
