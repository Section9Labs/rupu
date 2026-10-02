# Recover on interrupt — Plan 1: the continuation primitive

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let any interrupted agent run be continued from its on-disk transcript — the conversation rebuilt, the agent told it was interrupted — exposed as `rupu run <agent> --continue <agent_run_id>`.

**Architecture:** A new `rupu_agent::continuation` module classifies a transcript (finished / resumable / failed) and rebuilds its conversation with the existing `replay::reconstruct_transcript`. A continuation seeds the new run *by reference* to the old transcript (`AgentRunOpts::seed_source`) and passes a fixed note as `user_message`; a new role-alternation rule (applied identically in `run_agent` and in replay) merges that note into the trailing user message so no provider sees two user messages in a row. The CLI only parses `--continue` and delegates.

**Tech Stack:** Rust 2021, tokio, serde_json, thiserror (library errors), anyhow (CLI), clap, assert_cmd (CLI tests).

**Spec:** `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md` (this plan is its Rollout step 1: §1 "The continuation primitive").

## Global Constraints

- `rupu-cli` is thin: argument parsing + delegation only; all classification/rebuild logic lives in `rupu-agent`.
- Workspace deps only — never add a version to a crate `Cargo.toml`.
- Libraries use `thiserror`; the CLI uses `anyhow`.
- `#![deny(clippy::all)]` workspace-wide: `cargo clippy -p rupu-agent -p rupu-cli --all-targets -- -D warnings` must be clean (a pre-existing `clippy::question_mark` hit in `crates/rupu-cli/src/cmd/completers.rs` from a newer local toolchain may be allowed with `-A clippy::question_mark` when checking; do not touch that file).
- Integration tests: ONE binary per crate — new modules go under `crates/<crate>/tests/it/` and are listed in that dir's `main.rs`; never add a top-level `tests/*.rs`.
- Formatting: run `rustfmt --edition 2021 <file>` only on files you changed; NEVER `cargo fmt` on a whole package (main is fmt-dirty).
- Git: never run bare `git stash` / `git stash pop`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Public repo: test data is invented from scratch — no names, paths or content from any real assessment.
- The continuation note text is exactly:
  `This session was interrupted and has been resumed. Work from the step you were in the middle of was lost and may be partially applied — check the current state, then continue the task.`

## File Structure

- `crates/rupu-agent/src/runner.rs` — add `push_user_turn` (shared role-alternation helper); use it where `run_agent` appends `user_message`.
- `crates/rupu-agent/src/replay.rs` — use `push_user_turn` for `Event::UserMessage`; add the lockstep round-trip test.
- `crates/rupu-agent/src/continuation.rs` (new) — `CONTINUATION_NOTE`, `Continuation`, `ContinuationError`, `prepare_continuation`, `transcript_agent`, `apply_continuation`, with unit + round-trip tests.
- `crates/rupu-agent/src/lib.rs` — `pub mod continuation;`.
- `crates/rupu-cli/src/cmd/run.rs` — `--continue` flag and delegation.
- `crates/rupu-cli/src/cmd/coverage.rs` — add `continue_from: None` to its literal `run::Args { … }`.
- `crates/rupu-cli/tests/it/run_continue.rs` (new) + `crates/rupu-cli/tests/it/main.rs` — CLI tests.
- `docs/agent-format.md`, `CLAUDE.md` — docs.

---

### Task 1: Role-alternation rule in the runtime and in replay (lockstep)

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (new fn near `seed_sha256`, ~line 159; the `if !opts.user_message.is_empty()` block in `run_agent`, ~line 1452)
- Modify: `crates/rupu-agent/src/replay.rs` (the `Event::UserMessage` arm in `reconstruct_with`; tests module at the bottom)

**Interfaces:**
- Produces: `pub(crate) fn push_user_turn(messages: &mut Vec<Message>, text: &str)` in `crate::runner`.

- [ ] **Step 1: Write the failing test** — append to the `#[cfg(test)] mod tests` in `crates/rupu-agent/src/replay.rs`:

```rust
    /// Recover-on-interrupt spec §1: a non-empty `user_message` that follows a
    /// seed ending in a user turn joins that turn instead of becoming a second
    /// consecutive user message — and replay rebuilds exactly what was sent.
    #[tokio::test]
    async fn a_user_message_after_a_seed_ending_in_a_user_turn_merges_into_it() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript = tmp.path().join("merge.jsonl");
        let seed = vec![
            Message::user("summarise notes.txt"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "notes.txt" }),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "alpha\nbeta".into(),
                    is_error: false,
                }],
            },
        ];
        let provider = crate::runner::CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "summary done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let captured = provider.captured.clone();
        let mut opts = opts_for(Box::new(provider), tmp.path(), transcript.clone());
        opts.initial_messages = seed.clone();
        opts.user_message = "you were interrupted".into();
        run_agent(opts).await.unwrap();

        let mut expected = seed;
        expected[2].content.push(ContentBlock::Text {
            text: "you were interrupted".into(),
        });
        let sent = captured.lock().unwrap()[0].messages.clone();
        assert_eq!(
            serde_json::to_value(&sent).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "the note must join the trailing user turn, not follow it"
        );

        expected.push(Message::assistant("summary done"));
        let replayed = reconstruct_transcript(&transcript).unwrap();
        assert_eq!(
            serde_json::to_value(&replayed).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "replay must rebuild exactly what the runner sent"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p rupu-agent --lib replay::tests::a_user_message_after_a_seed_ending_in_a_user_turn_merges_into_it`
Expected: FAIL — the sent conversation has 4 messages (a second user message), not the merged 3.

- [ ] **Step 3: Add the helper** to `crates/rupu-agent/src/runner.rs`, directly after `seed_sha256` (make sure `Role` and `ContentBlock` are imported from `rupu_providers::types` in this file; add them to the existing `use` if missing):

```rust
/// Append a user turn without breaking role alternation. When the
/// conversation already ends in a user message (a seed that ends on the
/// prompt or on tool results), `text` joins that message as an extra block
/// instead of becoming a second consecutive user message, which some
/// providers reject. `replay::reconstruct_messages` applies the same rule,
/// so replaying a transcript keeps reproducing what the runner sent.
pub(crate) fn push_user_turn(messages: &mut Vec<Message>, text: &str) {
    match messages.last_mut() {
        Some(last) if last.role == Role::User => last.content.push(ContentBlock::Text {
            text: text.to_string(),
        }),
        _ => messages.push(Message::user(text)),
    }
}
```

- [ ] **Step 4: Use it in `run_agent`** — in the block that starts `if !opts.user_message.is_empty() {`, replace

```rust
        messages.push(Message::user(&opts.user_message));
```

with

```rust
        push_user_turn(&mut messages, &opts.user_message);
```

(leave the `writer.write(&Event::UserMessage { … })` line that follows unchanged — the transcript still records the note as its own event).

- [ ] **Step 5: Use it in replay** — in `crates/rupu-agent/src/replay.rs` `reconstruct_with`, replace

```rust
            Event::UserMessage { content } => messages.push(Message::user(content)),
```

with

```rust
            Event::UserMessage { content } => crate::runner::push_user_turn(&mut messages, content),
```

- [ ] **Step 6: Run the new test and the whole crate**

Run: `cargo test -p rupu-agent`
Expected: all pass, including the new test and the existing replay round-trip tests.

- [ ] **Step 7: Format + commit**

```bash
rustfmt --edition 2021 crates/rupu-agent/src/runner.rs crates/rupu-agent/src/replay.rs
git add crates/rupu-agent/src/runner.rs crates/rupu-agent/src/replay.rs
git commit -m "feat(agent): merge a user turn into a trailing user message, in runtime and replay

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `continuation` module — classify a transcript and rebuild it

**Files:**
- Create: `crates/rupu-agent/src/continuation.rs`
- Modify: `crates/rupu-agent/src/lib.rs` (add `pub mod continuation;` in alphabetical order with the other `pub mod` lines)

**Interfaces:**
- Consumes: `crate::replay::{reconstruct_transcript, ReplayError}`; `rupu_transcript::{Event, JsonlReader, ReadError, RunStatus}`.
- Produces:
  - `pub const CONTINUATION_NOTE: &str`
  - `pub enum Continuation { Finished { output: String }, Resume { messages: Vec<Message>, seed_source: PathBuf }, Failed { error: Option<String> } }` (derive `Debug`)
  - `pub enum ContinuationError` (thiserror) with variants `Read { path: String, source: ReadError }`, `NotAnAgentRun { path: String }`, `DanglingToolCall { path: String }`, `Replay { path: String, source: ReplayError }`
  - `pub fn prepare_continuation(transcript: &Path) -> Result<Continuation, ContinuationError>`
  - `pub fn transcript_agent(transcript: &Path) -> Result<String, ContinuationError>`

- [ ] **Step 1: Create the module with the tests first** — `crates/rupu-agent/src/continuation.rs`:

```rust
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
            Continuation::Resume { messages, seed_source } => {
                assert_eq!(seed_source, transcript);
                let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
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
        let mut complete = all.iter().find(|v| kind(v) == "run_complete").unwrap().clone();
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
```

Add `pub mod continuation;` to `crates/rupu-agent/src/lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-agent --lib continuation::`
Expected: compile error — `prepare_continuation` and `transcript_agent` are not defined.

- [ ] **Step 3: Implement** — add above `#[cfg(test)]` in `continuation.rs`:

```rust
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
    let messages = reconstruct_transcript(transcript).map_err(|source| {
        ContinuationError::Replay {
            path: path.clone(),
            source,
        }
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
                // `RunComplete` landed. Nothing left to do.
                Ok(Continuation::Finished {
                    output: text_of(last),
                })
            }
        }
        Some(_) => Ok(Continuation::Resume {
            messages,
            seed_source: transcript.to_path_buf(),
        }),
    }
}

/// The last non-empty assistant text in the transcript — the run's answer.
fn final_assistant_text(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::AssistantMessage { content, .. } if !content.is_empty() => {
                Some(content.clone())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn text_of(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

(If `RunStatus` is not `Copy`, use `status.clone()`; if `Role` is not `Copy`, compare with `messages.iter().map(|m| m.role.clone())` in the test helper.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-agent --lib continuation::`
Expected: all 10 tests PASS.

- [ ] **Step 5: Format + commit**

```bash
rustfmt --edition 2021 crates/rupu-agent/src/continuation.rs crates/rupu-agent/src/lib.rs
git add crates/rupu-agent/src/continuation.rs crates/rupu-agent/src/lib.rs
git commit -m "feat(agent): classify and rebuild an interrupted run from its transcript

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `apply_continuation` + end-to-end continuation round-trip

**Files:**
- Modify: `crates/rupu-agent/src/continuation.rs`

**Interfaces:**
- Consumes: `CONTINUATION_NOTE`, `Continuation::Resume` (Task 2); `runner::push_user_turn` behaviour (Task 1).
- Produces: `pub fn apply_continuation(opts: &mut crate::runner::AgentRunOpts, messages: Vec<Message>, seed_source: PathBuf)`.

- [ ] **Step 1: Write the failing test** — append inside `mod tests` in `continuation.rs`:

```rust
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

        let Continuation::Resume { messages, seed_source } = prepare_continuation(&first).unwrap()
        else {
            panic!("expected Resume");
        };
        let rebuilt = messages.clone();

        let second = tmp.path().join("second.jsonl");
        let provider = crate::runner::CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "finished after resume".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let captured = provider.captured.clone();
        let mut opts = opts_for(Box::new(provider), tmp.path(), second.clone());
        opts.user_message = "this must be replaced".into();
        apply_continuation(&mut opts, messages, seed_source);
        assert_eq!(opts.user_message, CONTINUATION_NOTE);
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
        assert_eq!(seed["data"]["source_transcript"], first.display().to_string());
        assert!(seed["data"].get("messages").is_none(), "seeded by reference, not copied");

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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p rupu-agent --lib continuation::tests::a_continued_run_picks_up_where_the_interrupted_one_stopped`
Expected: compile error — `apply_continuation` not defined.

- [ ] **Step 3: Implement** — add to `continuation.rs` (above the tests):

```rust
/// Point a freshly built agent run at the interrupted one: seed it with the
/// rebuilt conversation *by reference* to `seed_source` and hand it the
/// continuation note (which joins the conversation's trailing user turn).
/// Everything else on `opts` — agent, provider, model, tools, mode — is the
/// caller's fresh configuration.
pub fn apply_continuation(
    opts: &mut crate::runner::AgentRunOpts,
    messages: Vec<Message>,
    seed_source: PathBuf,
) {
    opts.initial_messages = messages;
    opts.seed_source = Some(seed_source);
    opts.user_message = CONTINUATION_NOTE.to_string();
}
```

- [ ] **Step 4: Run the crate's tests**

Run: `cargo test -p rupu-agent`
Expected: all pass.

- [ ] **Step 5: Format + commit**

```bash
rustfmt --edition 2021 crates/rupu-agent/src/continuation.rs
git add crates/rupu-agent/src/continuation.rs
git commit -m "feat(agent): apply_continuation — seed a run from an interrupted transcript

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `rupu run <agent> --continue <agent_run_id>`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs` (`Args` struct ~line 26; `run_inner` ~line 514: after the `transcript_path` is computed ~line 577, and at the `AgentRunOpts { … }` construction ~line 927)
- Modify: `crates/rupu-cli/src/cmd/coverage.rs:1221` (literal `crate::cmd::run::Args { … }`)
- Create: `crates/rupu-cli/tests/it/run_continue.rs`
- Modify: `crates/rupu-cli/tests/it/main.rs` (add `mod run_continue;` in alphabetical order)

**Interfaces:**
- Consumes: `rupu_agent::continuation::{prepare_continuation, transcript_agent, apply_continuation, Continuation}`.
- Produces: CLI flag `--continue <AGENT_RUN_ID>` on `rupu run`; field `Args::continue_from: Option<String>`.

- [ ] **Step 1: Write the failing tests** — `crates/rupu-cli/tests/it/run_continue.rs`:

```rust
//! `rupu run <agent> --continue <agent_run_id>` (recover-on-interrupt spec §1).

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

fn make_agent(dir: &Path, name: &str) {
    let agents = dir.join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nYou are a test agent.\n"
        ),
    )
    .unwrap();
}

fn rupu(dir: &Path, script: &str) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.current_dir(dir)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", script)
        .env("RUPU_HOME", dir.join(".rupu"));
    cmd
}

fn transcript(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(".rupu/transcripts").join(format!("{run_id}.jsonl"))
}

/// Run `hello` once to completion as `run_first`.
fn first_run(dir: &Path) {
    rupu(dir, r#"[{"AssistantText":{"text":"first answer","stop":"end_turn"}}]"#)
        .args(["run", "hello", "--mode", "bypass", "--run-id", "run_first", "say hi"])
        .assert()
        .success();
    assert!(transcript(dir, "run_first").is_file());
}

/// Cut `run_first`'s transcript back to before its first turn — what a
/// runner killed while waiting on the model leaves behind.
fn interrupt_first_run(dir: &Path) {
    let path = transcript(dir, "run_first");
    let kept: String = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .take_while(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["type"] != "turn_start"
        })
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&path, kept).unwrap();
}

#[test]
fn continue_resumes_an_interrupted_run_seeded_by_reference() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());
    interrupt_first_run(dir.path());

    rupu(dir.path(), r#"[{"AssistantText":{"text":"continued answer","stop":"end_turn"}}]"#)
        .args(["run", "hello", "--mode", "bypass", "--run-id", "run_second", "--continue", "run_first"])
        .assert()
        .success()
        .stdout(predicate::str::contains("continued answer"));

    let seed = std::fs::read_to_string(transcript(dir.path(), "run_second"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "seed")
        .expect("the continued run seeds from the interrupted one");
    assert!(seed["data"]["source_transcript"]
        .as_str()
        .unwrap()
        .ends_with("run_first.jsonl"));
}

#[test]
fn continue_on_a_finished_run_prints_its_answer_without_a_model_call() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    first_run(dir.path());

    // An empty script: any model call would fail the run.
    rupu(dir.path(), "[]")
        .args(["run", "hello", "--mode", "bypass", "--run-id", "run_second", "--continue", "run_first"])
        .assert()
        .success()
        .stdout(predicate::str::contains("first answer"));
    assert!(!transcript(dir.path(), "run_second").exists());
}

#[test]
fn continue_refuses_a_run_of_a_different_agent() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    make_agent(dir.path(), "other");
    first_run(dir.path());
    interrupt_first_run(dir.path());

    rupu(dir.path(), "[]")
        .args(["run", "other", "--mode", "bypass", "--continue", "run_first"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was agent `hello`"));
}

#[test]
fn continue_refuses_an_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    rupu(dir.path(), "[]")
        .args(["run", "hello", "--mode", "bypass", "--continue", "run_missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("can't be read"));
}

#[test]
fn continue_does_not_take_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    make_agent(dir.path(), "hello");
    rupu(dir.path(), "[]")
        .args(["run", "hello", "--continue", "run_first", "another prompt"])
        .assert()
        .failure();
}
```

Add `mod run_continue;` to `crates/rupu-cli/tests/it/main.rs`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rupu-cli --test it run_continue::`
Expected: FAIL — `--continue` is an unknown argument.

- [ ] **Step 3: Add the flag** — in `crates/rupu-cli/src/cmd/run.rs` `Args`, after `findings_profile`:

```rust
    /// Continue an interrupted run of this agent from its transcript instead
    /// of starting fresh: the earlier conversation is rebuilt and the agent
    /// is told it was interrupted. A run that had already finished prints
    /// its recorded answer without calling the model. Run it from the same
    /// project as the original run.
    #[arg(
        long = "continue",
        value_name = "AGENT_RUN_ID",
        conflicts_with_all = ["target", "prompt", "prompt_flag", "into", "tmp"]
    )]
    pub continue_from: Option<String>,
```

and add `continue_from: None,` to the literal `crate::cmd::run::Args { … }` in `crates/rupu-cli/src/cmd/coverage.rs` (~line 1221).

- [ ] **Step 4: Delegate in `run_inner`** — right after

```rust
    let transcript_path = transcripts.join(format!("{run_id}.jsonl"));
```

insert:

```rust
    // `--continue <agent_run_id>`: rebuild the interrupted run's conversation
    // from its transcript (recover-on-interrupt spec §1). Same transcripts
    // dir as the new run, so run it from the same project.
    let mut resume_from: Option<(Vec<rupu_providers::types::Message>, std::path::PathBuf)> =
        match args.continue_from.as_deref() {
            None => None,
            Some(prev) => {
                use rupu_agent::continuation::{prepare_continuation, transcript_agent, Continuation};
                let prev_path = transcripts.join(format!("{prev}.jsonl"));
                let prev_agent = transcript_agent(&prev_path)?;
                if prev_agent != spec.name {
                    anyhow::bail!("run {prev} was agent `{prev_agent}`, not `{}`", spec.name);
                }
                match prepare_continuation(&prev_path)? {
                    Continuation::Finished { output } => {
                        println!("{output}");
                        eprintln!(
                            "run {prev} had already finished — printed its recorded answer without calling the model"
                        );
                        return Ok(());
                    }
                    Continuation::Failed { error } => anyhow::bail!(
                        "run {prev} ended in failure{} — start it fresh instead",
                        error.map(|e| format!(" ({e})")).unwrap_or_default()
                    ),
                    Continuation::Resume { messages, seed_source } => Some((messages, seed_source)),
                }
            }
        };
```

Then at the `AgentRunOpts` construction, change `let opts = AgentRunOpts {` to `let mut opts = AgentRunOpts {` and directly after the struct literal's closing `};` add:

```rust
        if let Some((messages, seed_source)) = resume_from.take() {
            rupu_agent::continuation::apply_continuation(&mut opts, messages, seed_source);
        }
```

If the `AgentRunOpts` construction sits inside a nested block or closure that can't borrow `resume_from` mutably, move `resume_from` into it (it is used exactly once). If `rupu_providers` isn't a direct dependency of `rupu-cli`, use `rupu_agent`'s re-export path for `Message` (check `crates/rupu-cli/Cargo.toml`; add the workspace dep `rupu-providers = { workspace = true }` only if neither works).

- [ ] **Step 5: Run the CLI tests**

Run: `cargo test -p rupu-cli --test it run_continue::`
Expected: all 5 PASS.

- [ ] **Step 6: Run the crate's existing run tests to check for regressions**

Run: `cargo test -p rupu-cli --test it run_` and `cargo test -p rupu-cli --lib cmd::run`
Expected: all pass.

- [ ] **Step 7: Format + commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/src/cmd/coverage.rs crates/rupu-cli/tests/it/run_continue.rs crates/rupu-cli/tests/it/main.rs
git add crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/src/cmd/coverage.rs crates/rupu-cli/tests/it/run_continue.rs crates/rupu-cli/tests/it/main.rs
git commit -m "feat(cli): rupu run --continue <agent_run_id> picks up an interrupted run

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Docs, full verification, PR

**Files:**
- Modify: `docs/agent-format.md` (append a section at the end)
- Modify: `CLAUDE.md` (the `rupu-agent` crate entry; the "Read first" list)

- [ ] **Step 1: Document the flag** — append to `docs/agent-format.md`:

```markdown
## Continuing an interrupted run

If a run stops before it finishes — a closed terminal, a killed process, a
crash — pick it up where it left off instead of starting over:

    rupu run <agent> --continue <agent_run_id>

rupu rebuilds the conversation from the run's transcript, drops the turn that
was in flight when it stopped, and tells the agent it was interrupted (work
from that turn may be partially applied, so the agent re-checks before going
on). The new run's transcript links back to the old one rather than copying
it. If the run had actually finished, its recorded answer is printed without
calling the model; a run that failed can't be continued — start it fresh.
Run the command from the same project as the original run.
```

- [ ] **Step 2: Update `CLAUDE.md`** — append to the end of the `rupu-agent` crate entry:

```markdown
 `continuation` (spec `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md`): `prepare_continuation` classifies a transcript (finished / resume / failed) and rebuilds it via `replay`; `apply_continuation` seeds a new run by reference with the continuation note; `runner::push_user_turn` merges a user turn into a trailing user message, and replay applies the same rule (lockstep). Behind `rupu run --continue`.
```

and add to the "Read first" list:

```markdown
- Recover-on-interrupt spec + Plan 1 (continuation primitive): `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md`, `docs/superpowers/plans/2026-10-01-rupu-recover-on-interrupt-plan-1-primitive.md`
```

- [ ] **Step 3: Full verification**

```bash
cargo test -p rupu-agent
cargo test -p rupu-cli
cargo clippy -p rupu-agent -p rupu-cli --all-targets -- -D warnings -A clippy::question_mark
```

Expected: all tests pass; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add docs/agent-format.md CLAUDE.md
git commit -m "docs: rupu run --continue and the continuation module

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 5: Push and open the PR** (HTTPS with an explicit refspec — SSH push hangs and a bare `git push` pushes every matching branch)

```bash
git push https://github.com/section9labs/rupu.git HEAD:refs/heads/claude/recover-on-interrupt-spec
git ls-remote https://github.com/section9labs/rupu.git refs/heads/claude/recover-on-interrupt-spec
gh pr create --repo section9labs/rupu --base main --head claude/recover-on-interrupt-spec \
  --title "Recover on interrupt, PR 1: continue an agent run from its transcript" \
  --body-file <scratchpad>/pr-body.md
```

The PR body summarises: the spec + this plan, the role-alternation rule (runtime + replay lockstep), the `continuation` module's classification table, `rupu run --continue`, the tests added, and ends with:

```
🤖 Generated with [Claude Code](https://claude.com/claude-code)
```
