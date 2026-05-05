//! rupu transcript — JSONL event schema, writer, and reader.

pub mod aggregate;
pub mod event;
pub mod reader;
pub mod writer;

pub use aggregate::{aggregate, TimeWindow, UsageRow};
pub use event::{Event, FileEditKind, RunMode, RunStatus};
pub use reader::{JsonlReader, ReadError, RunSummary};
pub use writer::{JsonlWriter, WriteError};
