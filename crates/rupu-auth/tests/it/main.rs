//! `rupu-auth` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.
//!
//! Every test that sets process env vars (`RUPU_AUTH_FILE`, the
//! `RUPU_OAUTH_*` / `RUPU_DEVICE_*` seams) is `#[serial]`: all of these tests
//! share one process, so an unserialized one would leak into its neighbours.

mod gitlab_refresh;
mod json_file;
mod keychain_resolver;
mod netflow_capture;
mod oauth_callback;
mod oauth_device;
mod resolver_default_pref;
mod resolver_refresh;
