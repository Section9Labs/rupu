//! Line-stream output module for `rupu`.
//!
//! Provides a streaming vertical timeline printed line-by-line to stdout —
//! the default UI replacing the alt-screen TUI canvas. Works in any
//! terminal, any pipe, and any CI runner.
//!
//! The TUI canvas remains available behind the `--canvas` flag.

pub mod jsonl_reader;
pub mod palette;
pub mod printer;
pub mod spinner;
pub mod tables;
pub mod workflow_printer;

pub use jsonl_reader::TranscriptTailer;
pub use printer::LineStreamPrinter;
pub use spinner::SpinnerHandle;
