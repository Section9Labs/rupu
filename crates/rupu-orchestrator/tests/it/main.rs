//! `rupu-orchestrator` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.
//!
//! `tests/terminating.rs` deliberately stays its own binary: it raises
//! `credential_writes::request_termination()`'s process-wide flag, which is
//! never cleared and would make every other run here stop as terminating.

mod action_step;
mod branch_runner;
mod cancel_vs_completion;
mod dispatch_agent;
mod dispatch_agents_parallel;
mod distributed_fanout_e2e;
mod executor_file_tail;
mod executor_in_process;
mod gate_node;
mod gate_sweep_smoke;
mod linear_runner;
mod multi_gate_path_scoped;
mod pause_resume_e2e;
mod placed_step_e2e;
mod remote_findings_profile;
mod run_step_workflow;
mod runner_events;
mod templates;
mod usage_ledger_e2e;
mod workflow_parse;
mod workspace_sync_e2e;
