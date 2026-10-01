//! `rupu-scm` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.

mod classify_scm_error;
mod clone;
mod common;
mod github_clone;
mod github_translation;
mod gitlab_httpmock;
mod gitlab_translation;
mod jira_httpmock;
mod linear_httpmock;
mod live_smoke;
mod netflow_capture;
mod netflow_coarse;
mod registry_discover;
