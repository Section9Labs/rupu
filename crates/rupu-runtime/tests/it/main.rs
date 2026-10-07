//! `rupu-runtime` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.
//!
//! Every test that sets process env vars (`RUPU_MOCK_PROVIDER_SCRIPT`,
//! `RUPU_AUTH_FILE`, the base-URL seams) is `#[serial]`: all of these tests
//! share one process, so an unserialized one would leak into its neighbours.
//! So is every test that reaches the provider factory (a hop build,
//! `model_limits::refresh`), even one that sets nothing: the factory reads
//! `RUPU_MOCK_PROVIDER_SCRIPT`, and would build a mock for it while a
//! neighbour has the script set.

mod anthropic_prompt_cache;
mod gemini_code_assist_project;
mod hop_builder;
mod model_limits;
mod netflow_capture;
mod oauth_refresh_persists;
mod oauth_refresh_tracking;
mod provider_resolution;
mod sync_provider_build;
