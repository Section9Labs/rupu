//! `rupu-tools` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.

mod ast_grep;
mod bash;
mod coverage_instrumentation;
mod edit_file;
mod glob;
mod grep;
mod permission;
mod read_file;
mod tool_schemas;
mod write_file;
