//! `rupu-cp` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.

mod ast;
mod auth;
mod autoflows;
mod bucket_e2e;
mod common;
mod coverage;
mod customer_filter;
mod customers;
mod dashboard;
mod embed;
mod endpoints;
mod federation_e2e;
mod finding_artifacts;
mod graph;
mod host_http;
mod host_launch_control;
mod host_local;
mod host_reads;
mod host_registry;
mod host_run_netflow;
mod hosts_api;
mod legacy_codenames;
mod models_api;
mod netflow_api;
mod netflow_explorer;
mod node_tunnel;
mod projects;
mod run_graph;
mod run_observation;
mod run_streams;
mod runs;
mod server;
mod sessions_host;
mod sessions_live;
mod source;
mod sse;
mod transcript;
mod usage;
mod workers;
