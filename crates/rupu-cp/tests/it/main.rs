//! `rupu-cp` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.

mod agentiflows;
mod ast;
mod auth;
mod autoflows;
mod bucket_e2e;
mod common;
mod config_api;
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
mod launch_preview;
mod mcp;
mod mcp_ops_agentiflows;
mod mcp_ops_definitions;
mod mcp_ops_findings;
mod mcp_ops_fleet;
mod mcp_ops_projects;
mod mcp_ops_observe;
mod mcp_ops_runs;
mod mcp_ops_sessions;
mod mcp_resources;
mod mcp_routes;
mod mcp_wait;
mod legacy_codenames;
mod list_query;
mod messages;
mod models_api;
mod netflow_api;
mod netflow_explorer;
mod node_tunnel;
mod path_safety;
mod projects;
mod run_graph;
mod run_observation;
mod run_streams;
mod runs;
mod server;
mod sessions_host;
mod sessions_live;
mod skill_ship;
mod source;
mod sse;
mod supplied_identity;
mod transcript;
mod usage;
mod workers;
