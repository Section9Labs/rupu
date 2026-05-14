//! Line-stream output module for `rupu`.
//!
//! Provides a streaming vertical timeline printed line-by-line to stdout —
//! the default UI for long-running CLI surfaces. Works in any terminal,
//! any pipe, and any CI runner.

pub mod diag;
pub mod formats;
pub mod jsonl_reader;
pub mod palette;
pub mod printer;
pub mod report;
pub mod rich_payload;
pub mod spinner;
pub mod tables;
pub mod theme;
pub mod workflow_printer;
pub mod yaml_snippet;

pub use jsonl_reader::TranscriptTailer;
pub use printer::LineStreamPrinter;
pub use spinner::SpinnerHandle;
