//! JSONL reader for transcript events.
//!
//! Aborted runs (no `run_complete` event) are surfaced via [`RunSummary`]
//! with [`crate::RunStatus::Aborted`].
//!
//! Tolerated input variations:
//!
//! - **Empty lines** are silently skipped (formatting artifacts, not data).
//! - **Truncated last lines** are silently skipped (signature of an
//!   aborted/crashed write, not corruption).
//! - **Bad JSON lines mid-file** are returned as `Err(ReadError::Parse)`
//!   from [`JsonlReader::iter`]; [`JsonlReader::summary`] silently
//!   ignores them since they cannot be a `RunStart` or `RunComplete`.

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
#[non_exhaustive]
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
    /// First non-empty `AssistantMessage` content, retained verbatim
    /// (no truncation in the summary — callers decide presentation
    /// width). Used by `rupu transcript list` to render a one-line
    /// preview as a title alongside the otherwise opaque `run_id`.
    /// `None` when the run aborted before any assistant output (rare;
    /// most aborted runs still emit at least one chunk before the
    /// abort).
    pub first_assistant_text: Option<String>,
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
        let mut first_assistant: Option<String> = None;

        let mut last_io_err: Option<std::io::Error> = None;
        for ev in Self::iter(path)? {
            match ev {
                Ok(e @ Event::RunStart { .. }) if start.is_none() => start = Some(e),
                Ok(e @ Event::RunComplete { .. }) => complete = Some(e),
                Ok(Event::AssistantMessage { content, .. }) if first_assistant.is_none() => {
                    if !content.trim().is_empty() {
                        first_assistant = Some(content);
                    }
                }
                Ok(_) => {}
                // Track IO errors so we can surface them if no RunStart was found
                // (concatenated/truncated runs are expected; permission denied is not).
                Err(ReadError::Io(e)) => last_io_err = Some(e),
                // Parse errors are silently ignored — truncated tails are normal.
                Err(ReadError::Parse(_)) | Err(ReadError::MissingRunStart) => {}
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
            // Surface a real IO error if we hit one; otherwise the file genuinely
            // lacks a RunStart event.
            if let Some(e) = last_io_err {
                return Err(ReadError::Io(e));
            }
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
            first_assistant_text: first_assistant,
        })
    }

    /// Stream events line-by-line.
    ///
    /// - Empty lines are skipped silently (they're not data).
    /// - Bad JSON lines yield `Err(ReadError::Parse)`; iteration continues
    ///   to the next line. Callers that want to stop at the first parse
    ///   error should call `.take_while(Result::is_ok)`.
    /// - I/O errors during the read yield `Err(ReadError::Io)`; iteration
    ///   continues but most callers should treat this as fatal.
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
