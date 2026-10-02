//! Continue an interrupted agent run from its transcript (spec
//! `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md` §1).
//!
//! A transcript is written event by event, so it survives a pause, a killed
//! process or a crash. [`prepare_continuation`] reads one back and decides:
//! the run had finished (its answer is recovered without a model call), it
//! failed (not an interruption — the caller starts fresh), or it can be
//! resumed from the conversation `replay` rebuilds.

use crate::replay::{reconstruct_transcript, ReplayError};
use rupu_providers::types::{ContentBlock, Message, Role};
use rupu_transcript::{Event, JsonlReader, ReadError, RunStatus};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Sent as the continuation's `user_message`; joins the rebuilt
/// conversation's trailing user turn (see `runner::push_user_turn`).
pub const CONTINUATION_NOTE: &str = "This session was interrupted and has been resumed. Work from the step you were in the middle of was lost and may be partially applied — check the current state, then continue the task.";

/// What an interrupted attempt's transcript says to do next.
#[derive(Debug)]
pub enum Continuation {
    /// The run finished; only its record was lost. `output` is its final
    /// assistant text — no model call needed.
    Finished { output: String },
    /// Rebuild and continue: seed a new run with `messages` by reference to
    /// `seed_source` (the interrupted transcript).
    Resume {
        messages: Vec<Message>,
        seed_source: PathBuf,
    },
    /// The run ended in failure — not an interruption. Start fresh.
    Failed { error: Option<String> },
}

#[derive(Debug, Error)]
pub enum ContinuationError {
    #[error("transcript {path} can't be read: {source}")]
    Read {
        path: String,
        #[source]
        source: ReadError,
    },
    #[error("transcript {path} has no agent run in it")]
    NotAnAgentRun { path: String },
    #[error("transcript {path} ends on a tool call with no result")]
    DanglingToolCall { path: String },
    #[error("transcript {path} can't be replayed: {source}")]
    Replay {
        path: String,
        #[source]
        source: ReplayError,
    },
}

fn read_events(transcript: &Path) -> Result<Vec<Event>, ContinuationError> {
    let iter = JsonlReader::iter(transcript).map_err(|source| ContinuationError::Read {
        path: transcript.display().to_string(),
        source,
    })?;
    Ok(iter.filter_map(Result::ok).collect())
}

/// The agent name recorded in the transcript's `RunStart`.
pub fn transcript_agent(transcript: &Path) -> Result<String, ContinuationError> {
    read_events(transcript)?
        .into_iter()
        .find_map(|e| match e {
            Event::RunStart { agent, .. } => Some(agent),
            _ => None,
        })
        .ok_or_else(|| ContinuationError::NotAnAgentRun {
            path: transcript.display().to_string(),
        })
}

/// Decide how to pick up the attempt recorded in `transcript`.
///
/// | ends with                                | outcome    |
/// |------------------------------------------|------------|
/// | `RunComplete { status: ok }`             | `Finished` |
/// | `RunComplete { status: error }`          | `Failed`   |
/// | `RunComplete { status: aborted }`, none  | `Resume` — or `Finished` when the rebuilt conversation already ends in the final answer |
pub fn prepare_continuation(transcript: &Path) -> Result<Continuation, ContinuationError> {
    let path = transcript.display().to_string();
    let events = read_events(transcript)?;
    if !events.iter().any(|e| matches!(e, Event::RunStart { .. })) {
        return Err(ContinuationError::NotAnAgentRun { path });
    }
    let completion = events.iter().rev().find_map(|e| match e {
        Event::RunComplete { status, error, .. } => Some((*status, error.clone())),
        _ => None,
    });
    match completion {
        Some((RunStatus::Ok, _)) => {
            return Ok(Continuation::Finished {
                output: final_assistant_text(&events),
            })
        }
        Some((RunStatus::Error, error)) => return Ok(Continuation::Failed { error }),
        Some((RunStatus::Aborted, _)) | None => {}
    }
    let messages =
        reconstruct_transcript(transcript).map_err(|source| ContinuationError::Replay {
            path: path.clone(),
            source,
        })?;
    match messages.last() {
        None => Err(ContinuationError::NotAnAgentRun { path }),
        Some(last) if last.role == Role::Assistant => {
            if last
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            {
                Err(ContinuationError::DanglingToolCall { path })
            } else {
                // The final answer's turn completed; the process died before
                // `RunComplete` landed. Nothing left to do. The answer is read
                // the same way as after a `RunComplete`, so it can't depend on
                // whether that last flush landed.
                Ok(Continuation::Finished {
                    output: final_assistant_text(&events),
                })
            }
        }
        Some(_) => Ok(Continuation::Resume {
            messages,
            seed_source: transcript.to_path_buf(),
        }),
    }
}

/// The run's answer: the last assistant text event with anything in it.
///
/// The runner emits one `AssistantMessage` per text block, so this is the
/// final text block of the final turn that produced one. Both `Finished`
/// paths use it, so the recovered answer never depends on whether
/// `RunComplete` landed.
fn final_assistant_text(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                Some(content.clone())
            }
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{run_agent, tests::opts_for, MockProvider, ScriptedTurn};
    use rupu_providers::types::StopReason;

    /// A real two-turn run: turn 1 reads `notes.txt`, turn 2 answers.
    async fn finished_transcript(dir: &Path) -> PathBuf {
        std::fs::write(dir.join("notes.txt"), "alpha\nbeta\n").unwrap();
        let transcript = dir.join("first.jsonl");
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_1".into(),
                tool_name: "read_file".into(),
                tool_input: serde_json::json!({ "path": "notes.txt" }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "all done".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]);
        let mut opts = opts_for(Box::new(provider), dir, transcript.clone());
        opts.user_message = "summarise notes.txt".into();
        run_agent(opts).await.unwrap();
        transcript
    }

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn write_lines(path: &Path, lines: &[serde_json::Value]) {
        let body: String = lines.iter().map(|v| format!("{v}\n")).collect();
        std::fs::write(path, body).unwrap();
    }

    fn kind(v: &serde_json::Value) -> &str {
        v["type"].as_str().unwrap_or_default()
    }

    /// Lines up to and including the first `turn_end` (the tool turn) —
    /// what a runner that died during turn 2 leaves behind.
    fn through_first_turn_end(all: &[serde_json::Value]) -> Vec<serde_json::Value> {
        let end = all.iter().position(|v| kind(v) == "turn_end").unwrap();
        all[..=end].to_vec()
    }

    fn assert_resumes_after_tool_turn(c: Continuation, transcript: &Path) {
        match c {
            Continuation::Resume {
                messages,
                seed_source,
            } => {
                assert_eq!(seed_source, transcript);
                let roles: Vec<Role> = messages.iter().map(|m| m.role.clone()).collect();
                assert_eq!(roles, vec![Role::User, Role::Assistant, Role::User]);
                assert!(matches!(
                    &messages[2].content[0],
                    ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call_1"
                ));
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_finished_run_is_recovered_without_a_model_call() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        match prepare_continuation(&t).unwrap() {
            Continuation::Finished { output } => assert_eq!(output, "all done"),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_run_that_died_mid_turn_resumes_from_its_last_complete_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let all = lines(&t);
        let mut cut = through_first_turn_end(&all);
        // A started-but-unfinished second turn: dropped by replay.
        cut.push(serde_json::json!({ "type": "turn_start", "data": { "turn_idx": 1 } }));
        write_lines(&t, &cut);
        assert_resumes_after_tool_turn(prepare_continuation(&t).unwrap(), &t);
    }

    #[tokio::test]
    async fn a_paused_or_signalled_run_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let all = lines(&t);
        let mut complete = all
            .iter()
            .find(|v| kind(v) == "run_complete")
            .unwrap()
            .clone();
        complete["data"]["status"] = "aborted".into();
        complete["data"]["error"] = "terminating (SIGTERM)".into();
        let mut cut = through_first_turn_end(&all);
        cut.push(complete);
        write_lines(&t, &cut);
        assert_resumes_after_tool_turn(prepare_continuation(&t).unwrap(), &t);
    }

    #[tokio::test]
    async fn a_run_interrupted_before_its_first_turn_resumes_from_the_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let all = lines(&t);
        let first_turn = all.iter().position(|v| kind(v) == "turn_start").unwrap();
        write_lines(&t, &all[..first_turn]);
        match prepare_continuation(&t).unwrap() {
            Continuation::Resume { messages, .. } => {
                assert_eq!(
                    serde_json::to_value(&messages).unwrap(),
                    serde_json::to_value(vec![Message::user("summarise notes.txt")]).unwrap()
                );
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_run_that_died_after_its_answer_but_before_run_complete_is_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let all: Vec<_> = lines(&t)
            .into_iter()
            .filter(|v| kind(v) != "run_complete")
            .collect();
        write_lines(&t, &all);
        match prepare_continuation(&t).unwrap() {
            Continuation::Finished { output } => assert_eq!(output, "all done"),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    /// A real run whose final turn answers in several text blocks (the
    /// runner writes one `assistant_message` per block).
    async fn finished_transcript_with_answer_blocks(dir: &Path, blocks: &[&str]) -> PathBuf {
        let transcript = dir.join("blocks.jsonl");
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: blocks
                .iter()
                .map(|t| ContentBlock::Text {
                    text: (*t).to_string(),
                })
                .collect(),
            stop: StopReason::EndTurn,
        }]);
        let mut opts = opts_for(Box::new(provider), dir, transcript.clone());
        opts.user_message = "answer in pieces".into();
        run_agent(opts).await.unwrap();
        transcript
    }

    fn finished_output(t: &Path) -> String {
        match prepare_continuation(t).unwrap() {
            Continuation::Finished { output } => output,
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_recovered_answer_does_not_depend_on_run_complete_landing() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript_with_answer_blocks(tmp.path(), &["a", "b"]).await;
        let with_complete = finished_output(&t);
        let without: Vec<_> = lines(&t)
            .into_iter()
            .filter(|v| kind(v) != "run_complete")
            .collect();
        write_lines(&t, &without);
        let without_complete = finished_output(&t);
        assert_eq!(with_complete, "b");
        assert_eq!(without_complete, with_complete);
    }

    #[tokio::test]
    async fn a_whitespace_only_trailing_chunk_is_not_the_answer_on_either_path() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript_with_answer_blocks(tmp.path(), &["the answer", " \n"]).await;
        assert_eq!(finished_output(&t), "the answer");
        let without: Vec<_> = lines(&t)
            .into_iter()
            .filter(|v| kind(v) != "run_complete")
            .collect();
        write_lines(&t, &without);
        assert_eq!(finished_output(&t), "the answer");
    }

    #[tokio::test]
    async fn a_failed_run_is_not_continued() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let mut all = lines(&t);
        let i = all.iter().position(|v| kind(v) == "run_complete").unwrap();
        all[i]["data"]["status"] = "error".into();
        all[i]["data"]["error"] = "max turns (5) reached".into();
        write_lines(&t, &all);
        match prepare_continuation(&t).unwrap() {
            Continuation::Failed { error } => {
                assert_eq!(error.as_deref(), Some("max turns (5) reached"))
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_tool_call_without_its_result_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let cut: Vec<_> = through_first_turn_end(&lines(&t))
            .into_iter()
            .filter(|v| kind(v) != "tool_result")
            .collect();
        write_lines(&t, &cut);
        assert!(matches!(
            prepare_continuation(&t),
            Err(ContinuationError::DanglingToolCall { .. })
        ));
    }

    #[test]
    fn a_missing_transcript_is_a_read_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            prepare_continuation(&tmp.path().join("nope.jsonl")),
            Err(ContinuationError::Read { .. })
        ));
    }

    #[test]
    fn a_file_with_no_agent_run_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path().join("empty.jsonl");
        std::fs::write(&t, "").unwrap();
        assert!(matches!(
            prepare_continuation(&t),
            Err(ContinuationError::NotAnAgentRun { .. })
        ));
    }

    #[tokio::test]
    async fn transcript_agent_reads_the_run_start() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        assert_eq!(transcript_agent(&t).unwrap(), "test-agent");
    }
}
