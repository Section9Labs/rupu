//! `rupu-cli` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.
//!
//! Nothing here may mutate process-global state — env vars or the working
//! directory. Every test shares this process, and every `rupu` child spawned
//! meanwhile inherits both. Tests that need to do that belong in
//! `tests/serial/`; `test_layout.rs` rejects `set_var` / `remove_var` /
//! `set_current_dir` here.

mod auth_login_gitlab;
mod auth_login_modes;
mod auth_status_table;
mod cli_auth_backend_retired;
mod cli_cleanup;
mod cli_generate;
mod cli_man;
mod cli_scm_bind_walkthrough;
mod cli_session;
mod cli_ui;
mod cli_update_platform;
mod cli_usage;
mod cli_workflow_summary;
mod findings_export;
mod findings_schema;
mod init_create_skeleton;
mod init_force;
mod init_git_flag;
mod init_gitignore;
mod init_manifest_in_sync;
mod init_merge_behavior;
mod init_smoke;
mod init_with_samples;
mod mcp_serve_stdio_smoke;
mod models_subcommand;
mod multi_provider_e2e;
mod no_stream_flag;
mod output_line_stream;
mod run_header;
mod run_model_limits;
mod run_target_parse;
mod sigterm_exit;
mod test_layout;
