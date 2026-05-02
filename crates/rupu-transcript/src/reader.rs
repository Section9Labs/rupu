//! JSONL reader for transcript events.
//!
//! Aborted runs (no `run_complete` event) are surfaced via [`RunSummary`].
//! Truncated last lines are silently skipped (a partial last line is the
//! signature of an aborted/crashed write, not corruption to surface).

use crate::event::{Event, RunMode, RunStatus};
use chrono::{DateTime, Utc};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("transcript has no run_start event")]
    MissingRunStart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: String,
    pub workspace_id: String,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub started_at: DateTime<Utc>,
    pub mode: RunMode,
    pub status: RunStatus,
    pub total_tokens: u64,
    pub duration_ms: u64,
    pub error: Option<String>,
}

pub struct JsonlReader;

impl JsonlReader {
    /// Build a [`RunSummary`] for the run by reading `run_start` and the
    /// last `run_complete`. If `run_complete` is absent, status is
    /// [`RunStatus::Aborted`]. Truncated/unparseable lines are silently
    /// ignored — they're the signature of an aborted write, not corruption.
    pub fn summary(path: impl AsRef<Path>) -> Result<RunSummary, ReadError> {
        let mut start: Option<Event> = None;
        let mut complete: Option<Event> = None;

        for ev in Self::iter(path)? {
            match ev {
                Ok(e @ Event::RunStart { .. }) => start = Some(e),
                Ok(e @ Event::RunComplete { .. }) => complete = Some(e),
                Ok(_) => {}
                // Bad lines silently ignored (truncated tail of aborted run).
                Err(_) => {}
            }
        }

        let Some(Event::RunStart {
            run_id,
            workspace_id,
            agent,
            provider,
            model,
            started_at,
            mode,
        }) = start
        else {
            return Err(ReadError::MissingRunStart);
        };

        let (status, total_tokens, duration_ms, error) = match complete {
            Some(Event::RunComplete {
                status,
                total_tokens,
                duration_ms,
                error,
                ..
            }) => (status, total_tokens, duration_ms, error),
            _ => (RunStatus::Aborted, 0, 0, None),
        };

        Ok(RunSummary {
            run_id,
            workspace_id,
            agent,
            provider,
            model,
            started_at,
            mode,
            status,
            total_tokens,
            duration_ms,
            error,
        })
    }

    /// Stream events line-by-line. Bad lines yield `Err(ReadError::Parse)`;
    /// the iterator continues to the next line. Empty lines are skipped
    /// silently (they're not errors).
    pub fn iter(
        path: impl AsRef<Path>,
    ) -> Result<impl Iterator<Item = Result<Event, ReadError>>, ReadError> {
        let f = File::open(path)?;
        let reader = BufReader::new(f);
        Ok(reader.lines().filter_map(|line_res| {
            let line = match line_res {
                Ok(l) => l,
                Err(e) => return Some(Err(ReadError::Io(e))),
            };
            if line.trim().is_empty() {
                return None;
            }
            Some(serde_json::from_str(&line).map_err(ReadError::Parse))
        }))
    }
}
