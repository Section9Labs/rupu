//! rupu transcript — JSONL event schema, writer, and reader.
//!
//! See `docs/transcript-schema.md` and the Slice A spec for the event
//! schema definition.

pub mod event;
pub mod reader;
pub mod writer;

pub use reader::{JsonlReader, ReadError, RunSummary};
pub use writer::{JsonlWriter, WriteError};
