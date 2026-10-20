//! Continue an interrupted agent run from its transcript (spec
//! `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md` §1).
//!
//! A transcript is written event by event, so it survives a pause, a killed
//! process or a crash. [`prepare_continuation`] reads one back and decides:
//! the run had finished (its answer is recovered without a model call), it
//! failed (not an interruption — the caller starts fresh), or it can be
//! resumed from the conversation `replay` rebuilds.
//!
//! An operator who gives the continuation a prompt
//! ([`prepare_prompted_continuation`] / [`apply_prompted_continuation`],
//! `rupu run --continue <id> <prompt>`) continues ANY ended run: a finished
//! one gets the prompt as a new user turn (a real model call), a failed one
//! continues on the same model, and an interrupted one gets the
//! continuation note followed by the prompt.

use crate::replay::{reconstruct_transcript, ReplayError};
use rupu_providers::types::{ContentBlock, Message, Role};
use rupu_transcript::{Event, JsonlReader, OutcomeRecord, ReadError, RunStatus};
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
        /// The outcome the run failed on, when it is a failed run resumed by
        /// [`prepare_recovery_continuation`]; `None` for an interruption.
        stopped_on: Option<OutcomeRecord>,
        /// The skills the conversation had loaded ([`crate::replay::loaded_skills`]).
        loaded_skills: Vec<crate::LoadedSkill>,
    },
    /// The run ended in failure — not an interruption. Start fresh.
    /// `seeded_from` is the transcript it was seeded from, when it was itself
    /// a continuation (that run may still be continued).
    Failed {
        error: Option<String>,
        seeded_from: Option<PathBuf>,
    },
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

/// The `rupu run` arguments `transcript`'s run recorded (`run_start`'s
/// `launch_args`), which `rupu run --continue` restores. Empty for a run
/// `rupu run` did not launch and for a transcript written before the field.
pub fn launch_args(transcript: &Path) -> Result<Vec<String>, ContinuationError> {
    Ok(read_events(transcript)?
        .into_iter()
        .find_map(|e| match e {
            Event::RunStart { launch_args, .. } => Some(launch_args),
            _ => None,
        })
        .unwrap_or_default())
}

/// The skills `transcript`'s run was given by its launch, as the launch
/// wrote them `(skills, preload_skills)`: `run_start`'s `launch_skills` /
/// `launch_preload_skills`, so `*` stays `*` and a launch preload of a skill
/// the agent also names is kept. A transcript written before those fields
/// existed falls back to its `skill_index` rows with reason `launch` (`step`
/// before the rename), layer-qualified. A new transcript with no launch
/// skills has no such rows either, so the fallback gives it none. A
/// continued run is given them again unless its own launch names skills.
pub fn launch_skills(transcript: &Path) -> Result<(Vec<String>, Vec<String>), ContinuationError> {
    let events = read_events(transcript)?;
    let recorded = events.iter().find_map(|e| match e {
        Event::RunStart {
            launch_skills,
            launch_preload_skills,
            ..
        } => Some((launch_skills.clone(), launch_preload_skills.clone())),
        _ => None,
    });
    if let Some(recorded) = recorded.filter(|(s, p)| !s.is_empty() || !p.is_empty()) {
        return Ok(recorded);
    }
    let mut skills = Vec::new();
    let mut preload = Vec::new();
    for event in events {
        let Event::SkillIndex { skills: rows } = event else {
            continue;
        };
        for row in rows {
            if row.reason == "launch" || row.reason == "step" {
                let entry = format!("{}:{}", row.layer, row.name);
                if row.preload {
                    preload.push(entry);
                } else {
                    skills.push(entry);
                }
            }
        }
    }
    Ok((skills, preload))
}

/// Decide how to pick up the attempt recorded in `transcript`.
///
/// | ends with                                | outcome    |
/// |------------------------------------------|------------|
/// | `RunComplete { status: ok }`             | `Finished` |
/// | `RunComplete { status: error }`          | `Failed`   |
/// | `RunComplete { status: aborted }`, none  | `Resume` — or `Finished` when the rebuilt conversation already ends in the final answer |
pub fn prepare_continuation(transcript: &Path) -> Result<Continuation, ContinuationError> {
    prepare(transcript, false)
}

/// Like [`prepare_continuation`], but a run that ended in error WITH a
/// `RunComplete.outcome` is `Resume` (its conversation is intact; the failure
/// was provider-side), carrying that outcome as `stopped_on`. Every other
/// classification is unchanged. `rupu run --continue --model/--provider`
/// uses it: a different model may answer what this one would not.
///
/// A failed run is never read as finished, even when its rebuilt
/// conversation ends in an assistant reply: it did not finish.
pub fn prepare_recovery_continuation(transcript: &Path) -> Result<Continuation, ContinuationError> {
    prepare(transcript, true)
}

fn prepare(transcript: &Path, recovery: bool) -> Result<Continuation, ContinuationError> {
    let path = transcript.display().to_string();
    let events = read_events(transcript)?;
    if !events.iter().any(|e| matches!(e, Event::RunStart { .. })) {
        return Err(ContinuationError::NotAnAgentRun { path });
    }
    let completion = events.iter().rev().find_map(|e| match e {
        Event::RunComplete {
            status,
            error,
            outcome,
            ..
        } => Some((*status, error.clone(), outcome.clone())),
        _ => None,
    });
    let stopped_on = match completion {
        Some((RunStatus::Ok, _, _)) => {
            return Ok(Continuation::Finished {
                output: final_assistant_text(&events),
            })
        }
        Some((RunStatus::Error, _, Some(outcome))) if recovery => Some(outcome),
        Some((RunStatus::Error, error, _)) => {
            return Ok(Continuation::Failed {
                error,
                seeded_from: seeded_from(&events),
            })
        }
        Some((RunStatus::Aborted, _, _)) | None => None,
    };
    let messages =
        reconstruct_transcript(transcript).map_err(|source| ContinuationError::Replay {
            path: path.clone(),
            source,
        })?;
    match messages.last() {
        None => Err(ContinuationError::NotAnAgentRun { path }),
        Some(last)
            if last.role == Role::Assistant
                && last
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. })) =>
        {
            Err(ContinuationError::DanglingToolCall { path })
        }
        Some(last) if last.role == Role::Assistant && stopped_on.is_none() => {
            // The final answer's turn completed; the process died before
            // `RunComplete` landed. Nothing left to do. The answer is read
            // the same way as after a `RunComplete`, so it can't depend on
            // whether that last flush landed.
            Ok(Continuation::Finished {
                output: final_assistant_text(&events),
            })
        }
        Some(_) => Ok(Continuation::Resume {
            messages,
            seed_source: transcript.to_path_buf(),
            stopped_on,
            loaded_skills: crate::replay::loaded_skills(&events),
        }),
    }
}

/// How the run a prompted continuation picks up had ended.
#[derive(Debug, Clone, PartialEq)]
pub enum PriorEnd {
    /// Paused, signalled or crashed before it recorded an end.
    Interrupted,
    /// It finished: `RunComplete { status: ok }`, or its final answer's turn
    /// completed before the process died.
    Finished,
    /// It ended in failure. `outcome` is the provider-side outcome it failed
    /// on (a refusal, say), when it names one.
    Failed {
        error: Option<String>,
        outcome: Option<OutcomeRecord>,
    },
}

/// What [`prepare_prompted_continuation`] rebuilt: the conversation, by
/// reference to `seed_source`, how its run ended and the skills it had
/// loaded.
#[derive(Debug)]
pub struct PromptedContinuation {
    pub messages: Vec<Message>,
    pub seed_source: PathBuf,
    pub ended: PriorEnd,
    pub loaded_skills: Vec<crate::LoadedSkill>,
}

/// Rebuild `transcript`'s conversation for a continuation the operator gives
/// a prompt (`rupu run --continue <id> <prompt>`). A prompt is new input, so
/// every ended run can take one: unlike [`prepare_continuation`], a finished
/// run is rebuilt rather than answered from its record, and a failed run is
/// rebuilt whether or not it names an outcome. The conversation must still
/// replay and must not end on a tool call with no result.
pub fn prepare_prompted_continuation(
    transcript: &Path,
) -> Result<PromptedContinuation, ContinuationError> {
    let path = transcript.display().to_string();
    let events = read_events(transcript)?;
    if !events.iter().any(|e| matches!(e, Event::RunStart { .. })) {
        return Err(ContinuationError::NotAnAgentRun { path });
    }
    let ended = match events.iter().rev().find_map(|e| match e {
        Event::RunComplete {
            status,
            error,
            outcome,
            ..
        } => Some((*status, error.clone(), outcome.clone())),
        _ => None,
    }) {
        Some((RunStatus::Ok, _, _)) => PriorEnd::Finished,
        Some((RunStatus::Error, error, outcome)) => PriorEnd::Failed { error, outcome },
        Some((RunStatus::Aborted, _, _)) | None => PriorEnd::Interrupted,
    };
    let messages =
        reconstruct_transcript(transcript).map_err(|source| ContinuationError::Replay {
            path: path.clone(),
            source,
        })?;
    let ended = match messages.last() {
        None => return Err(ContinuationError::NotAnAgentRun { path }),
        Some(last)
            if last.role == Role::Assistant
                && last
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. })) =>
        {
            return Err(ContinuationError::DanglingToolCall { path })
        }
        // The final answer's turn completed before `RunComplete` landed: it
        // finished, as `prepare_continuation` reads it.
        Some(last) if last.role == Role::Assistant && ended == PriorEnd::Interrupted => {
            PriorEnd::Finished
        }
        Some(_) => ended,
    };
    Ok(PromptedContinuation {
        messages,
        seed_source: transcript.to_path_buf(),
        ended,
        loaded_skills: crate::replay::loaded_skills(&events),
    })
}

/// Seed `turn` — whose `message` is the operator's prompt, its invoked
/// skills' bodies included (their `operator_loads` stay with it) — with a
/// [`PromptedContinuation`]. The prompt moves to `continue_prompt` and
/// `message` becomes the note that goes ahead of it in the same user turn:
///
/// | the run                          | note                                  |
/// |----------------------------------|---------------------------------------|
/// | interrupted                      | [`CONTINUATION_NOTE`] (none when its trailing user turn already holds it) |
/// | finished                         | none: the prompt is a new user turn    |
/// | failed on an outcome             | [`crate::recovery::recovery_retry_note`] on `provider`/`model` |
/// | failed with no outcome           | none                                   |
///
/// The runner writes the note and the prompt as two `user_message` events
/// joined by [`crate::runner::push_user_turn`], the rule replay applies, so
/// a continuation of this run rebuilds them and never sends the prompt
/// again.
pub fn apply_prompted_continuation(
    turn: &mut crate::runner::UserTurn,
    prepared: PromptedContinuation,
    provider: &str,
    model: &str,
) {
    let note = match &prepared.ended {
        PriorEnd::Interrupted if ends_with_note(&prepared.messages) => String::new(),
        PriorEnd::Interrupted => CONTINUATION_NOTE.to_string(),
        PriorEnd::Finished | PriorEnd::Failed { outcome: None, .. } => String::new(),
        PriorEnd::Failed {
            outcome: Some(outcome),
            ..
        } => crate::recovery::recovery_retry_note(&outcome.title, provider, model),
    };
    let prompt = std::mem::replace(&mut turn.message, note);
    turn.continue_prompt = Some(prompt);
    turn.initial_messages = prepared.messages;
    turn.seed_source = Some(prepared.seed_source);
    turn.loaded_skills = prepared.loaded_skills;
}

/// The transcript this run was seeded from by reference, if any.
fn seeded_from(events: &[Event]) -> Option<PathBuf> {
    events.iter().find_map(|e| match e {
        Event::Seed {
            source_transcript: Some(path),
            ..
        } => Some(PathBuf::from(path)),
        _ => None,
    })
}

/// The run's answer: the final turn's text ([`rupu_transcript::final_turn_text`]
/// — every non-empty `AssistantMessage` after the last `TurnStart`, joined by
/// a blank line; the last non-empty one for a transcript without `TurnStart`).
///
/// The runner emits one `AssistantMessage` per text block, so a final turn
/// text → thinking → text has two fragments, and both are the answer — the
/// same rule a workflow step's output and the dispatch tool use. Both
/// `Finished` paths use it, so the recovered answer never depends on whether
/// `RunComplete` landed.
fn final_assistant_text(events: &[Event]) -> String {
    rupu_transcript::final_turn_text(events.iter().cloned()).unwrap_or_default()
}

/// Where a continuation's seed goes: a run's [`UserTurn`] (or, until W3b
/// deletes it, the legacy flat options a workflow step still builds).
///
/// [`UserTurn`]: crate::runner::UserTurn
pub trait ContinuationTarget {
    /// Seed with `messages` by reference to `seed_source`, carrying the
    /// skills the conversation had loaded, and send `note`.
    fn seed_continuation(
        &mut self,
        messages: Vec<Message>,
        seed_source: PathBuf,
        loaded_skills: Vec<crate::LoadedSkill>,
        note: String,
    );
}

impl ContinuationTarget for crate::runner::UserTurn {
    fn seed_continuation(
        &mut self,
        messages: Vec<Message>,
        seed_source: PathBuf,
        loaded_skills: Vec<crate::LoadedSkill>,
        note: String,
    ) {
        self.message = note;
        self.continue_prompt = None;
        self.operator_loads.clear();
        self.initial_messages = messages;
        self.seed_source = Some(seed_source);
        self.loaded_skills = loaded_skills;
    }
}

/// Point a freshly built agent run at the interrupted one: seed it with the
/// rebuilt conversation *by reference* to `seed_source` and hand it the
/// continuation note (which joins the conversation's trailing user turn).
/// Everything else about the run — agent, provider, model, tools, mode — is
/// the caller's fresh configuration.
///
/// If the conversation already ends with the note — the run being continued
/// was itself a continuation that died before its first turn finished — the
/// run is seed-only (empty message): the model has been told once, and
/// repeating it would stack a second identical note into the turn.
pub fn apply_continuation(
    target: &mut impl ContinuationTarget,
    messages: Vec<Message>,
    seed_source: PathBuf,
    loaded_skills: Vec<crate::LoadedSkill>,
) {
    let note = if ends_with_note(&messages) {
        String::new()
    } else {
        CONTINUATION_NOTE.to_string()
    };
    apply_continuation_with(target, messages, seed_source, loaded_skills, note);
}

/// [`apply_continuation`] with the caller's `note` as the user message —
/// a failed run continued on another model gets
/// [`crate::recovery::recovery_retry_note`] instead of [`CONTINUATION_NOTE`].
pub fn apply_continuation_with(
    target: &mut impl ContinuationTarget,
    messages: Vec<Message>,
    seed_source: PathBuf,
    loaded_skills: Vec<crate::LoadedSkill>,
    note: String,
) {
    target.seed_continuation(messages, seed_source, loaded_skills, note);
}

/// Does `messages` already end in a user turn carrying [`CONTINUATION_NOTE`]?
/// True of the rebuilt conversation of a continuation that died before its
/// first turn finished: its note was recorded, so it replays into the history
/// — followed by the operator's prompt, when the continuation was given one.
fn ends_with_note(messages: &[Message]) -> bool {
    messages.last().is_some_and(|m| {
        m.role == Role::User
            && m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text == CONTINUATION_NOTE))
    })
}

/// The transcript `turn` continues, when it describes a continuation: a run
/// seeded by reference whose conversation ends with [`CONTINUATION_NOTE`]
/// (sent as its message, or already in the seed), that is sent a
/// recovery retry note (a failed run continued on another model), or that
/// carries an operator's continuation prompt ([`apply_prompted_continuation`]).
/// `run_agent` records it on the run's coverage manifest.
pub(crate) fn continued_from(turn: &crate::runner::UserTurn) -> Option<&Path> {
    let source = turn.seed_source.as_deref()?;
    let carries_note = turn.message == CONTINUATION_NOTE
        || crate::recovery::is_recovery_retry_note(&turn.message)
        || turn.continue_prompt.is_some()
        || (turn.message.is_empty() && ends_with_note(&turn.initial_messages));
    carries_note.then_some(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{run_agent, tests::opts_for, MockProvider, ScriptedTurn};
    use rupu_providers::types::StopReason;

    fn stride_concerns() -> crate::RunConcerns {
        crate::RunConcerns {
            block: rupu_coverage::ConcernsBlock {
                entries: vec![rupu_coverage::ConcernsEntry::Include(
                    rupu_coverage::IncludeDirective {
                        include: "stride".to_string(),
                        overrides: vec![],
                        mode: rupu_coverage::CatalogMode::Auto,
                        filter: None,
                    },
                )],
            },
            full_mode_max_concerns: rupu_coverage::DEFAULT_FULL_MODE_THRESHOLD,
        }
    }

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
        opts.prompt.message = "summarise notes.txt".into();
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
                ..
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
        opts.prompt.message = "answer in pieces".into();
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
        assert_eq!(with_complete, "a\n\nb");
        assert_eq!(without_complete, with_complete);
    }

    /// A finished run whose final turn is text → thinking → text reports
    /// both fragments, the same rule a workflow step's output uses; the
    /// earlier tool turn's text is not part of the answer.
    #[tokio::test]
    async fn a_two_fragment_final_turn_is_the_whole_answer() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "alpha\n").unwrap();
        let transcript = tmp.path().join("fragments.jsonl");
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: Some("reading first".into()),
                tool_id: "call_1".into(),
                tool_name: "read_file".into(),
                tool_input: serde_json::json!({ "path": "notes.txt" }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantBlocks {
                content: vec![
                    ContentBlock::Text {
                        text: "Summary:".into(),
                    },
                    ContentBlock::Reasoning {
                        text: Some("check the notes again".into()),
                        provider: "anthropic".into(),
                        model: "mock-1".into(),
                        raw: serde_json::json!({}),
                    },
                    ContentBlock::Text {
                        text: "alpha only".into(),
                    },
                ],
                stop: StopReason::EndTurn,
            },
        ]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), transcript.clone());
        opts.prompt.message = "summarise notes.txt".into();
        run_agent(opts).await.unwrap();
        assert_eq!(finished_output(&transcript), "Summary:\n\nalpha only");
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
            Continuation::Failed { error, seeded_from } => {
                assert_eq!(error.as_deref(), Some("max turns (5) reached"));
                assert_eq!(seeded_from, None, "an ordinary run was seeded from nothing");
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

    fn skills_transcript(dir: &Path, launch: serde_json::Value) -> PathBuf {
        let t = dir.join("skills.jsonl");
        let mut start = serde_json::json!({
            "run_id": "run_1", "workspace_id": "ws", "agent": "a", "provider": "anthropic",
            "model": "m", "started_at": "2026-10-12T00:00:00Z", "mode": "readonly",
        });
        start
            .as_object_mut()
            .unwrap()
            .extend(launch.as_object().unwrap().clone());
        let row = |name: &str, preload: bool, reason: &str| {
            serde_json::json!({
                "name": name, "layer": "global", "stamp": "s", "preload": preload,
                "indexed": !preload, "reason": reason,
            })
        };
        write_lines(
            &t,
            &[
                serde_json::json!({ "type": "run_start", "data": start }),
                serde_json::json!({ "type": "skill_index", "data": { "skills": [
                    row("pdf", true, "agent"), row("x", false, "step"), row("y", true, "launch"),
                ] } }),
            ],
        );
        t
    }

    #[test]
    fn launch_skills_are_run_starts_entries_as_written() {
        let tmp = tempfile::tempdir().unwrap();
        let t = skills_transcript(
            tmp.path(),
            serde_json::json!({ "launch_skills": ["*"], "launch_preload_skills": ["pdf"] }),
        );
        assert_eq!(
            launch_skills(&t).unwrap(),
            (vec!["*".to_string()], vec!["pdf".to_string()])
        );
    }

    #[test]
    fn launch_skills_of_an_older_transcript_come_from_its_launch_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let t = skills_transcript(tmp.path(), serde_json::json!({}));
        assert_eq!(
            launch_skills(&t).unwrap(),
            (vec!["global:x".to_string()], vec!["global:y".to_string()])
        );
    }

    #[test]
    fn launch_args_are_run_starts_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let t = skills_transcript(
            tmp.path(),
            serde_json::json!({ "launch_args": ["a", "--mode=readonly", "--no-stream"] }),
        );
        assert_eq!(
            launch_args(&t).unwrap(),
            vec!["a", "--mode=readonly", "--no-stream"]
        );
    }

    #[test]
    fn launch_args_of_an_older_transcript_are_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let t = skills_transcript(tmp.path(), serde_json::json!({}));
        assert!(launch_args(&t).unwrap().is_empty());
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

    /// The whole primitive: a run dies during turn 2, a fresh run continues
    /// it. The provider sees the rebuilt conversation with the note merged
    /// into the trailing user turn; the new transcript seeds BY REFERENCE to
    /// the old one; replay rebuilds the full conversation across both files;
    /// and the continued run now reads as finished.
    #[tokio::test]
    async fn a_continued_run_picks_up_where_the_interrupted_one_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let first = finished_transcript(tmp.path()).await;
        write_lines(&first, &through_first_turn_end(&lines(&first)));

        let Continuation::Resume {
            messages,
            seed_source,
            ..
        } = prepare_continuation(&first).unwrap()
        else {
            panic!("expected Resume");
        };
        let rebuilt = messages.clone();

        let second = tmp.path().join("second.jsonl");
        let provider =
            crate::runner::CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
                text: "finished after resume".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
        let captured = provider.captured.clone();
        let mut opts = opts_for(Box::new(provider), tmp.path(), second.clone());
        opts.prompt.message = "this must be replaced".into();
        apply_continuation(&mut opts.prompt, messages, seed_source, Vec::new());
        assert_eq!(opts.prompt.message, CONTINUATION_NOTE);
        run_agent(opts).await.unwrap();

        let mut expected = rebuilt;
        expected[2].content.push(ContentBlock::Text {
            text: CONTINUATION_NOTE.into(),
        });
        let sent = captured.lock().unwrap()[0].messages.clone();
        assert_eq!(
            serde_json::to_value(&sent).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );

        let seed = lines(&second)
            .into_iter()
            .find(|v| kind(v) == "seed")
            .expect("the continued transcript starts with a seed");
        assert_eq!(
            seed["data"]["source_transcript"],
            first.display().to_string()
        );
        assert!(
            seed["data"].get("messages").is_none(),
            "seeded by reference, not copied"
        );

        expected.push(Message::assistant("finished after resume"));
        assert_eq!(
            serde_json::to_value(reconstruct_transcript(&second).unwrap()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        match prepare_continuation(&second).unwrap() {
            Continuation::Finished { output } => assert_eq!(output, "finished after resume"),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    /// The coverage manifest of a continued run records what it continued: its
    /// own `user_prompt` is only the continuation note, so `rupu coverage
    /// rerun` needs the link to find the run that has the real prompt. An
    /// ordinary run records nothing.
    #[tokio::test]
    async fn a_continued_run_records_what_it_continued_on_its_coverage_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let first = finished_transcript(tmp.path()).await;
        write_lines(&first, &through_first_turn_end(&lines(&first)));
        let Continuation::Resume {
            messages,
            seed_source,
            ..
        } = prepare_continuation(&first).unwrap()
        else {
            panic!("expected Resume");
        };

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let second = tmp.path().join("second.jsonl");
        let mut opts = opts_for(Box::new(provider), tmp.path(), second);
        opts.tool_context.identity_mut().run_id = "run_second".into();
        opts.concerns = Some(stride_concerns());
        let mut opts = crate::grant::with_grant(opts, None, &[]).expect("grant");
        apply_continuation(&mut opts.prompt, messages, seed_source, Vec::new());
        run_agent(opts).await.unwrap();

        let paths = rupu_coverage::CoveragePaths::new(
            tmp.path(),
            &rupu_coverage::target_id(tmp.path(), "test-agent"),
        );
        let m = rupu_coverage::find_manifest(&paths, "run_second")
            .unwrap()
            .expect("the continued run wrote a manifest");
        assert_eq!(m.user_prompt, CONTINUATION_NOTE);
        assert_eq!(m.continued_from, Some(first.display().to_string()));
        // ...and rerun refuses it, pointing at the run it continued.
        assert!(matches!(
            rupu_coverage::plan_rerun(&m),
            Err(rupu_coverage::RerunError::ContinuedRun { continued, .. }) if continued == "first"
        ));
    }

    /// Continue `interrupted` into `dest` the way the CLI does, answering
    /// `answer`. Returns the conversation the provider was sent.
    async fn continue_run(interrupted: &Path, dest: &Path, answer: &str) -> Vec<Message> {
        let Continuation::Resume {
            messages,
            seed_source,
            ..
        } = prepare_continuation(interrupted).unwrap()
        else {
            panic!("expected Resume");
        };
        let provider =
            crate::runner::CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
                text: answer.into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
        let captured = provider.captured.clone();
        let dir = dest.parent().unwrap();
        let mut opts = opts_for(Box::new(provider), dir, dest.to_path_buf());
        apply_continuation(&mut opts.prompt, messages, seed_source, Vec::new());
        run_agent(opts).await.unwrap();
        let sent = captured.lock().unwrap()[0].messages.clone();
        sent
    }

    /// Cut a continued transcript back to before its first turn — a runner
    /// that died waiting on the model leaves the seed and the note behind.
    fn die_before_first_turn(t: &Path) {
        let all = lines(t);
        let first_turn = all.iter().position(|v| kind(v) == "turn_start").unwrap();
        write_lines(t, &all[..first_turn]);
    }

    fn note_blocks(messages: &[Message]) -> usize {
        messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::Text { text } if text == CONTINUATION_NOTE))
            .count()
    }

    /// A -> B -> C. A died after its tool turn; B continued it and died
    /// before its first turn, so B's conversation already ends with the note.
    /// C must not stack a second note on top, and replay must still rebuild
    /// the whole conversation across all three files.
    #[tokio::test]
    async fn re_continuing_a_continuation_that_died_early_adds_no_second_note() {
        let tmp = tempfile::tempdir().unwrap();
        let a = finished_transcript(tmp.path()).await;
        write_lines(&a, &through_first_turn_end(&lines(&a)));

        let b = tmp.path().join("b.jsonl");
        let sent_b = continue_run(&a, &b, "from b").await;
        assert_eq!(note_blocks(&sent_b), 1);
        die_before_first_turn(&b);

        let c = tmp.path().join("c.jsonl");
        let sent_c = continue_run(&b, &c, "from c").await;
        assert_eq!(
            note_blocks(&sent_c),
            1,
            "the note is already in the conversation; sending it again would repeat it"
        );
        assert_eq!(
            serde_json::to_value(&sent_c).unwrap(),
            serde_json::to_value(&sent_b).unwrap(),
            "C resumes from exactly the conversation B had sent"
        );

        let mut expected = sent_c;
        expected.push(Message::assistant("from c"));
        assert_eq!(
            serde_json::to_value(reconstruct_transcript(&c).unwrap()).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "replay resolves C -> B -> A"
        );
        match prepare_continuation(&c).unwrap() {
            Continuation::Finished { output } => assert_eq!(output, "from c"),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    /// `apply_continuation` on a conversation that already ends with the note
    /// runs seed-only: the note is not sent twice.
    #[test]
    fn apply_continuation_is_seed_only_when_the_note_is_already_last() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), tmp.path().join("t.jsonl"));
        let mut tool_turn = Message::user("go");
        tool_turn.content.push(ContentBlock::Text {
            text: CONTINUATION_NOTE.into(),
        });
        let messages = vec![Message::assistant("thinking"), tool_turn];
        apply_continuation(
            &mut opts.prompt,
            messages.clone(),
            PathBuf::from("/t/b.jsonl"),
            Vec::new(),
        );
        assert_eq!(opts.prompt.message, "");
        assert_eq!(
            serde_json::to_value(&opts.prompt.initial_messages).unwrap(),
            serde_json::to_value(&messages).unwrap()
        );
        assert_eq!(opts.prompt.seed_source, Some(PathBuf::from("/t/b.jsonl")));
        // A seed-only continuation is still a continuation.
        assert_eq!(continued_from(&opts.prompt), Some(Path::new("/t/b.jsonl")));
    }

    /// A continued transcript whose referenced source was edited after the
    /// fact no longer replays: the seed's recorded hash catches it.
    #[tokio::test]
    async fn a_continuation_whose_source_was_edited_does_not_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let a = finished_transcript(tmp.path()).await;
        write_lines(&a, &through_first_turn_end(&lines(&a)));
        let b = tmp.path().join("b.jsonl");
        continue_run(&a, &b, "from b").await;
        die_before_first_turn(&b);
        assert!(matches!(
            prepare_continuation(&b).unwrap(),
            Continuation::Resume { .. }
        ));

        let edited: Vec<_> = lines(&a)
            .into_iter()
            .map(|mut v| {
                if kind(&v) == "tool_result" {
                    v["data"]["output"] = "tampered".into();
                }
                v
            })
            .collect();
        write_lines(&a, &edited);
        match prepare_continuation(&b) {
            Err(ContinuationError::Replay { source, .. }) => {
                assert!(matches!(source, ReplayError::SeedHashMismatch { .. }))
            }
            other => panic!("expected Replay(SeedHashMismatch), got {other:?}"),
        }
    }

    /// A real run that refuses on its only turn with no fallback chain: it
    /// ends `RunComplete { status: error, outcome: Some(refusal) }`.
    async fn refused_transcript(dir: &Path) -> PathBuf {
        let transcript = dir.join("refused.jsonl");
        let provider = MockProvider::new(vec![ScriptedTurn::Reply {
            content: vec![ContentBlock::Text {
                text: "I won't do that.".into(),
            }],
            stop: rupu_providers::types::Stop::synthetic(StopReason::Refusal, "mock"),
            usage: Default::default(),
        }]);
        let mut opts = opts_for(Box::new(provider), dir, transcript.clone());
        opts.prompt.message = "do the thing".into();
        let result = run_agent(opts).await.unwrap();
        assert_eq!(result.status, RunStatus::Error);
        assert!(result.outcome.is_some(), "the refusal is the run's outcome");
        transcript
    }

    #[tokio::test]
    async fn a_run_that_failed_on_an_outcome_resumes_only_for_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let t = refused_transcript(tmp.path()).await;
        assert!(matches!(
            prepare_continuation(&t).unwrap(),
            Continuation::Failed { .. }
        ));
        match prepare_recovery_continuation(&t).unwrap() {
            Continuation::Resume {
                messages,
                seed_source,
                stopped_on,
                ..
            } => {
                assert_eq!(seed_source, t);
                assert_eq!(
                    serde_json::to_value(&messages).unwrap(),
                    serde_json::to_value(vec![Message::user("do the thing")]).unwrap(),
                    "the refused turn was discarded; the prompt survives"
                );
                let outcome = stopped_on.expect("a failed run names its outcome");
                assert_eq!(outcome.class, "refusal");
                assert!(outcome.title.starts_with("refused"), "{}", outcome.title);
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_run_that_failed_without_an_outcome_is_failed_for_both() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let mut all = lines(&t);
        let i = all.iter().position(|v| kind(v) == "run_complete").unwrap();
        all[i]["data"]["status"] = "error".into();
        all[i]["data"]["error"] = "max turns (5) reached".into();
        write_lines(&t, &all);
        assert!(matches!(
            prepare_continuation(&t).unwrap(),
            Continuation::Failed { .. }
        ));
        match prepare_recovery_continuation(&t).unwrap() {
            Continuation::Failed { error, .. } => {
                assert_eq!(error.as_deref(), Some("max turns (5) reached"))
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// A failed run whose conversation ends in an assistant message (a kept
    /// reply, then a provider failure) is resumed, not read as finished: it
    /// did not finish.
    #[tokio::test]
    async fn a_failed_run_ending_on_an_assistant_reply_resumes_for_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let mut all = lines(&t);
        let i = all.iter().position(|v| kind(v) == "run_complete").unwrap();
        all[i]["data"]["status"] = "error".into();
        all[i]["data"]["outcome"] = serde_json::json!({
            "id": "oc_1",
            "class": "provider_error",
            "severity": "error",
            "title": "provider error · overloaded",
        });
        write_lines(&t, &all);
        match prepare_recovery_continuation(&t).unwrap() {
            Continuation::Resume {
                messages,
                stopped_on,
                ..
            } => {
                assert_eq!(messages.last().unwrap().role, Role::Assistant);
                assert_eq!(stopped_on.unwrap().title, "provider error · overloaded");
            }
            other => panic!("expected Resume, got {other:?}"),
        }
    }

    /// An interrupted run classifies the same way under both entry points.
    #[tokio::test]
    async fn an_interrupted_run_resumes_the_same_way_for_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        write_lines(&t, &through_first_turn_end(&lines(&t)));
        match prepare_recovery_continuation(&t).unwrap() {
            Continuation::Resume { stopped_on, .. } => assert!(stopped_on.is_none()),
            other => panic!("expected Resume, got {other:?}"),
        }
        assert_resumes_after_tool_turn(prepare_recovery_continuation(&t).unwrap(), &t);
    }

    #[test]
    fn apply_continuation_with_sends_the_given_note() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), tmp.path().join("t.jsonl"));
        let messages = vec![Message::user("do the thing")];
        let note = crate::recovery::recovery_retry_note("refused", "anthropic", "mock-2");
        apply_continuation_with(
            &mut opts.prompt,
            messages.clone(),
            PathBuf::from("/t/a.jsonl"),
            Vec::new(),
            note.clone(),
        );
        assert_eq!(opts.prompt.message, note);
        assert_eq!(
            serde_json::to_value(&opts.prompt.initial_messages).unwrap(),
            serde_json::to_value(&messages).unwrap()
        );
        assert_eq!(opts.prompt.seed_source, Some(PathBuf::from("/t/a.jsonl")));
        // A recovery continuation is a continuation: its coverage manifest
        // points `rupu coverage rerun` at the run with the real prompt.
        assert_eq!(continued_from(&opts.prompt), Some(Path::new("/t/a.jsonl")));
    }

    /// A continuation replaces the turn's message, and with it the bodies of
    /// any skills the operator injected there: their loads go too.
    #[test]
    fn a_continuation_drops_the_operator_loads_of_the_message_it_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), tmp.path().join("t.jsonl"));
        opts.prompt.message = "<skill name=\"pdf\">…</skill>\n\nsummarise".into();
        opts.prompt.operator_loads = vec![crate::OperatorLoad {
            name: "pdf".into(),
            layer: "global".into(),
            content_hash: "0".repeat(64),
            base_dir: "/skills/pdf".into(),
        }];
        apply_continuation(
            &mut opts.prompt,
            vec![Message::user("first")],
            PathBuf::from("/t/c.jsonl"),
            Vec::new(),
        );
        assert_eq!(opts.prompt.message, CONTINUATION_NOTE);
        assert_eq!(opts.prompt.operator_loads, Vec::new());
    }

    /// A seeded run with an ordinary prompt (a session's next turn) is not a
    /// continuation.
    #[test]
    fn a_seeded_run_with_an_ordinary_prompt_is_not_a_continuation() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = MockProvider::new(vec![]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), tmp.path().join("t.jsonl"));
        apply_continuation_with(
            &mut opts.prompt,
            vec![Message::user("first")],
            PathBuf::from("/t/a.jsonl"),
            Vec::new(),
            "A previous attempt at this task stopped early; carry on".into(),
        );
        assert_eq!(continued_from(&opts.prompt), None);
    }

    /// A failed continuation says which run it was seeded from — that run
    /// may still be continued.
    #[tokio::test]
    async fn a_failed_continuation_names_the_run_it_was_seeded_from() {
        let tmp = tempfile::tempdir().unwrap();
        let a = finished_transcript(tmp.path()).await;
        write_lines(&a, &through_first_turn_end(&lines(&a)));
        let b = tmp.path().join("b.jsonl");
        continue_run(&a, &b, "from b").await;
        let mut all = lines(&b);
        let i = all.iter().position(|v| kind(v) == "run_complete").unwrap();
        all[i]["data"]["status"] = "error".into();
        write_lines(&b, &all);
        match prepare_continuation(&b).unwrap() {
            Continuation::Failed { seeded_from, .. } => assert_eq!(seeded_from, Some(a)),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ── prompted continuations (`rupu run --continue <id> <prompt>`) ──────

    /// Continue `prev` into `dest` with `prompt`, the way the CLI does,
    /// answering `answer`. Returns the conversation the provider was sent.
    async fn continue_with_prompt(
        prev: &Path,
        dest: &Path,
        prompt: &str,
        answer: &str,
    ) -> Vec<Message> {
        let prepared = prepare_prompted_continuation(prev).unwrap();
        let provider =
            crate::runner::CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
                text: answer.into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
        let captured = provider.captured.clone();
        let mut opts = opts_for(
            Box::new(provider),
            dest.parent().unwrap(),
            dest.to_path_buf(),
        );
        opts.prompt.message = prompt.into();
        apply_prompted_continuation(&mut opts.prompt, prepared, "anthropic", "mock-1");
        run_agent(opts).await.unwrap();
        let sent = captured.lock().unwrap()[0].messages.clone();
        sent
    }

    fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    fn user_messages(t: &Path) -> Vec<String> {
        lines(t)
            .into_iter()
            .filter(|v| kind(v) == "user_message")
            .map(|v| v["data"]["content"].as_str().unwrap().to_string())
            .collect()
    }

    fn text(t: &str) -> ContentBlock {
        ContentBlock::Text { text: t.into() }
    }

    /// Replay of the continuation rebuilds exactly what its provider was
    /// sent, plus its answer: the runner and replay merge the note and the
    /// prompt by the same rule.
    fn assert_replays_as_sent(dest: &Path, sent: &[Message], answer: &str) {
        let mut expected = sent.to_vec();
        expected.push(Message::assistant(answer));
        assert_eq!(
            json(&reconstruct_transcript(dest).unwrap()),
            json(&expected)
        );
    }

    #[tokio::test]
    async fn a_finished_run_continued_with_a_prompt_takes_it_as_a_new_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let first = finished_transcript(tmp.path()).await;
        let prepared = prepare_prompted_continuation(&first).unwrap();
        assert_eq!(prepared.ended, PriorEnd::Finished);
        let rebuilt = prepared.messages.clone();
        assert_eq!(rebuilt.last().unwrap().role, Role::Assistant);

        let second = tmp.path().join("second.jsonl");
        let sent = continue_with_prompt(&first, &second, "now list them", "alpha, beta").await;
        let mut expected = rebuilt;
        expected.push(Message::user("now list them"));
        assert_eq!(json(&sent), json(&expected), "no note: the prompt alone");
        assert_eq!(user_messages(&second), ["now list them"]);
        assert_replays_as_sent(&second, &sent, "alpha, beta");

        // Continuing THAT run with no prompt finds it finished: the prompt is
        // in its conversation and is never sent again.
        match prepare_continuation(&second).unwrap() {
            Continuation::Finished { output } => assert_eq!(output, "alpha, beta"),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_interrupted_run_continued_with_a_prompt_gets_the_note_then_the_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let first = finished_transcript(tmp.path()).await;
        write_lines(&first, &through_first_turn_end(&lines(&first)));
        let prepared = prepare_prompted_continuation(&first).unwrap();
        assert_eq!(prepared.ended, PriorEnd::Interrupted);
        let rebuilt = prepared.messages.clone();

        let second = tmp.path().join("second.jsonl");
        let sent = continue_with_prompt(&first, &second, "skip beta", "done").await;
        let mut expected = rebuilt;
        expected[2].content.push(text(CONTINUATION_NOTE));
        expected[2].content.push(text("skip beta"));
        assert_eq!(
            json(&sent),
            json(&expected),
            "both join the trailing user turn"
        );
        assert_eq!(user_messages(&second), [CONTINUATION_NOTE, "skip beta"]);
        assert_replays_as_sent(&second, &sent, "done");
    }

    /// A -> B (with a prompt) died before its first turn; C continues B. With
    /// no prompt it sends exactly what B sent (the note and B's prompt once
    /// each); with a prompt it adds only the new prompt.
    #[tokio::test]
    async fn re_continuing_a_prompted_continuation_never_resends_its_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let a = finished_transcript(tmp.path()).await;
        write_lines(&a, &through_first_turn_end(&lines(&a)));
        let b = tmp.path().join("b.jsonl");
        let sent_b = continue_with_prompt(&a, &b, "skip beta", "from b").await;
        die_before_first_turn(&b);

        let c = tmp.path().join("c.jsonl");
        let sent_c = continue_run(&b, &c, "from c").await;
        assert_eq!(
            json(&sent_c),
            json(&sent_b),
            "seed-only: nothing is sent again"
        );
        assert_eq!(user_messages(&c), Vec::<String>::new());

        let d = tmp.path().join("d.jsonl");
        let sent_d = continue_with_prompt(&b, &d, "and gamma", "from d").await;
        assert_eq!(note_blocks(&sent_d), 1, "the note is already in the turn");
        let mut expected = sent_b;
        expected.last_mut().unwrap().content.push(text("and gamma"));
        assert_eq!(json(&sent_d), json(&expected));
        assert_replays_as_sent(&d, &sent_d, "from d");
    }

    #[tokio::test]
    async fn a_run_that_failed_on_an_outcome_continues_with_a_prompt_on_the_same_model() {
        let tmp = tempfile::tempdir().unwrap();
        let first = refused_transcript(tmp.path()).await;
        let prepared = prepare_prompted_continuation(&first).unwrap();
        let PriorEnd::Failed {
            outcome: Some(outcome),
            ..
        } = &prepared.ended
        else {
            panic!("expected Failed with an outcome, got {:?}", prepared.ended);
        };
        let note = crate::recovery::recovery_retry_note(&outcome.title, "anthropic", "mock-1");

        let second = tmp.path().join("second.jsonl");
        let sent = continue_with_prompt(&first, &second, "it is authorised", "ok").await;
        let mut turn = Message::user("do the thing");
        turn.content.push(text(&note));
        turn.content.push(text("it is authorised"));
        assert_eq!(json(&sent), json(&vec![turn]));
        assert_replays_as_sent(&second, &sent, "ok");
    }

    #[tokio::test]
    async fn a_run_that_failed_without_an_outcome_continues_with_a_prompt_and_no_note() {
        let tmp = tempfile::tempdir().unwrap();
        let first = finished_transcript(tmp.path()).await;
        let mut all = through_first_turn_end(&lines(&first));
        all.push(serde_json::json!({ "type": "run_complete", "data": {
            "run_id": "r", "status": "error", "total_tokens": 0, "duration_ms": 0,
            "error": "max turns (1) reached",
        } }));
        write_lines(&first, &all);
        assert!(matches!(
            prepare_continuation(&first).unwrap(),
            Continuation::Failed { .. }
        ));
        let prepared = prepare_prompted_continuation(&first).unwrap();
        assert_eq!(
            prepared.ended,
            PriorEnd::Failed {
                error: Some("max turns (1) reached".into()),
                outcome: None
            }
        );
        let rebuilt = prepared.messages.clone();

        let second = tmp.path().join("second.jsonl");
        let sent = continue_with_prompt(&first, &second, "wrap up now", "done").await;
        let mut expected = rebuilt;
        expected[2].content.push(text("wrap up now"));
        assert_eq!(json(&sent), json(&expected));
        assert_eq!(user_messages(&second), ["wrap up now"]);
        assert_replays_as_sent(&second, &sent, "done");
    }

    #[tokio::test]
    async fn a_run_that_answered_before_run_complete_landed_is_finished_for_a_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let all: Vec<_> = lines(&t)
            .into_iter()
            .filter(|v| kind(v) != "run_complete")
            .collect();
        write_lines(&t, &all);
        assert_eq!(
            prepare_prompted_continuation(&t).unwrap().ended,
            PriorEnd::Finished
        );
    }

    #[tokio::test]
    async fn a_prompted_continuation_still_refuses_a_dangling_tool_call() {
        let tmp = tempfile::tempdir().unwrap();
        let t = finished_transcript(tmp.path()).await;
        let cut: Vec<_> = through_first_turn_end(&lines(&t))
            .into_iter()
            .filter(|v| kind(v) != "tool_result")
            .collect();
        write_lines(&t, &cut);
        assert!(matches!(
            prepare_prompted_continuation(&t),
            Err(ContinuationError::DanglingToolCall { .. })
        ));
    }

    /// The prompt keeps the skills the operator invoked in it; a prompted
    /// continuation is recorded as one on the coverage manifest.
    #[tokio::test]
    async fn a_prompted_continuation_keeps_the_prompts_operator_loads_and_is_a_continuation() {
        let tmp = tempfile::tempdir().unwrap();
        let first = finished_transcript(tmp.path()).await;
        let provider = MockProvider::new(vec![]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), tmp.path().join("t.jsonl"));
        opts.prompt.message = "<skill name=\"pdf\">…</skill>\n\nsummarise".into();
        let load = crate::OperatorLoad {
            name: "pdf".into(),
            layer: "global".into(),
            content_hash: "0".repeat(64),
            base_dir: "/skills/pdf".into(),
        };
        opts.prompt.operator_loads = vec![load.clone()];
        let prepared = prepare_prompted_continuation(&first).unwrap();
        apply_prompted_continuation(&mut opts.prompt, prepared, "anthropic", "mock-1");
        assert_eq!(opts.prompt.message, "");
        assert_eq!(
            opts.prompt.continue_prompt.as_deref(),
            Some("<skill name=\"pdf\">…</skill>\n\nsummarise")
        );
        assert_eq!(opts.prompt.operator_loads, vec![load]);
        assert_eq!(continued_from(&opts.prompt), Some(first.as_path()));
    }
}
