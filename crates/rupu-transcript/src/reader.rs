//! JSONL reader; handles missing `run_complete` (aborted runs).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
}

pub struct JsonlReader;
pub struct RunSummary;
