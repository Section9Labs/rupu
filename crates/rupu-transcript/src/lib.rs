//! rupu transcript — JSONL event schema, writer, and reader.
//!
//! See `docs/transcript-schema.md` and the Slice A spec for the event
//! schema definition.

// `Event` and `RunStatus` are added in Task 4 (TDD); re-exported then.
pub mod event;
pub mod reader;
pub mod writer;

pub use reader::{JsonlReader, ReadError, RunSummary};
pub use writer::{JsonlWriter, WriteError};
