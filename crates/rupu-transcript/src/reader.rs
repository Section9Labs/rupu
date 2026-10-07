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

/// Separator between the text fragments of one turn in [`final_turn_text`].
/// A blank line, so fragments the model wrote as separate blocks (split by a
/// thinking block) stay separate paragraphs in Markdown.
const FRAGMENT_SEPARATOR: &str = "\n\n";

/// The run's final answer text: every non-empty `AssistantMessage` written
/// after the last `TurnStart`, joined by a blank line. A provider can end a
/// turn with several text blocks (text, thinking, text), and the runner
/// writes one `AssistantMessage` per block, so the last message alone is
/// only the last fragment.
///
/// Falls back to the last non-empty `AssistantMessage` when the transcript
/// has no `TurnStart` (older writers) or the final turn wrote no text.
/// `None` when no assistant text was written at all. Shared by the workflow
/// step output and the dispatch tool's child output.
pub fn final_turn_text(events: impl IntoIterator<Item = Event>) -> Option<String> {
    let mut saw_turn_start = false;
    let mut turn_fragments: Vec<String> = Vec::new();
    // Fragments of earlier turns this answer continues (a truncation
    // continuation's `Recovery { continues_output }`).
    let mut carried: Vec<String> = Vec::new();
    let mut carry_next = false;
    let mut last_non_empty: Option<String> = None;
    // `last_non_empty` as it stood when the current turn began, so a
    // discarded turn can be rolled back out of the fallback too.
    let mut last_before_turn: Option<String> = None;
    // The chain state as it stood when the current turn began. A discarded
    // turn (a refused, retried or empty reply) is transparent to the chain:
    // its `TurnEnd` restores this, so the earlier pieces survive it and the
    // next turn continues them.
    let mut chain_before_turn: (Vec<String>, Vec<String>, bool) = (Vec::new(), Vec::new(), false);
    for event in events {
        match event {
            Event::TurnStart { .. } => {
                saw_turn_start = true;
                chain_before_turn = (carried.clone(), turn_fragments.clone(), carry_next);
                if carry_next {
                    carried.append(&mut turn_fragments);
                } else {
                    carried.clear();
                    turn_fragments.clear();
                }
                carry_next = false;
                last_before_turn = last_non_empty.clone();
            }
            Event::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                if saw_turn_start {
                    turn_fragments.push(content.clone());
                }
                last_non_empty = Some(content);
            }
            Event::TurnEnd {
                discarded: true, ..
            } => {
                (carried, turn_fragments, carry_next) = chain_before_turn.clone();
                last_non_empty = last_before_turn.clone();
            }
            Event::Recovery {
                continues_output: true,
                ..
            } => carry_next = true,
            _ => {}
        }
    }
    carried.append(&mut turn_fragments);
    if carried.is_empty() {
        last_non_empty
    } else {
        Some(carried.join(FRAGMENT_SEPARATOR))
    }
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
    /// The file holds no `run_start` event, so it is not an agent
    /// transcript at all.
    ///
    /// This is a CLASSIFICATION, not corruption. `<global>/transcripts/`
    /// is a shared namespace: alongside agent transcripts it holds
    /// single-record `run:` step transcripts (`{"type":"RunStep",…}`),
    /// action-step transcripts and per-run netflow ledgers — each written
    /// by a different producer, with its own schema, under the same
    /// `run_<ulid>.jsonl` naming. A *damaged* agent transcript still
    /// carries its `run_start`: that event is written before the agent
    /// loop starts, and the tolerated damage modes (truncated tail, bad
    /// line mid-file) all happen after it. So "no run_start anywhere"
    /// means the file was never one of ours, and a directory scanner
    /// should skip it quietly rather than report it as unreadable.
    #[error(
        "not an agent transcript: no run_start event (first record: {})",
        first_tag.as_deref().unwrap_or("<empty file>")
    )]
    NotAnAgentTranscript {
        /// The `type` tag of the file's first record, for diagnostics.
        first_tag: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    /// Codename recorded on `run_start`; `None` on pre-codename transcripts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
}

/// What a transcript's `run_start` event carries — everything knowable
/// about a run without reading past its first record.
///
/// [`RunSummary`] needs the whole file: `status`/`total_tokens` come from
/// `run_complete` at the end, and `first_assistant_text` from somewhere in
/// the middle. A directory listing that sorts by time and then shows the
/// newest N runs does not need any of that for the rows it will discard,
/// so it can sort on [`RunHead`] and summarize only the survivors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunHead {
    pub run_id: String,
    pub workspace_id: String,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub started_at: DateTime<Utc>,
    pub mode: RunMode,
    /// Customer recorded on `run_start`, tri-state (see
    /// [`crate::recorded`]): `None` on transcripts that predate customers,
    /// `Some(None)` for a run recorded with no customer.
    pub customer: crate::recorded::RecordedField,
}

pub struct JsonlReader;

impl JsonlReader {
    /// Build a [`RunSummary`] for the run by reading `run_start` and the
    /// last `run_complete`. If `run_complete` is absent, status is
    /// [`RunStatus::Aborted`]. Truncated/unparseable lines are silently
    /// ignored — they're the signature of an aborted write, not corruption.
    pub fn summary(path: impl AsRef<Path>) -> Result<RunSummary, ReadError> {
        let path = path.as_ref();
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
                Err(ReadError::Parse(_)) | Err(ReadError::NotAnAgentTranscript { .. }) => {}
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
            codename,
            ..
        }) = start
        else {
            // Surface a real IO error if we hit one; otherwise the file genuinely
            // lacks a RunStart event.
            if let Some(e) = last_io_err {
                return Err(ReadError::Io(e));
            }
            return Err(ReadError::NotAnAgentTranscript {
                first_tag: Self::first_record_tag(path),
            });
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
            codename,
        })
    }

    /// Read a transcript's [`RunHead`], stopping at its `run_start`.
    ///
    /// For an agent transcript that is the first line of the file, so this
    /// reads one record where [`summary`](Self::summary) reads the whole
    /// file. It does NOT assume the ordering: if `run_start` is not first,
    /// this keeps scanning for it, and only a file with no `run_start`
    /// anywhere is reported as [`ReadError::NotAnAgentTranscript`] — the
    /// same classification `summary` makes, reached the same way.
    pub fn head(path: impl AsRef<Path>) -> Result<RunHead, ReadError> {
        let path = path.as_ref();
        for ev in Self::iter(path)? {
            match ev {
                Ok(Event::RunStart {
                    run_id,
                    workspace_id,
                    agent,
                    provider,
                    model,
                    started_at,
                    mode,
                    customer,
                    ..
                }) => {
                    return Ok(RunHead {
                        run_id,
                        workspace_id,
                        agent,
                        provider,
                        model,
                        started_at,
                        mode,
                        customer,
                    })
                }
                // A real IO error is fatal; a bad line is not (the tolerated
                // damage modes are documented on this module).
                Err(ReadError::Io(e)) => return Err(ReadError::Io(e)),
                Ok(_) | Err(_) => {}
            }
        }
        Err(ReadError::NotAnAgentTranscript {
            first_tag: Self::first_record_tag(path),
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

    /// The `type` tag of the file's first non-empty record, or `None` when
    /// the file is empty or its first record is not a tagged JSON object.
    ///
    /// Diagnostics only, and only on the
    /// [`ReadError::NotAnAgentTranscript`] path — it names the producer
    /// whose file we just skipped (`RunStep`, `net_flow`, …) so an
    /// operator reading debug logs can tell "a step transcript, correctly
    /// ignored" from "something unexpected in the transcripts directory".
    fn first_record_tag(path: &Path) -> Option<String> {
        let f = File::open(path).ok()?;
        BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .find(|line| !line.trim().is_empty())
            .and_then(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
            .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
    }
}
