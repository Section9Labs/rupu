//! `rupu-runtime` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.
//!
//! Every test that sets process env vars (`RUPU_MOCK_PROVIDER_SCRIPT`,
//! `RUPU_AUTH_FILE`, the base-URL seams) is `#[serial]`: all of these tests
//! share one process, so an unserialized one would leak into its neighbours.

mod anthropic_prompt_cache;
mod netflow_capture;
mod provider_resolution;
