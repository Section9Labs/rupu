//! `rupu-launch` integration tests, linked as ONE test binary (see
//! `rupu-cli`'s `tests/it/test_layout.rs`). Tests that set process env vars
//! (`RUPU_MOCK_PROVIDER_SCRIPT`) are `#[serial]`.

mod continuation;
mod subagent;
mod support;
mod w7;
mod workflow_step;
