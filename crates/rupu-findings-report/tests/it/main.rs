//! `rupu-findings-report` integration tests, linked as ONE test binary. Every top-level
//! `tests/*.rs` file is its own binary that links the whole dependency
//! graph, so each file here is a module instead. Add new tests as a module
//! below, not as a new `tests/*.rs` — `rupu-cli`'s `tests/it/test_layout.rs`
//! enforces this.

mod blocks;
mod common;
mod exports;
mod html_typst;
mod markdown;
mod pdf;
