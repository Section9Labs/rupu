//! `rupu-coverage` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.

mod attach_report;
mod cwe_index_mode_end_to_end;
mod determinism;
mod end_to_end;
mod finding_tags;
mod report_schema_lockstep;
mod verify_finding;
