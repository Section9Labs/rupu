//! JSONL append-only writer for transcript events.

use crate::event::Event;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub struct JsonlWriter {
    inner: BufWriter<File>,
}

impl JsonlWriter {
    /// Create or truncate the file at `path`.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            inner: BufWriter::new(f),
        })
    }

    /// Open `path` for append (create if missing).
    pub fn append(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            inner: BufWriter::new(f),
        })
    }

    pub fn write(&mut self, event: &Event) -> Result<(), WriteError> {
        let line = serde_json::to_string(event)?;
        self.inner.write_all(line.as_bytes())?;
        self.inner.write_all(b"\n")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), WriteError> {
        self.inner.flush()?;
        Ok(())
    }
}

impl Drop for JsonlWriter {
    fn drop(&mut self) {
        let _ = self.inner.flush();
    }
}
