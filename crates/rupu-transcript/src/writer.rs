//! JSONL append-only writer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub struct JsonlWriter;
