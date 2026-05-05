//! Live + replay TUI for rupu runs. Consumes `rupu-transcript` JSONL
//! and `rupu-orchestrator` RunRecords; renders a DAG canvas using
//! ratatui. See `docs/superpowers/specs/2026-05-05-rupu-slice-c-tui-design.md`.

#![doc(html_root_url = "https://docs.rs/rupu-tui")]

pub mod control;
pub mod err;
pub mod source;
pub mod state;
pub mod view;
pub use err::{TuiError, TuiResult};
