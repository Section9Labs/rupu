//! JSONL append-only writer for transcript events.
//!
//! Two construction modes:
//!
//! - [`JsonlWriter::create`] — create a new file or truncate an existing
//!   one. Use for a fresh transcript.
//! - [`JsonlWriter::append`] — open an existing file (or create it if
//!   missing) for append. Use when continuing an in-progress transcript.
//!
//! Each [`JsonlWriter::write`] call produces exactly one JSONL line.
//! Buffering is via [`std::io::BufWriter`] (default 8 KB); call
//! [`JsonlWriter::flush`] explicitly after critical events (e.g. after
//! `RunComplete`) to guarantee durability. The `Drop` impl best-effort
//! flushes and emits a `tracing::warn!` on failure.
//!
//! This type provides no synchronization. Do not open the same path
//! from two instances concurrently — interleaved writes will corrupt
//! the JSONL.

use crate::event::Event;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use thiserror::Error;
use tracing::warn;

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
    /// Create the file at `path`, truncating if it exists. To fail
    /// instead of overwriting an existing file, callers should check
    /// `path.exists()` first; a `create_new` variant can be added if
    /// needed.
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

    /// Open `path` for append, creating it if missing. Note: a typo in
    /// `path` will silently create a new file at the typo'd location.
    pub fn append(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            inner: BufWriter::new(f),
        })
    }

    /// Serialize and append `event` as a single JSONL line.
    ///
    /// Atomicity: the event is serialized to a `String` *before* any I/O,
    /// so a serialization failure leaves the file unchanged. A successful
    /// call writes exactly one complete line (event + `\n`). The reader
    /// in `rupu-transcript::reader` depends on this line-boundary
    /// invariant for aborted-run detection.
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
        if let Err(e) = self.inner.flush() {
            warn!(error = %e, "JsonlWriter: flush on drop failed; trailing buffered events may be lost");
        }
    }
}
