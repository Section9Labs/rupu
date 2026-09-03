//! rupu transcript — JSONL event schema, writer, and reader.

pub mod aggregate;
pub mod event;
pub mod netflow_sink;
pub mod reader;
pub mod writer;

pub use aggregate::{aggregate, TimeWindow, UsageRow};
pub use event::{Event, FileEditKind, RunMode, RunStatus};
pub use netflow_sink::TranscriptSink;
pub use reader::{JsonlReader, ReadError, RunHead, RunSummary};
pub use writer::{JsonlWriter, WriteError};
