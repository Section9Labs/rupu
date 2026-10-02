//! rupu transcript — JSONL event schema, writer, and reader.

pub mod aggregate;
pub mod cursor;
pub mod event;
pub mod netflow_sink;
pub mod reader;
pub mod writer;

pub use aggregate::{aggregate, TimeWindow, UsageRow};
pub use cursor::{transcript_key, DrainStats, JsonlCursor};
pub use event::{Event, FileEditKind, RunMode, RunStatus};
pub use netflow_sink::TranscriptSink;
pub use reader::{final_turn_text, JsonlReader, ReadError, RunHead, RunSummary};
pub use writer::{JsonlWriter, WriteError};
