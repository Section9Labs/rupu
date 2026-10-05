# Response outcomes — Plan 2: classification and recovery — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The agent runner classifies every reply and provider error into an outcome. It records each outcome, and every recovery step, in the transcript. It climbs rungs 0–2 of the recovery ladder: fix in place, then a fallback model on the same provider, then one on another provider. When the ladder runs out it fails with a typed cause, and that cause reaches the orchestrator's records.

**Architecture:**
- `rupu-transcript` gains the typed records that cross every boundary: `OutcomeRecord`, `StopRecord`, and the `Outcome` / `Recovery` events. It is a leaf crate, so classes are strings there.
- `rupu-agent` gets two new modules, both pure and unit-tested:
  - `outcome` classifies a response or an error into an `Outcome`.
  - `recovery` holds the ladder policy table, the per-run `RecoveryState`, and the `HopBuilder` port.
- The turn loop in `runner.rs` calls into both modules.
- `rupu-runtime` implements `HopBuilder` with the existing provider factory and limits resolver.
- The launch sites pass the resolved fallback chain:
  - the `fallbacks:` frontmatter, else the global `[recovery].fallbacks`;
  - `rupu run`, the workflow step factory, `session _run-turn` and sub-agent dispatch.
- Anthropic API-key requests opt into server-side `fallbacks: "default"`.

**Tech stack:** Rust 2021, serde, tokio, async-trait. Web: TypeScript/React and vitest, for the minimal renderers only.

**Spec:** `docs/superpowers/specs/2026-10-01-rupu-response-outcomes-design.md`. This plan implements §4.5's echo rule, §4.6, §5, §6, §7.1–7.3, and the `rupu run --continue` part of §8.1. Plan 1 (provider outcomes) is merged as PR #729.

## Deviations from the spec (decided while planning; recorded here so reviewers judge them)

1. **Rung 3 in this plan is "fail with a hint".** There is no `RecoveryDecider` port yet. The spec's §12 Plan 2 says "rung 3 as `Never` only", and that is the same behavior. The port and its `Interactive` / `Park` implementations come in Plan 4, with the decisions they need. Adding a decision enum now with no code that handles it would be a mock feature.
2. **`[recovery].park_timeout_secs` waits for Plan 4.** Nothing reads it before then.
3. **New events get minimal renderers in this plan,** in the CLI and the web. Each is one line: severity glyph, title and recovery action. That way no release ships an event that renders blank. Plan 3 replaces these lines with the full presentation layer.
4. **`rupu run --continue` can continue a FAILED run, in one case only.** The run must have ended with an `outcome` (its conversation is intact), and the caller must pass `--model` or `--provider`. This is what makes the rung-3 hint actionable. `prepare_continuation`'s classification is unchanged, so recover-on-interrupt still restarts failed attempts.
5. **The transcript `TurnEnd.stop` is a transcript-side `StopRecord`.** It has `reason: String`, `wire`, `refusal` and `served_by` as JSON, and deserializes from a serialized `rupu_providers::Stop`. `rupu-transcript` does not depend on `rupu-providers`.

## Global constraints

- **Formatting:** never run package-wide `cargo fmt`. Use check-first `rustfmt --edition 2021 --check <file>`, then `rustfmt --edition 2021 <file>`, on files you touched. Never on `lib.rs` / `mod.rs`. Don't reformat unrelated lines.
- **Git:** never run `git stash` in any form. Commit on the branch only. Each commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Clippy:** must pass with `-D warnings`. One exception: `-A clippy::question_mark`, for a pre-existing failure in `crates/rupu-cli/src/cmd/completers.rs` that is identical on `main`. CI's clippy is 1.95, so write `is_none_or` or `!matches!` instead of `!x.is_some_and(..)`, and never write a match arm whose only body is an `if`.
- **Tests:**
  - Integration tests go in `crates/<c>/tests/it/`, listed in `main.rs`.
  - `rupu-cli` tests that mutate env go in `tests/serial/` with `ENV_LOCK`, and run with `< /dev/null`.
  - Tests that build a provider through the factory hold `test_support::ENV_LOCK`.
- **Fixtures:** invent them from scratch. This is a public repo.
- **Serde back-compat:** every new field on a persisted or streamed struct is `#[serde(default, skip_serializing_if = …)]`.
- **Prose:** no "simply", "just", "easy" and no exclamation marks in strings or docs.
- **Budgets** are constants:
  - `PAUSE_TURN_BUDGET = 5`, `TRUNCATION_BUDGET = 3`, `MALFORMED_BUDGET = 2`, `EMPTY_REPLY_BUDGET = 1`, `INCOMPLETE_BUDGET = 1`, `RAISED_CAP_BUDGET = 1`.
  - `MAX_RECOVERY_ACTIONS = 20` per run.
- **Note texts**, verbatim:
  - `TRUNCATION_NOTE` = `Your previous reply was cut off at the output limit. Continue exactly where you stopped; don't repeat what you already wrote.`
  - `EMPTY_REPLY_NOTE` = `Your last reply was empty. Continue the task.`
  - `MALFORMED_NOTE_PREFIX` = `Your last tool call could not be used:`. The full note is `{prefix} {name}: {error}. Call the tool again with valid arguments.`
  - `RECOVERY_RETRY_NOTE` = `A previous attempt at this task stopped ({title}). You are continuing on {provider}/{model}; check the current state, then continue the task.`
- **Server-side fallback models:** exactly `claude-fable-5`, `claude-fable-5-1`, `claude-opus-5`, `claude-opus-5-5`, `claude-sonnet-5-5`. The match is on the model id after the `[1m]` suffix is stripped. The beta is `server-side-fallback-2026-07-01` and the body field is `"fallbacks": "default"`. Both apply on the API-key auth path only, never OAuth.

## File structure

| File | Responsibility |
|---|---|
| `crates/rupu-transcript/src/outcome.rs` (new) | `Severity`, `OutcomeRecord`, `StopRecord`, `RecoveryAction` |
| `crates/rupu-transcript/src/event.rs` | `Event::Outcome`, `Event::Recovery`, `TurnEnd.stop`/`discarded`, `RunComplete.outcome`, `Unknown { tag, data }` with verbatim round-trip |
| `crates/rupu-transcript/src/reader.rs` | `final_turn_text` follows continuation chains |
| `crates/rupu-config/src/recovery_config.rs` (new) | `RecoveryConfig`, `FallbackEntry`, `split_chain` |
| `crates/rupu-agent/src/spec.rs` | `fallbacks:` frontmatter |
| `crates/rupu-agent/src/outcome.rs` (new) | `OutcomeClass`, `Outcome`, `classify_response`, `classify_error` |
| `crates/rupu-agent/src/recovery.rs` (new) | `Rung0`, `policy()`, `RecoveryState`, `HopBuilder`, `Hop`, `RecoveryOpts`, note constants |
| `crates/rupu-agent/src/runner.rs` | turn-loop integration, `RunError::Outcome`, `RunResult.outcome`, `run_agent_full`, summariser stop check |
| `crates/rupu-agent/src/replay.rs` | `discarded` turns dropped, `merge_into_previous` |
| `crates/rupu-agent/src/continuation.rs` | `apply_continuation_with`, `prepare_recovery_continuation` |
| `crates/rupu-providers/src/anthropic.rs` | server-side fallback opt-in, disable-on-400, echo rule |
| `crates/rupu-providers/src/error.rs` | `ProviderError::FallbackUnavailable` |
| `crates/rupu-runtime/src/hop_builder.rs` (new) | `RuntimeHopBuilder` |
| `crates/rupu-runtime/src/provider_factory.rs` | `ProviderConfig.anthropic_server_side_fallback` |
| `crates/rupu-orchestrator/src/{executor/event.rs,runs.rs,runner.rs,step_factory.rs}` | `cause` propagation; executor `Unknown`; chain + hop builder in the step factory |
| `crates/rupu-cli/src/cmd/{run.rs,session.rs,dispatch.rs}` | chain + hop builder wiring; `--continue --model/--provider`; session history on failure |
| CLI renderers (`cmd/transcript.rs`, `cmd/session.rs`, `cmd/autoflow.rs`, `output/workflow_printer.rs`, `output/live_view/mux.rs`, `cmd/run.rs`, `cmd/watch.rs`) | minimal outcome and recovery lines |
| `crates/rupu-cp/web/src/lib/transcript.ts`, `components/transcript/{transcriptView.ts,Turn.tsx}` | minimal outcome and recovery blocks |
| `docs/response-outcomes.md` (new), `CLAUDE.md` | docs |

---

### Task 1: Transcript outcome types and events

**Files:**
- Create: `crates/rupu-transcript/src/outcome.rs`
- Modify: `crates/rupu-transcript/src/lib.rs` (add `pub mod outcome;` and `pub use outcome::{OutcomeRecord, RecoveryAction, Severity, StopRecord};`)
- Modify: `crates/rupu-transcript/src/event.rs`
- Modify: `crates/rupu-transcript/src/reader.rs` (`final_turn_text`)
- Modify: every exhaustive `match` over `Event` that the compiler flags: `crates/rupu-agent/src/replay.rs` (Task 7 implements the replay semantics; add placeholder arms here that keep today's behavior, `{}`) and the CLI renderers (Task 2 renders the events; here add `=> {}` / empty-vec arms only to compile)
- Test: `crates/rupu-transcript/tests/it/outcome_events.rs` (new, registered in `tests/it/main.rs`), plus the `final_turn_text` tests in `tests/it/final_turn_text.rs`

**Interfaces:**
- Produces:
  ```rust
  // rupu_transcript::outcome
  pub enum Severity { Info, Warning, Error }                       // snake_case serde
  pub struct OutcomeRecord { pub id: String, pub class: String, pub severity: Severity,
      pub title: String, pub detail: Option<String>, pub error_class: Option<String>,
      pub wire: serde_json::Value }
  pub struct StopRecord { pub reason: String, pub wire: serde_json::Value,
      pub refusal: Option<serde_json::Value>, pub served_by: Option<serde_json::Value> }
  pub enum RecoveryAction { Continued, Retried, Compacted, FellBack, ServedByFallback, Skipped,
      Asked, Parked, Failed, #[serde(other)] Other }               // snake_case serde
  // rupu_transcript::Event (new / changed)
  Event::Outcome { turn_idx: u32, outcome: OutcomeRecord }
  Event::Recovery { outcome_id: String, rung: u8, action: RecoveryAction, attempt: Option<u32>,
      budget: Option<u32>, provider: Option<String>, model: Option<String>, reason: Option<String>,
      merge_into_previous: bool, continues_output: bool }
  Event::TurnEnd { …existing…, stop: Option<StopRecord>, discarded: bool }
  Event::RunComplete { …existing…, outcome: Option<OutcomeRecord> }
  Event::Unknown { tag: String, data: serde_json::Value }          // never constructed by writers
  ```

- [ ] **Step 1: Write the failing tests** in `crates/rupu-transcript/tests/it/outcome_events.rs`:

```rust
use rupu_transcript::{Event, OutcomeRecord, RecoveryAction, Severity, StopRecord};
use serde_json::json;

fn record() -> OutcomeRecord {
    OutcomeRecord {
        id: "oc_01".into(),
        class: "refusal".into(),
        severity: Severity::Error,
        title: "refused · cyber".into(),
        detail: Some("Declined for this example.".into()),
        error_class: None,
        wire: json!({"provider": "anthropic", "value": "refusal"}),
    }
}

fn roundtrip(e: &Event) {
    let line = serde_json::to_string(e).unwrap();
    let back: Event = serde_json::from_str(&line).unwrap();
    assert_eq!(&back, e, "{line}");
}

#[test]
fn outcome_and_recovery_round_trip() {
    roundtrip(&Event::Outcome { turn_idx: 3, outcome: record() });
    roundtrip(&Event::Recovery {
        outcome_id: "oc_01".into(),
        rung: 1,
        action: RecoveryAction::FellBack,
        attempt: Some(1),
        budget: None,
        provider: Some("anthropic".into()),
        model: Some("claude-opus-4-8".into()),
        reason: None,
        merge_into_previous: false,
        continues_output: false,
    });
}

#[test]
fn turn_end_stop_and_discarded_round_trip_and_default() {
    let stop = StopRecord {
        reason: "refusal".into(),
        wire: json!({"provider": "anthropic", "value": "refusal"}),
        refusal: Some(json!({"category": "cyber", "explanation": null, "recommended_model": null, "source": "classifier"})),
        served_by: None,
    };
    roundtrip(&Event::TurnEnd {
        turn_idx: 1, tokens_in: Some(10), tokens_out: Some(2),
        stop_reason: Some("refusal".into()), response_id: None,
        stop: Some(stop), discarded: true,
    });
    // Old lines (no stop/discarded) still parse with the defaults.
    let old: Event = serde_json::from_str(r#"{"type":"turn_end","data":{"turn_idx":0}}"#).unwrap();
    assert!(matches!(old, Event::TurnEnd { stop: None, discarded: false, .. }));
}

#[test]
fn stop_record_reads_a_serialized_provider_stop() {
    // The exact JSON shape rupu_providers::Stop serializes to.
    let v = json!({"reason": "max_tokens", "wire": {"provider": "openai-codex", "value": "max_output_tokens"}});
    let s: StopRecord = serde_json::from_value(v).unwrap();
    assert_eq!(s.reason, "max_tokens");
    assert_eq!(s.wire["value"], "max_output_tokens");
}

#[test]
fn run_complete_outcome_round_trips_and_defaults() {
    roundtrip(&Event::RunComplete {
        run_id: "r".into(),
        status: rupu_transcript::RunStatus::Error,
        total_tokens: 5, duration_ms: 1,
        error: Some("refused · cyber".into()),
        outcome: Some(record()),
    });
    let old: Event = serde_json::from_str(
        r#"{"type":"run_complete","data":{"run_id":"r","status":"ok","total_tokens":0,"duration_ms":0}}"#,
    ).unwrap();
    assert!(matches!(old, Event::RunComplete { outcome: None, .. }));
}

#[test]
fn unknown_event_keeps_and_re_serializes_its_payload() {
    let line = r#"{"type":"brand_new_event","data":{"x":1,"nested":{"y":[1,2]}}}"#;
    let e: Event = serde_json::from_str(line).unwrap();
    assert_eq!(e, Event::Unknown { tag: "brand_new_event".into(), data: json!({"x":1,"nested":{"y":[1,2]}}) });
    let back = serde_json::to_value(&e).unwrap();
    assert_eq!(back, serde_json::from_str::<serde_json::Value>(line).unwrap());
    // No data key → Null data, still re-serialized without inventing one.
    let e: Event = serde_json::from_str(r#"{"type":"bare_new_event"}"#).unwrap();
    assert_eq!(e, Event::Unknown { tag: "bare_new_event".into(), data: serde_json::Value::Null });
    assert_eq!(serde_json::to_value(&e).unwrap(), json!({"type": "bare_new_event"}));
}

#[test]
fn unknown_recovery_action_is_other() {
    let a: RecoveryAction = serde_json::from_value(json!("teleported")).unwrap();
    assert_eq!(a, RecoveryAction::Other);
}
```

  Add to `tests/it/final_turn_text.rs`:

```rust
#[test]
fn a_continuation_chain_joins_across_turns() {
    use rupu_transcript::{final_turn_text, Event, RecoveryAction};
    let ev = vec![
        Event::TurnStart { turn_idx: 0 },
        Event::AssistantMessage { content: "part one".into(), thinking: None },
        Event::Recovery {
            outcome_id: "oc".into(), rung: 0, action: RecoveryAction::Continued,
            attempt: Some(1), budget: Some(3), provider: None, model: None, reason: None,
            merge_into_previous: false, continues_output: true,
        },
        Event::UserMessage { content: "continue".into() },
        Event::TurnStart { turn_idx: 1 },
        Event::AssistantMessage { content: "part two".into(), thinking: None },
    ];
    assert_eq!(final_turn_text(ev).as_deref(), Some("part one\n\npart two"));
}

#[test]
fn a_discarded_turn_does_not_contribute() {
    use rupu_transcript::{final_turn_text, Event};
    let ev = vec![
        Event::TurnStart { turn_idx: 0 },
        Event::AssistantMessage { content: "kept".into(), thinking: None },
        Event::TurnEnd { turn_idx: 0, tokens_in: None, tokens_out: None, stop_reason: None,
            response_id: None, stop: None, discarded: false },
        Event::TurnStart { turn_idx: 1 },
        Event::AssistantMessage { content: "refused partial".into(), thinking: None },
        Event::TurnEnd { turn_idx: 1, tokens_in: None, tokens_out: None, stop_reason: None,
            response_id: None, stop: None, discarded: true },
    ];
    assert_eq!(final_turn_text(ev).as_deref(), Some("kept"));
}
```

  Run: `cargo test -p rupu-transcript --test it outcome_events:: final_turn_text::`
  Expected: compile errors.

- [ ] **Step 2: Implement `outcome.rs`:**

```rust
//! Typed outcome records shared by every transcript reader (spec
//! 2026-10-01 response-outcomes §7.1). `class` / `reason` stay strings:
//! this crate is a leaf, and a newer writer's class must still parse.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// One classified outcome (a non-normal reply or a provider error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeRecord {
    /// Unique within the run; `Recovery.outcome_id` points here.
    pub id: String,
    /// `refusal`, `safety`, `max_tokens`, `context_window_exceeded`,
    /// `pause_turn`, `malformed_tool_call`, `incomplete`, `empty_reply`,
    /// `unrecognized_stop`, `unreported_stop`, `provider_error`.
    pub class: String,
    pub severity: Severity,
    /// One line, e.g. `refused · cyber`.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For `provider_error`: the normalized `ErrorClass` (snake_case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    /// The provider's own words: the serialized `WireStop` or `ApiErrorBody`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub wire: Value,
}

/// A turn's stop as recorded on `TurnEnd`. Deserializes from a serialized
/// `rupu_providers::Stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRecord {
    pub reason: String,
    #[serde(default)]
    pub wire: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_by: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    Continued,
    Retried,
    Compacted,
    FellBack,
    ServedByFallback,
    Skipped,
    Asked,
    Parked,
    Failed,
    #[serde(other)]
    Other,
}
```

- [ ] **Step 3: Change `event.rs`.**
  - Add `"outcome"` and `"recovery"` to `KNOWN_EVENT_TAGS`.
  - Add the two variants after `Notice`:

```rust
    /// A classified non-normal reply or provider error (spec §7.1).
    Outcome {
        turn_idx: u32,
        outcome: crate::outcome::OutcomeRecord,
    },
    /// One recovery action taken for an outcome (spec §5.2). Rendered in
    /// order after its `Outcome`, the pair reads as a timeline.
    Recovery {
        outcome_id: String,
        /// 0 = in place, 1 = same provider, 2 = other provider, 3 = operator.
        rung: u8,
        action: crate::outcome::RecoveryAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        budget: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Replay appends the NEXT turn's assistant content to the previous
        /// assistant message (a `pause_turn` continuation).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        merge_into_previous: bool,
        /// The next turn's text continues this turn's answer (a truncation
        /// continuation): `final_turn_text` joins across the boundary.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        continues_output: bool,
    },
```

  - In `TurnEnd`, add the following, and update the `stop_reason` doc to say it holds the provider's wire value:
    ```rust
    /// The typed stop (spec §7.1). `None` on transcripts before 2026-10.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    stop: Option<crate::outcome::StopRecord>,
    /// The turn's content was discarded (a refused / blocked / retried
    /// turn): replay drops it, `final_turn_text` ignores it.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    discarded: bool,
    ```
  - In `RunComplete`, add `#[serde(skip_serializing_if = "Option::is_none", default)] outcome: Option<crate::outcome::OutcomeRecord>,`.
  - Replace the `Unknown` unit variant with:

```rust
    /// Forward-compatibility catch-all: a line whose `type` this binary
    /// doesn't know. Keeps the payload so readers can render it and the CP
    /// can pass it through verbatim. Never written by rupu itself.
    #[serde(skip)]
    Unknown { tag: String, data: Value },
```

  - In the custom `Deserialize`, the unknown-tag arm becomes:
    ```rust
    Some(Value::String(tag)) if !KNOWN_EVENT_TAGS.contains(&tag.as_str()) => Ok(Event::Unknown {
        tag: tag.clone(),
        data: obj.get("data").cloned().unwrap_or(Value::Null),
    }),
    ```
  - The custom `Serialize` becomes:

```rust
impl Serialize for Event {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Event::Unknown { tag, data } => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("type", tag)?;
                if !data.is_null() {
                    map.serialize_entry("data", data)?;
                }
                map.end()
            }
            other => Event::serialize(other, serializer),
        }
    }
}
```

  - Update the existing inline tests that assert `Event::Unknown` (around `unrecognized_event_type_parses_as_unknown_not_error`) to the struct form.
  - Fix every compile error the new fields cause:
    - existing constructors of `TurnEnd` need `stop: None, discarded: false`, and `RunComplete` needs `outcome: None`;
    - every `Event::Unknown` pattern becomes `Event::Unknown { .. }`.
  - Find all the sites with:
    ```bash
    grep -rn "Event::TurnEnd {\|Event::RunComplete {\|Event::Unknown" crates --include='*.rs'
    ```
  - For an exhaustive match that the new `Outcome` / `Recovery` variants break, add an arm that keeps the current behavior for unrendered events. In the CLI renderers that means the same thing their `Notice` arm does with an empty message. In replay, `{}`. Task 2 replaces the renderer arms and Task 7 the replay ones.

- [ ] **Step 4: Make `final_turn_text` follow chains and skip discarded turns:**

```rust
pub fn final_turn_text(events: impl IntoIterator<Item = Event>) -> Option<String> {
    let mut saw_turn_start = false;
    let mut turn_fragments: Vec<String> = Vec::new();
    // Fragments of earlier turns this answer continues (a truncation
    // continuation's `Recovery { continues_output }`).
    let mut carried: Vec<String> = Vec::new();
    let mut carry_next = false;
    let mut last_non_empty: Option<String> = None;
    for event in events {
        match event {
            Event::TurnStart { .. } => {
                saw_turn_start = true;
                if carry_next {
                    carried.append(&mut turn_fragments);
                } else {
                    carried.clear();
                    turn_fragments.clear();
                }
                carry_next = false;
            }
            Event::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                if saw_turn_start {
                    turn_fragments.push(content.clone());
                }
                last_non_empty = Some(content);
            }
            Event::TurnEnd { discarded: true, .. } => turn_fragments.clear(),
            Event::Recovery { continues_output: true, .. } => carry_next = true,
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
```

  Keep the existing `final_turn_text` tests green. Note that `last_non_empty` can still be a discarded fragment when no turn kept text. That is acceptable: a run whose only text was refused fails with an outcome, and Task 10's readers show the outcome instead.

- [ ] **Step 5: Run the tests.**

  Run: `cargo test -p rupu-transcript`
  Run: `cargo check --workspace --all-targets`
  Expected: PASS / clean.

- [ ] **Step 6: Format and commit.**
  Commit subject: `feat(transcript): outcome and recovery events, typed turn stop, raw unknown events`.

---

### Task 2: Minimal renderers for the new events (CLI and web)

**Files:**
- Modify (CLI): `crates/rupu-cli/src/cmd/transcript.rs` (`transcript_event_lines`, `render_pretty_transcript_event`), `crates/rupu-cli/src/cmd/session.rs` (`push_transcript_event`, `transcript_event_lines`), `crates/rupu-cli/src/output/workflow_printer.rs` (`workflow_transcript_event_lines`, `process_event`), `crates/rupu-cli/src/cmd/autoflow.rs` (`live_run_event_lines`), `crates/rupu-cli/src/output/live_view/mux.rs` (`project_event`), `crates/rupu-cli/src/cmd/run.rs` (live printer match), `crates/rupu-cli/src/cmd/watch.rs`
- Modify (web): `crates/rupu-cp/web/src/lib/transcript.ts`, `crates/rupu-cp/web/src/components/transcript/transcriptView.ts`, `crates/rupu-cp/web/src/components/transcript/Turn.tsx`
- Test: extend each CLI file's existing "every variant gets a row" guard; `crates/rupu-cp/web/src/components/transcript/transcriptView.test.ts`; `Turn.test.tsx`

**Interfaces:**
- Consumes: Task 1's events.
- Produces: one shared line formatter in `rupu-transcript`, which Plan 3 replaces with the full `present` module:
  ```rust
  // rupu_transcript::outcome
  pub fn outcome_line(o: &OutcomeRecord) -> String;      // "✗ refused · cyber — Declined…"
  pub fn recovery_line(action: RecoveryAction, rung: u8, provider: Option<&str>, model: Option<&str>,
      attempt: Option<u32>, budget: Option<u32>, reason: Option<&str>) -> String; // "↺ rung 1 · fell back to anthropic/claude-opus-4-8"
  ```

**Rendering rules:**
- **Outcome glyph by severity:** `✗` for error, `!` for warning, `·` for info.
- **Outcome text:** `{glyph} {title}`, plus ` — {detail}` when there is a detail.
- **Recovery text:** `↺ rung {rung} · {action phrase}`. The phrases:

  | Action | Phrase |
  |---|---|
  | `continued` | `continued {attempt}/{budget}` |
  | `retried` | `retried` |
  | `compacted` | `compacted` |
  | `fell_back` | `fell back to {provider}/{model}` |
  | `served_by_fallback` | `served by {model}` |
  | `skipped` | `skipped {provider}/{model}: {reason}` |
  | `failed` | `no recovery left` |
  | `asked`, `parked`, other | the action name |

- **CLI tones:** each renderer uses its existing tone for failures (`Failed` / `Danger`) for error outcomes, its warning tone for warnings, and its dim/notice tone for info and recovery lines. `Unknown { tag, data }` renders `unrecognized event · {tag}`, plus compact JSON of `data` truncated to 200 chars.
- **Web:**
  - `lib/transcript.ts` types the new events and the `unknown` data.
  - `transcriptView.ts` adds the block kinds `{ kind: 'outcome'; severity; title; detail }` and `{ kind: 'recovery'; text }`. `text` is computed by a TS port of `recovery_line`, `recoveryLine()` in `lib/outcome.ts` (new). The `unknown` block keeps `data`.
  - `Turn.tsx` renders `outcome` as a left-border block: `border-err bg-err-bg/40` for error, `border-warn bg-warn-bg/40` for warning, `border-info` for info. It shows the title, with the detail under it. `recovery` renders as a `text-meta text-ink-mute` line, and `unknown` shows its type plus `<pre>` JSON.

- [ ] **Step 1: Write the failing tests.**
  - Rust, in `rupu-transcript` (`tests/it/outcome_events.rs`): `outcome_line` for the error, warning and info severities, and `recovery_line` for every action listed above. Assert exact strings.
  - In each CLI file, extend the existing every-variant guard to construct `Event::Outcome`, `Event::Recovery` and `Event::Unknown { .. }`, and assert a non-empty row containing the title, the action phrase, or `unrecognized event · brand_new_event`. The guards are `transcript.rs` `v2_rows_exist_for_every_new_variant_and_netflow`, `session.rs` `transcript_event_lines_v2_rows_exist_for_every_new_variant`, `workflow_printer.rs` `workflow_transcript_event_lines_v2_rows_exist_for_every_new_variant`, and `autoflow.rs` `live_run_event_lines_v2_rows_exist_for_every_new_variant`. In `mux.rs`, a test that `project_event` returns `Some` for both new events.
  - Web vitest, in `transcriptView.test.ts`:
    - an `outcome` event yields `{kind:'outcome', severity:'error', title}`;
    - a `recovery` event yields text `↺ rung 1 · fell back to anthropic/claude-opus-4-8`;
    - an unknown event keeps `data`;
    - extend the "nothing dropped" test with the two new types.
  - In `Turn.test.tsx`, an outcome block renders its title and detail.

  Run:
  ```bash
  cargo test -p rupu-transcript --test it outcome_events::
  cargo test -p rupu-cli --lib v2_rows
  ```
  And in `crates/rupu-cp/web`: `npx vitest run src/components/transcript`.
  Expected: FAIL.

- [ ] **Step 2: Implement** `outcome_line` and `recovery_line` in `rupu-transcript/src/outcome.rs`, the TS port in `web/src/lib/outcome.ts`, and the renderer arms, per the rules above.

- [ ] **Step 3: Run the tests.**

  Run:
  ```bash
  cargo test -p rupu-transcript
  cargo test -p rupu-cli --lib
  ```
  And in `crates/rupu-cp/web`: `npx vitest run` and `npx tsc --noEmit`.
  Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(cli,cp-web): render outcome, recovery and raw unknown transcript events`.

---

### Task 3: Fallback configuration

**Files:**
- Create: `crates/rupu-config/src/recovery_config.rs`
- Modify: `crates/rupu-config/src/config.rs` (`pub recovery: crate::recovery_config::RecoveryConfig` with `#[serde(default)]`), `crates/rupu-config/src/lib.rs` (`pub mod recovery_config; pub use recovery_config::{FallbackEntry, RecoveryConfig};`)
- Modify: `crates/rupu-agent/src/spec.rs` (`fallbacks:` frontmatter → `AgentSpec.fallbacks: Option<Vec<rupu_config::FallbackEntry>>`)
- Test: inline tests in `recovery_config.rs`; `spec.rs` `compaction_config_tests` module (add a test there)

**Interfaces:**
- Produces:
  ```rust
  // rupu_config
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
  pub struct FallbackEntry { #[serde(default)] pub provider: Option<String>, pub model: String }
  pub struct RecoveryConfig { pub fallbacks: Vec<FallbackEntry>, pub server_side_fallback: bool }  // defaults: [], true
  impl RecoveryConfig {
      /// Agent frontmatter wins; else this table's chain; else empty.
      pub fn chain_for(&self, agent: Option<&[FallbackEntry]>) -> Vec<FallbackEntry>;
  }
  /// Splits a chain for a run on `current_provider`: (rung 1 = same provider or unnamed, rung 2 = others), order kept.
  pub fn split_chain(chain: &[FallbackEntry], current_provider: &str) -> (Vec<FallbackEntry>, Vec<FallbackEntry>);
  // rupu_agent::AgentSpec
  pub fallbacks: Option<Vec<rupu_config::FallbackEntry>>
  ```

- [ ] **Step 1: Write the failing tests.**
  - `recovery_config.rs`:
    - a TOML table `[recovery]\nfallbacks = [{ model = "claude-opus-4-8" }, { provider = "openai-codex", model = "gpt-5.6-cyber" }]` parses through `rupu_config::Config`;
    - `server_side_fallback` defaults to `true`, and `server_side_fallback = false` parses;
    - an unknown key in `[recovery]` is an error;
    - `chain_for(Some(&[x]))` returns `[x]` and ignores the table; `chain_for(None)` returns the table's chain;
    - `split_chain(&[unnamed opus, codex/gpt, anthropic/sonnet], "anthropic")` returns `([unnamed opus, anthropic/sonnet], [codex/gpt])`.
  - `spec.rs`:
    - `AgentSpec::parse("---\nname: a\nfallbacks:\n  - model: claude-opus-4-8\n  - provider: openai-codex\n    model: gpt-5.6-cyber\n---\nbody\n")` gives two entries;
    - absent → `None`;
    - an entry with an unknown key → parse error.

  Run:
  ```bash
  cargo test -p rupu-config --lib recovery_config::
  cargo test -p rupu-agent --lib spec::
  ```
  Expected: FAIL.

- [ ] **Step 2: Implement.**
  - Write `RecoveryConfig` with `#[serde(default, deny_unknown_fields)]` and a `Default` impl that sets `server_side_fallback: true`, using `#[serde(default = "RecoveryConfig::default_true")]` on that field, the same pattern as `CpConfig`.
  - Write `chain_for` and `split_chain` (a straightforward partition). A `provider` of `None` belongs to rung 1.
  - For the `AgentSpec` frontmatter, add a `#[serde(default)] fallbacks: Option<Vec<rupu_config::FallbackEntry>>` field to `Frontmatter`, add the `pub` field to `AgentSpec`, and assign it in `parse`.

- [ ] **Step 3: Run the tests.** Expected: PASS. Then `cargo check --workspace --all-targets`, which surfaces any exhaustive `Config { … }` / `AgentSpec { … }` literals to update.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(config,agent): fallbacks frontmatter and [recovery] table`.

---

### Task 4: Anthropic server-side fallback

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs`, `crates/rupu-providers/src/error.rs`
- Modify: `crates/rupu-runtime/src/provider_factory.rs` (`ProviderConfig.anthropic_server_side_fallback: Option<bool>`; `build_anthropic` calls `.with_server_side_fallback(config.anthropic_server_side_fallback.unwrap_or(true))`)
- Modify: the `ProviderConfig { … }` literals the compiler flags (add `anthropic_server_side_fallback: None`); Task 9 sets real values at the launch sites
- Test: the inline tests in `anthropic.rs`; the `error.rs` tests

**Interfaces:**
- Produces:
  ```rust
  impl AnthropicClient { pub fn with_server_side_fallback(self, enabled: bool) -> Self; }
  ProviderError::FallbackUnavailable { message: String }    // class(): InvalidRequest; never retryable
  pub(crate) fn supports_server_side_fallback(model: &str) -> bool;  // after strip_1m, exact set
  ```

**Behavior:**
- **The client fields:** `server_side_fallback: bool` (enabled by the builder, default `false` on the client) and `server_side_fallback_disabled: bool`. Add both to all five full struct literals.
- **The predicate** `sends_server_side_fallback(&self, model)` is true when all four hold:
  - `self.server_side_fallback`
  - `!self.server_side_fallback_disabled`
  - `matches!(self.auth, AuthMethod::ApiKey(_))`
  - `supports_server_side_fallback(model)`
- **Body:** when the predicate holds, `build_request_body` sets `body["fallbacks"] = "default"` (before `apply_cache_breakpoints`).
- **Headers:** on the API-key path, `apply_auth_headers` builds a CSV.
  - The value `context-1m-2025-08-07` is included when `sends_1m_beta`.
  - `server-side-fallback-2026-07-01` is included when the predicate holds.
  - The header is set only when the CSV is non-empty.
  - OAuth is unchanged.
- **Disable on 400:** in both `send` and `stream`, the non-success branch that builds `api_error_from_response` changes. When the status is 400, the request carried fallbacks (the predicate held for `request.model`), and the body text contains `fallbacks`, the client:
  1. sets `server_side_fallback_disabled = true`;
  2. `warn!`s;
  3. returns `ProviderError::FallbackUnavailable { message: <body text> }`.

  Mirror `long_context_refusal`.
- **Echo rule (spec §4.5).** In `restore_reasoning_blocks`, for an assistant message containing at least one `"fallback"` block, drop every `thinking` / `redacted_thinking` / `tool_use` block positioned before the last fallback block. Run this after the fallback rewrite. `text` blocks and everything after the boundary are kept.
- **`ProviderError::FallbackUnavailable`:**
  - Display is `server-side fallback unavailable: {message}`.
  - `class()` is `InvalidRequest`.
  - `tuned::is_retryable` and the runner's `is_retryable_provider_error` both return `false` for it. Add it to the runner's match.

- [ ] **Step 1: Write the failing tests**, in `anthropic.rs` `mod tests`:
  1. `server_side_fallback_body_and_beta_on_api_key_for_supported_models`:
     - Build `AnthropicClient::with_url(key, server.url(...), sink).with_server_side_fallback(true)`.
     - Use an httpmock mock matching `.header("anthropic-beta", "server-side-fallback-2026-07-01")` and `.json_body_partial(r#"{"fallbacks":"default"}"#)`.
     - Assert one hit for `claude-opus-5-5`.
  2. `server_side_fallback_omitted_for_unsupported_model_oauth_and_when_off`: in each of three cases, the body has no `fallbacks` and the header check uses `no_beta_header`:
     - an unsupported model (`claude-haiku-4-5`);
     - an OAuth client (body check only, via `build_request_body`);
     - `with_server_side_fallback(false)`.
  3. `beta_csv_combines_1m_and_fallback`: with `context_window: Some(OneMillion)` on a supported model, the header is `context-1m-2025-08-07,server-side-fallback-2026-07-01`.
  4. `a_400_naming_fallbacks_disables_it_for_the_client`. Model it on `api_key_long_context_refusal_disables_the_1m_beta`, and loop over `streamed in [false, true]`.
     - Mock A: carries the fallback beta → 400 `{"type":"error","error":{"type":"invalid_request_error","message":"fallbacks: not enabled for this organization"}}`.
     - Mock B: no beta → 200 (send) / an SSE body (stream).
     - The first call returns `FallbackUnavailable`. The second call hits B.
  5. `echo_rule_drops_thinking_and_tool_use_before_the_last_fallback`. The assistant blocks are `[Reasoning(anthropic), ToolUse, Text "a", Fallback, Reasoning, Text "b"]`. The body's assistant content is `[text a, fallback, thinking, text b]`.

  In `error.rs`: `FallbackUnavailable` has class `InvalidRequest` and is not retryable in `tuned::is_retryable`.

  Run: `cargo test -p rupu-providers --lib anthropic:: error::`
  Expected: FAIL.

- [ ] **Step 2: Implement** per the behavior above. Also implement the factory field: `provider_factory.rs` `ProviderConfig` gets the field with a doc comment, and `build_anthropic` applies it.

- [ ] **Step 3: Run the tests.**

  Run:
  ```bash
  cargo test -p rupu-providers
  cargo test -p rupu-runtime
  cargo check --workspace --all-targets
  ```
  Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(providers): Anthropic server-side fallbacks on API-key auth, disabled on a refusing 400; fallback echo rule`.

---

### Task 5: Outcome classification (`rupu-agent::outcome`)

**Files:**
- Create: `crates/rupu-agent/src/outcome.rs`
- Modify: `crates/rupu-agent/src/lib.rs` (`pub mod outcome;`)
- Test: the inline `#[cfg(test)] mod tests` in `outcome.rs`

**Interfaces:**
- Consumes: `rupu_providers::{LlmResponse, Stop, StopReason, ProviderError, ContentBlock}` and `reply_error::ErrorClass` (Plan 1); `rupu_transcript::{OutcomeRecord, Severity}` (Task 1).
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum OutcomeClass { PauseTurn, MaxTokens, ContextWindowExceeded, Refusal, Safety,
      MalformedToolCall, Incomplete, EmptyReply, UnrecognizedStop, UnreportedStop,
      ProviderError(rupu_providers::reply_error::ErrorClass) }
  impl OutcomeClass { pub fn as_str(&self) -> &'static str; pub fn severity(&self) -> Severity; }
  #[derive(Debug, Clone)]
  pub struct Outcome { pub id: String, pub class: OutcomeClass, pub title: String,
      pub detail: Option<String>, pub wire: serde_json::Value, pub truncated_tool: bool }
  impl Outcome { pub fn record(&self) -> OutcomeRecord; pub fn severity(&self) -> Severity; }
  pub fn classify_response(resp: &LlmResponse) -> Option<Outcome>;
  pub fn classify_error(e: &rupu_providers::ProviderError) -> Outcome;
  ```

**Classification rules:**
- **`classify_response`:**

  | `stop.reason` | Result |
  |---|---|
  | `EndTurn`, `ToolUse`, `StopSequence` | `None`, unless the reply has no non-empty `Text` and no `ToolUse` block, which is an `EmptyReply` outcome |
  | `PauseTurn` | `PauseTurn` (info) |
  | `MaxTokens` | `MaxTokens` (error); `truncated_tool = stop.wire.details["truncated_tool"].is_some()` |
  | `ContextWindowExceeded` | `ContextWindowExceeded` (error) |
  | `Refusal` | `Refusal` (error) |
  | `Safety` | `Safety` (error) |
  | `MalformedToolCall` | `MalformedToolCall` (error) |
  | `Incomplete` | `Incomplete` (error) |
  | `Unrecognized` | `UnrecognizedStop` (warning) |
  | `Unreported` | `UnreportedStop` (warning) |

  An `UnrecognizedStop` or `UnreportedStop` whose reply has no text and no tool call becomes `EmptyReply` instead.
- **`class.as_str()`:** `pause_turn`, `max_tokens`, `context_window_exceeded`, `refusal`, `safety`, `malformed_tool_call`, `incomplete`, `empty_reply`, `unrecognized_stop`, `unreported_stop`, `provider_error`.
- **Severity:**
  - info for `PauseTurn`;
  - warning for `UnrecognizedStop` / `UnreportedStop`;
  - error for everything else, including every `ProviderError(_)`.
- **Titles** (detail in brackets):
  - `refusal`: `refused · {category}` when `stop.refusal.category` is `Some`, else `refused` [`refusal.explanation`].
  - `safety`: `blocked by a safety filter · {wire.value}` [`details.finishMessage` as string, when present].
  - `max_tokens`: `truncated · output limit` [`truncated tool call {name}` when `truncated_tool`].
  - `context_window_exceeded`: `truncated · context window full`.
  - `pause_turn`: `paused by the provider (server tool loop)`.
  - `malformed_tool_call`: `malformed tool call · {details.malformed_tool.name}` [`details.malformed_tool.error`].
  - `incomplete`: `incomplete reply · {wire.value or "unreported"}`.
  - `empty_reply`: `empty reply`.
  - `unrecognized_stop`: `unrecognized stop reason · {wire.provider} "{wire.value}"`.
  - `unreported_stop`: `no stop reason reported · {wire.provider}`.
  - `provider_error`: `provider error · {error_class}` [the error's Display]. `error_class` is the snake_case serde name of `e.class()`.
- **`wire`:**
  - for a response: `serde_json::to_value(&resp.stop.wire)`;
  - for an `ApiErrorBody` error: `serde_json::to_value(body)`;
  - otherwise `{"message": e.to_string()}`.
- **`id`:** `classify_*` return `id: String::new()`. The runner always sets it from `RecoveryState::next_outcome_id()` (`oc_1`, `oc_2`, …, run-local) before writing the `Outcome` event, and `Recovery.outcome_id` uses the same value.

- [ ] **Step 1: Write the failing tests.** Build each `LlmResponse` with `Stop::from_wire` / `Stop::synthetic` and the content blocks it needs:
  - one test per row of the table;
  - each title format;
  - `EmptyReply` for `EndTurn` with empty content, and for `EndTurn` with only a whitespace `Text`;
  - `ToolUse` with tool blocks → `None`;
  - `classify_error` on `ProviderError::api("anthropic", 529, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)` → `ProviderError(Overloaded)`, title `provider error · overloaded`, `record().error_class == Some("overloaded")`;
  - `record()` maps every field.

  Run: `cargo test -p rupu-agent --lib outcome::`
  Expected: FAIL.

- [ ] **Step 2: Implement** per the rules.

- [ ] **Step 3: Run the tests.** Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(agent): classify every reply and provider error into an outcome`.

---

### Task 6: The recovery ladder (`rupu-agent::recovery`) and `AgentRunOpts.recovery`

**Files:**
- Create: `crates/rupu-agent/src/recovery.rs`
- Modify: `crates/rupu-agent/src/lib.rs` (`pub mod recovery;` and re-export `RecoveryOpts, HopBuilder, Hop`)
- Modify: `crates/rupu-agent/src/runner.rs` (`AgentRunOpts.recovery: crate::recovery::RecoveryOpts`)
- Modify: every `AgentRunOpts { … }` literal in the workspace (about 100; add `recovery: Default::default(),`). Find them with `grep -rn "AgentRunOpts {" crates --include='*.rs'`.
- Test: the inline tests in `recovery.rs`

**Interfaces:**
- Consumes: `OutcomeClass` (Task 5), `rupu_config::{FallbackEntry, split_chain}` (Task 3).
- Produces:
  ```rust
  pub const PAUSE_TURN_BUDGET: u32 = 5; pub const TRUNCATION_BUDGET: u32 = 3;
  pub const MALFORMED_BUDGET: u32 = 2; pub const EMPTY_REPLY_BUDGET: u32 = 1;
  pub const INCOMPLETE_BUDGET: u32 = 1; pub const RAISED_CAP_BUDGET: u32 = 1;
  pub const MAX_RECOVERY_ACTIONS: u32 = 20;
  pub const TRUNCATION_NOTE: &str; pub const EMPTY_REPLY_NOTE: &str;
  pub const MALFORMED_NOTE_PREFIX: &str; pub const RECOVERY_RETRY_NOTE: &str;   // with {title} {provider} {model}
  pub fn malformed_note(name: &str, error: &str) -> String;
  pub fn recovery_retry_note(title: &str, provider: &str, model: &str) -> String;

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Rung0 { ContinuePause, ContinueTruncated, RetryRaisedCap, CompactThenContinue,
      CorrectMalformed, NudgeEmpty, RetryTurn, ExistingErrorPipeline, None }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct Policy { pub rung0: Rung0, pub budget: u32, pub rung1: bool, pub rung2: bool,
      pub discard_partial: bool }
  pub fn policy(class: &OutcomeClass, truncated_tool: bool) -> Policy;

  pub struct Hop { pub provider: Box<dyn rupu_providers::provider::LlmProvider>,
      pub provider_name: String, pub model: String,
      pub limits: rupu_providers::model_limits::ModelLimits }
  #[async_trait::async_trait]
  pub trait HopBuilder: Send + Sync {
      async fn build(&self, provider: &str, model: &str) -> Result<Hop, String>;
  }
  #[derive(Clone, Default)]
  pub struct RecoveryOpts { pub chain: Vec<rupu_config::FallbackEntry>,
      pub hop_builder: Option<std::sync::Arc<dyn HopBuilder>> }

  pub struct RecoveryState { /* private */ }
  impl RecoveryState {
      pub fn new() -> Self;
      /// Count one action; false once MAX_RECOVERY_ACTIONS is reached.
      pub fn take_action(&mut self) -> bool;
      /// Per-turn budget use for `kind`; returns Some(attempt) while under budget.
      pub fn try_budget(&mut self, turn_idx: u32, kind: Rung0, budget: u32) -> Option<u32>;
      /// Next untried hop: rung-1 entries first (if `allow_rung1`), then rung-2 (if `allow_rung2`).
      /// Skips the current provider/model and entries already tried. Marks the returned entry tried.
      pub fn next_hop(&mut self, chain: &[FallbackEntry], current_provider: &str, current_model: &str,
          allow_rung1: bool, allow_rung2: bool) -> Option<(u8, FallbackEntry)>;
      pub fn next_outcome_id(&mut self) -> String;   // "oc_1", "oc_2", … (run-local)
  }
  ```

**Policy table** (spec §5.2; `discard_partial` is true for `Refusal`, `Safety`, `Incomplete`, and `MaxTokens` with a truncated tool):

| Class | Rung 0 | Budget | Rung 1 | Rung 2 |
|---|---|---|---|---|
| `PauseTurn` | `ContinuePause` | 5 | no | no |
| `MaxTokens`, no truncated tool | `ContinueTruncated` | 3 | yes | yes |
| `MaxTokens`, truncated tool | `RetryRaisedCap` | 1 | yes | yes |
| `ContextWindowExceeded` | `CompactThenContinue` | 3 | yes | yes |
| `Refusal`, `Safety` | `None` | 0 | yes | yes |
| `MalformedToolCall` | `CorrectMalformed` | 2 | yes | yes |
| `EmptyReply` | `NudgeEmpty` | 1 | yes | yes |
| `Incomplete` | `RetryTurn` | 1 | yes | yes |
| `UnrecognizedStop`, `UnreportedStop` | `None` | 0 | no | no |
| `ProviderError`: `RateLimited` / `Overloaded` / `Server` / `Timeout` | `ExistingErrorPipeline` | 0 | no | yes |
| `ProviderError`: `Quota` | `None` | 0 | no | yes |
| `ProviderError`: `ContextOverflow` | `ExistingErrorPipeline` | 0 | no | yes |
| `ProviderError`: `NotFound` | `None` | 0 | yes | yes |
| `ProviderError`: `Policy` | `None` | 0 | yes | yes |
| `ProviderError`: `Auth` / `Permission` / `InvalidRequest` / `TooLarge` / `Unrecognized` | `None` | 0 | no | no |

`PauseTurn` falls to `Incomplete` handling once its budget is spent. The runner implements that by reclassifying.

- [ ] **Step 1: Write the failing tests:**
  - one assertion per policy row;
  - `try_budget` returns 1, 2, 3, then `None` for `TRUNCATION_BUDGET`, and a new `turn_idx` resets it;
  - `take_action` is false on the 21st call;
  - `next_hop`:
    - chain `[unnamed opus-4-8, codex/gpt, anthropic/sonnet]` on anthropic/opus-5-5 gives `(1, opus-4-8)`, then `(1, sonnet)`, then `(2, codex/gpt)`, then `None`;
    - with `allow_rung1 = false` it starts at codex;
    - an entry equal to the current provider/model is skipped;
  - `malformed_note("read_file", "EOF")` equals the global-constraints text;
  - `next_outcome_id` counts.

  Run: `cargo test -p rupu-agent --lib recovery::`
  Expected: FAIL.

- [ ] **Step 2: Implement** `recovery.rs`. Add the `recovery` field to `AgentRunOpts`, with the doc comment `Recovery ladder inputs (spec 2026-10-01 §5–§6). Default: no fallback chain and no hop builder — rungs 1 and 2 are unavailable, rung 0 still applies.` Then add `recovery: Default::default(),` to every literal.

- [ ] **Step 3: Run the tests.** Run `cargo test -p rupu-agent --lib recovery::` and `cargo check --workspace --all-targets`. Expected: PASS / clean.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(agent): recovery ladder policy, per-run state and the HopBuilder port`.

---

### Task 7: Runner rung 0, the success rule, turn stops, and replay lockstep

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs`, `crates/rupu-agent/src/replay.rs`
- Test: `crates/rupu-agent/tests/it/runner_outcomes.rs` (new, registered in `tests/it/main.rs`); invert `final_answer_without_tool_calls_terminates_regardless_of_stop_reason` in `runner.rs`

**Interfaces:**
- Consumes: Tasks 1, 5 and 6.
- Produces:
  ```rust
  RunError::Outcome { message: String, outcome: Box<rupu_transcript::OutcomeRecord> }   // Display: "{message}"
  impl RunError { pub fn outcome(&self) -> Option<&rupu_transcript::OutcomeRecord>; }   // Outcome + TerminalStatus
  RunError::TerminalStatus { status, detail, outcome: Option<Box<OutcomeRecord>> }      // field added
  RunResult.outcome: Option<rupu_transcript::OutcomeRecord>                             // field added
  pub struct RunExit { pub result: Result<RunResult, RunError>,
      pub final_limits: ModelLimits, pub messages: Vec<Message> }
  pub async fn run_agent_full(opts: AgentRunOpts) -> RunExit;   // run_agent_with_limits wraps it
  ```

**Turn-loop changes.** These are in `run_agent_inner`. A new `let mut recovery = crate::recovery::RecoveryState::new();` lives next to `compaction_seq`.

1. **Every `TurnEnd`** writes `stop: serde_json::from_value(serde_json::to_value(&resp.stop)?).ok()` and `discarded`. It defaults to `false`; rows 3–6 below set it.
2. **Right after `let resp = …` and the usage accounting**, before `emit_turn_content`, compute `let outcome = crate::outcome::classify_response(&resp);`. If it is `Some(o)`:
   - write `Event::Outcome { turn_idx, outcome: o.record() }`;
   - look up `let p = crate::recovery::policy(&o.class, o.truncated_tool);`.
3. **If `p.discard_partial`:**
   - Do not call `emit_turn_content` and do not push the assistant message.
   - Write `TurnEnd` with `discarded: true`.
   - Rung 0 for these rows:
     - `RetryRaisedCap`: when `output_cap_lowered` was set this turn and `opts.limits.output.tokens > req.max_tokens`, write `Recovery { rung: 0, action: Retried, reason: Some("output cap raised to the model maximum") }`, then retry with `opts.limits.output.tokens`.
     - `RetryTurn`: retry within its budget, writing `Recovery { action: Retried }`.
   - A retry means `turn_idx += 1; continue 'turns` with the same `messages`.
   - Otherwise go to the ladder (step 6).
4. **Otherwise (keep the partial):** emit content and push the assistant message as today. Then:
   - **`ContinuePause`**, under budget:
     - Write `Recovery { rung: 0, action: Continued, attempt, budget, merge_into_previous: true }`.
     - Set a loop flag `merge_next_assistant = true`. The NEXT turn's `messages.push(assistant)` becomes an append onto the last assistant message's content.
     - `continue 'turns` with no user message.
     - Over budget: reclassify as `OutcomeClass::Incomplete` (write a fresh `Outcome`) and use `Incomplete`'s policy. Its partial is not discarded here, because it is already pushed. Go to the ladder.
   - **`ContinueTruncated`**, under budget:
     - The tool calls in this turn (if any) are still dispatched normally. Truncation with complete tool calls is rare. When there were tool calls, the normal tool-result path continues the loop, so write only `Recovery { action: Continued, reason: Some("tool calls ran") }`.
     - With no tool calls: write `Recovery { rung: 0, action: Continued, attempt, budget, continues_output: true }`, call `push_user_turn(&mut messages, TRUNCATION_NOTE)`, write `Event::UserMessage { content: TRUNCATION_NOTE }`, and `continue 'turns`.
   - **`CompactThenContinue`:** call `compact_context` (as the overflow path does). When it ran, write `Recovery { action: Compacted }`. Then act as `ContinueTruncated`.
   - **`CorrectMalformed`**, under budget: push the malformed note as a user turn the same way (`push_user_turn` plus a `UserMessage` event), write `Recovery { action: Continued }`, and `continue 'turns`.
   - **`NudgeEmpty`**, under budget: the reply was empty, so `TurnEnd` writes `discarded: true` and the empty assistant message is NOT pushed. Then push `EMPTY_REPLY_NOTE` the same way, write `Recovery { action: Continued }`, and `continue 'turns`.
   - **`None` with a warning severity** (unrecognized or unreported stop): no recovery. The content rule applies: tool calls continue, no tool calls finish.
   - **Over budget, or a class whose rung 0 is `None`:** go to the ladder (step 6).
   - Every action first calls `recovery.take_action()`. When that returns false, go straight to rung 3 (fail).
5. **Success rule.** The `if !made_tool_calls { break 'turns … Ok }` branch is reached only when the turn's outcome is `None` or a warning. Every error outcome is handled above and never falls through to it.
6. **The ladder, response outcomes.** This task implements rung 3 only. Task 8 adds rungs 1–2 in front of it, through a helper `fn exhausted(...)`:
   - write `Recovery { rung: 3, action: Failed, reason: Some(hint) }`;
   - break with `LoopOutcome::Status(RunStatus::Error, Some(title_with_hint), Some(record))`. `LoopOutcome::Status` gains a third field, `Option<OutcomeRecord>`, which feeds `RunComplete.outcome` and `RunResult.outcome`.
   - The hint is `no recovery left — add fallbacks: to the agent or [recovery].fallbacks to try other models`. When `opts.step_id.is_empty()` (a standalone `rupu run`), it is instead `no recovery left — continue with another model: rupu run {agent} --continue {run_id} --model <model> [--provider <provider>]`.
7. **`FallbackUnavailable`.** In the `CallStep::Err` arm, next to `LongContextUnavailable`, add a once-per-turn arm. It writes `Notice { kind: "server_side_fallback_disabled", message: "server-side fallback refused by the API — disabled for this run" }` and `continue`s.
8. **`RunError::Outcome` and `RunResult.outcome`.** `terminal_error()` carries `outcome` into `TerminalStatus`. `RunError::outcome()` returns it.
9. **Replay** (`replay.rs`):
   - `Event::TurnEnd { discarded: true, .. }` clears the accumulator without flushing: add `fn discard(&mut self)` to `TurnAccum`.
   - `Event::Recovery { merge_into_previous: true, .. }` sets `merge_next = true`. On the next non-discarded `TurnEnd`, flush the assistant blocks by appending them to the last message when it is an assistant message, then reset `merge_next`.
   - `Event::Outcome { .. }` is ignored.
10. **`run_agent_full`.** Change the Ok path to `final_messages: messages.clone()`. After the async block, return `RunExit { result, final_limits: opts.limits.clone(), messages }`. `run_agent_with_limits` becomes `{ let e = run_agent_full(opts).await; (e.result, e.final_limits) }`.

- [ ] **Step 1: Write the failing tests.** All of them go in `crates/rupu-agent/tests/it/runner_outcomes.rs` and use `MockProvider` with `ScriptedTurn::Reply` / `AssistantText`. Each test asserts:
  - the provider requests recorded by the existing request-recording mock (the "Like [`MockProvider`], but stores every received [`LlmRequest`]" struct);
  - the transcript events, read with `JsonlReader`;
  - the `RunResult`.

  The tests:
  1. `truncation_continues_with_a_note_and_joins_the_answer`:
     - Script `MaxTokens` text `"part one"`, then `EndTurn` text `"part two"`.
     - The run is `Ok`, 2 turns.
     - The second request's last message is a user message whose last text is `TRUNCATION_NOTE`.
     - `final_turn_text(events) == "part one\n\npart two"`.
     - The transcript has `Outcome { class: "max_tokens" }` and `Recovery { action: Continued, continues_output: true }`.
  2. `truncation_budget_exhausted_fails_with_an_outcome`:
     - Script four `MaxTokens` replies.
     - The status is `Error`, and `result.outcome.class == "max_tokens"`.
     - `RunComplete.outcome` is set.
     - The last `Recovery` is `rung 3, Failed`.
     - `result.error` contains `truncated · output limit` and `no recovery left`.
  3. `refusal_with_no_chain_fails_and_discards_the_partial`:
     - Script a `Refusal` with text `"partial"`.
     - `TurnEnd.discarded == true`.
     - `final_messages` has no assistant message with `"partial"`.
     - The status is `Error`, outcome `refusal`.
  4. `empty_reply_is_nudged_once`:
     - Script `EndTurn` with empty content, then `EndTurn` `"done"`.
     - The second request's last user message ends with `EMPTY_REPLY_NOTE`.
     - The run is `Ok`.
  5. `malformed_tool_call_gets_a_correction`:
     - Script a `Reply` with `Stop` `MalformedToolCall` and `details.malformed_tool = {name:"read_file", id:"t1", error:"EOF"}`, then `EndTurn` `"ok"`.
     - The correction note text is `malformed_note("read_file","EOF")`.
  6. `pause_turn_merges_into_the_same_assistant_message`:
     - Script `PauseTurn` text `"a"`, then `EndTurn` text `"b"`.
     - The second request ends with the assistant message `[Text a]` and no user message after it.
     - `final_messages` has exactly one assistant message, `[Text a, Text b]`.
  7. `unrecognized_stop_is_a_warning_and_finishes_by_content`:
     - Script `Stop::from_wire(Unrecognized, "anthropic", Some("brand_new"))` with text.
     - The run is `Ok`, and there is an `Outcome` with severity `warning`.
  8. `replay_lockstep_for_every_rung0_action`: for scripts 1, 4, 5 and 6, `rupu_agent::replay::reconstruct_messages(&events)` equals `result.final_messages` (compare their serialized JSON).
  9. In `runner.rs`, rename the old test to `final_answer_cut_off_by_max_tokens_is_continued_not_accepted`. Script `MaxTokens` text, then `EndTurn` text, and assert `Ok` with 2 turns.
  10. `fallback_unavailable_writes_a_notice_and_retries_once`:
      - `FallbackUnavailable` is not an `ApiErrorBody`, so `ScriptedTurn` can't script it, and this plan adds no test-only variant for it.
      - Instead, define a small struct in the test that implements `LlmProvider` and returns `Err(ProviderError::FallbackUnavailable { message: "fallbacks: not enabled".into() })` once, then `Ok` with an `EndTurn` text reply.
      - Assert one `Notice { kind: "server_side_fallback_disabled" }` and an `Ok` run.

  Run:
  ```bash
  cargo test -p rupu-agent --test it runner_outcomes::
  cargo test -p rupu-agent --lib final_answer
  ```
  Expected: FAIL.

- [ ] **Step 2: Implement** the turn-loop changes, the replay changes, and `run_agent_full`. Keep every existing early-exit path (SIGTERM, operator stop, context overflow exhaustion) writing `outcome: None` unless this task specifies otherwise.

- [ ] **Step 3: Run the tests.**

  Run:
  ```bash
  cargo test -p rupu-agent
  cargo test -p rupu-orchestrator
  cargo test -p rupu-cli --test it
  ```
  Expected: PASS. A workflow test that asserted a `MaxTokens` final turn succeeds must now script a following `EndTurn`. Update such tests only to add that turn, never to weaken an assertion.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(agent): rung-0 recovery, outcome and recovery events, the success rule, and replay lockstep`.

---

### Task 8: Runner rungs 1–2 (fallback hops) and provider-error outcomes

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs`
- Test: `crates/rupu-agent/tests/it/runner_outcomes.rs`

**Interfaces:**
- Consumes: `RecoveryOpts.chain` / `hop_builder`, `RecoveryState::next_hop` (Task 6), `Hop`.

**Behavior** (added in front of rung 3's `exhausted`):
1. **Hop selection.** The ladder calls `recovery.next_hop(&opts.recovery.chain, &opts.provider_name, &opts.model, p.rung1, p.rung2)`. For each candidate:
   - `opts.recovery.hop_builder` is `None` → write `Recovery { rung, action: Skipped, provider, model, reason: Some("no hop builder in this context") }`, and try the next candidate.
   - `builder.build(provider_name_of_entry, &entry.model)` is `Err(reason)` → write `Recovery { action: Skipped, reason: Some(reason) }`, and try the next. `provider_name_of_entry` is `entry.provider.clone().unwrap_or(opts.provider_name.clone())`.
   - `Ok(hop)` → swap `opts.provider = hop.provider`, `opts.provider_name`, `opts.model`, `opts.limits = hop.limits`. Then:
     - write `Notice { kind: "model_limits", message: opts.limits.describe(&opts.provider_name, Utc::now()) }`;
     - write `Recovery { rung, action: FellBack, provider, model }`;
     - if `opts.limits.compact_threshold()` is `Some(t)` and `last_turn_input_tokens as u64 > t`, run `compact_context` first (`Recovery { action: Compacted }` when it ran);
     - retry: `turn_idx += 1; continue 'turns` for response outcomes, or rebuild `req` and `continue` the inner call loop for errors.
   - Every hop calls `recovery.take_action()` first.
   - The swap is sticky. Later turns use the hop provider because `opts` now holds it.
2. **Response outcomes.**
   - For `Refusal`/`Safety`/`Incomplete`/truncated-tool, the partial was discarded in Task 7.
   - For `ContinueTruncated`/`CorrectMalformed`/`NudgeEmpty`/`CompactThenContinue` over budget, the content is already pushed. The hop retries the next turn, and the conversation already contains the note from the last rung-0 action, or nothing, which is fine.
3. **Provider errors.** In the `CallStep::Err` arm, at the point where today's code writes `RunComplete { status: Error, error: "provider: …" }` and returns `Err(RunError::Provider(..))` (and likewise the context-overflow exhaustion return), do this instead:
   - `let o = classify_error(&e);`
   - write `Event::Outcome { turn_idx, outcome: o.record() }`;
   - `p = policy(&o.class, false)`;
   - try hops (rung 1 when `p.rung1`, rung 2 when `p.rung2`) as above;
   - on a successful hop, rebuild `req` from the hop (`req.model = opts.model.clone(); req.max_tokens = opts.limits.output.tokens;`), reset `http_retries`, `overflow_compacted`, `output_cap_lowered` and `trim_attempts`, and `continue` the inner loop;
   - when nothing is left, write `Recovery { rung: 3, action: Failed, reason: Some(hint) }` and the same `RunComplete` as today with `outcome: Some(o.record())`. Return `Err(RunError::Outcome { message: <today's exact error string>, outcome: Box::new(o.record()) })`.
   - `Preflight` errors keep their current path, with no outcome: they never involved a provider.
4. **Usage events** after a hop carry the hop's `provider_name` / `model`. This already holds, because they read `opts`.
5. **`ServedBy`.** When `resp.stop.served_by` is `Some`, a server-side fallback served the turn. Write `Recovery { outcome_id: <a fresh outcome id>, rung: 1, action: ServedByFallback, model: Some(served_by.model), … }` after an `Outcome` with class `refusal` and the title `refused · served by {model}`. Severity is `error`, per spec §3.7: errors always show as errors. The turn then proceeds normally, because the reply is the fallback model's answer.

- [ ] **Step 1: Write the failing tests** in `runner_outcomes.rs`. Use a test `HopBuilder` that maps `(provider, model)` to a prebuilt `MockProvider` (with `with_provider_id`) or to `Err("no credentials")`, and records the calls.
  1. `refusal_falls_back_on_the_same_provider_and_sticks`:
     - The primary mock's script has ONE turn, a `Refusal`.
     - The hop `anthropic/claude-opus-4-8` has two: first an `AssistantToolUse` calling `read_file` on a file the test creates, then `EndTurn` `"answer"`.
     - The run is `Ok`.
     - The transcript has `Recovery { rung: 1, action: FellBack, model: "claude-opus-4-8" }`.
     - The run completing at all proves stickiness: the second turn went to the hop provider, because a second call to the one-turn primary would fail with "mock script exhausted".
  2. `skipped_hop_then_cross_provider_hop`:
     - The chain is `[unnamed missing-model, openai-codex/gpt-test]`. The builder returns `Err("no credentials")` for the first.
     - Expect `Recovery { action: Skipped, reason: "no credentials" }`, then `Recovery { rung: 2, action: FellBack }`, then `Ok`.
  3. `overloaded_after_retries_goes_cross_provider`:
     - Primary: 11 × `ReplyError` overloaded. That is `MAX_HTTP_RETRIES = 10` retries; the test runs with paused tokio time via `#[tokio::test(start_paused = true)]` so the backoff sleeps don't take real time.
     - A codex hop answers `EndTurn`.
     - The run is `Ok`. There is an `Outcome { class: "provider_error", error_class: "overloaded" }` and a rung-2 `FellBack`.
  4. `auth_error_does_not_hop`:
     - Primary: `ReplyError` with a 401 `authentication_error`. The chain has entries.
     - Expect `Err(RunError::Outcome { .. })` with `outcome().error_class == Some("auth")`, no `FellBack`, and `Recovery { rung: 3, Failed }`.
  5. `served_by_fallback_is_recorded_and_the_answer_kept`:
     - Script a `Reply` with `stop.served_by = Some(ServedBy { model: "claude-opus-4-8", hops: [..] })` and `EndTurn` text.
     - The run is `Ok`, the answer is kept, and there is a `Recovery { action: ServedByFallback }`.
  6. `max_recovery_actions_caps_the_ladder`: a chain of 25 entries that all fail to build ends at rung 3 after the cap, not after 25 skips. Skips count as actions.

  Run: `cargo test -p rupu-agent --test it runner_outcomes::`
  Expected: FAIL.

- [ ] **Step 2: Implement** per the behavior above.

- [ ] **Step 3: Run the tests.** Run `cargo test -p rupu-agent` and `cargo test -p rupu-agent --test it runner_model_limits::`. The model-limits ordering must be unchanged: LongContext → output cap → overflow → retry → ladder. Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(agent): fallback hops on the same and other providers; provider errors climb the ladder`.

---

### Task 9: Compaction summariser stop check

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (`compact_messages`)
- Test: inline runner tests next to `compact_messages_returns_outcome_with_mock_provider`

**Behavior:**
- After `provider.send(&summary_req)`, the summary is rejected when either:
  - `crate::outcome::classify_response(&summary_resp)` is `Some(o)` with `o.severity() == Severity::Error`;
  - the joined summary text is empty after `trim`.
- On rejection, return `Err(ProviderError::Other(anyhow!("compaction summary rejected: {title}")))`. Use `empty summary` as the title when the text is empty. The callers (`compact_context`, session compact) already treat `Err` as a failed compaction and fall back.

- [ ] **Step 1: Write the failing tests:**
  - a refused summary (`Reply` with `Refusal`) gives `Err` whose message contains `compaction summary rejected: refused`;
  - a `MaxTokens` summary gives `Err`;
  - an empty `EndTurn` summary gives `Err` mentioning `empty summary`;
  - a normal summary still gives `Ok(Some(..))`.

  Run: `cargo test -p rupu-agent --lib compact_messages`
  Expected: FAIL.

- [ ] **Step 2: Implement.**

- [ ] **Step 3: Run the tests.** Run `cargo test -p rupu-agent`. Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `fix(agent): never replace history with a refused, truncated or empty summary`.

---

### Task 10: `RuntimeHopBuilder` and launch-site wiring

**Files:**
- Create: `crates/rupu-runtime/src/hop_builder.rs`. Export it from `crates/rupu-runtime/src/lib.rs` (`pub mod hop_builder;`).
- Modify: `crates/rupu-cli/src/cmd/run.rs`, `crates/rupu-cli/src/cmd/session.rs`, `crates/rupu-cli/src/cmd/dispatch.rs`, `crates/rupu-orchestrator/src/step_factory.rs` (struct fields plus its constructors in `crates/rupu-cli/src/cmd/workflow.rs` and `crates/rupu-cli/src/resume.rs`)
- Modify: every runtime `ProviderConfig { … }` built at these launch sites, to set `anthropic_server_side_fallback: Some(cfg.recovery.server_side_fallback)`
- Test: `crates/rupu-runtime/tests/it/hop_builder.rs` (new, registered); `crates/rupu-cli/tests/serial/cli_run_fallback.rs` (new, registered in `tests/serial/main.rs`, holding `ENV_LOCK`)

**Interfaces:**
- Produces:
  ```rust
  pub struct RuntimeHopBuilder {
      pub resolver: std::sync::Arc<dyn rupu_auth::CredentialResolver>,
      pub providers: std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
      pub limits_ctx: crate::model_limits::LimitsContext,
      pub sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
      pub server_side_fallback: bool,
  }
  #[async_trait] impl rupu_agent::recovery::HopBuilder for RuntimeHopBuilder {
      async fn build(&self, provider: &str, model: &str) -> Result<rupu_agent::recovery::Hop, String>;
  }
  ```

**Behavior:**
- **`build`:**
  - `!is_dispatchable_provider(provider, &self.providers)` → `Err(format!("provider {provider} is not configured"))`.
  - Otherwise compute `let mut cfg = provider_config_for(provider, &self.providers); cfg.anthropic_server_side_fallback = Some(self.server_side_fallback);`, then call `build_for_provider_with_config(provider, model, None, self.resolver.as_ref(), &cfg, self.sink.clone())`.
  - A `FactoryError` becomes `Err(e.to_string())`. For a missing credential, that reads like `missing credential for openai-codex: …`.
  - Then `let limits = model_limits::resolve(LimitOverrides::default(), provider, model, provider_box.as_mut(), &self.limits_ctx).await;`.
  - Return `Ok(Hop { provider: provider_box, provider_name: provider.into(), model: model.into(), limits })`.
- **Launch sites:**
  - Each site sets `opts.recovery = RecoveryOpts { chain: cfg.recovery.chain_for(spec.fallbacks.as_deref()), hop_builder: Some(Arc::new(RuntimeHopBuilder { … })) }`, using the same resolver, providers map, limits context and netflow sink the site already uses for its primary provider.
  - For sessions, the chain comes from the session's agent spec at turn time. Re-load the agent the way `run_turn` already resolves config. If the session record keeps no spec, use `cfg.recovery.chain_for(None)`, and say so in a doc comment.
  - For the step factory, add `recovery: rupu_config::RecoveryConfig` to `DefaultStepFactory`, set it at its constructors from the loaded config, and set `opts.recovery` in `build_opts_for_step`.

- [ ] **Step 1: Write the failing tests.**
  - `hop_builder.rs`, holding `ENV_LOCK`:
    - an unconfigured provider → `Err` containing `not configured`;
    - with `RUPU_MOCK_PROVIDER_SCRIPT` set to a one-turn script → `Ok(hop)` whose `provider_name` and `model` match the request, and whose limits are present.
  - `cli_run_fallback.rs`:
    - Run `rupu run` with a mock script whose first turn is a `Refusal` `Reply`, against an agent with `fallbacks: [{ model: mock-2 }]`.
    - The mock factory serves every provider from the same script queue, so the second turn (the hop) answers `EndTurn`.
    - Assert exit code 0, and that the transcript holds `Recovery` `fell_back` with model `mock-2`.

  Run:
  ```bash
  cargo test -p rupu-runtime --test it hop_builder::
  cargo test -p rupu-cli --test serial cli_run_fallback:: < /dev/null
  ```
  Expected: FAIL.

- [ ] **Step 2: Implement** the builder and the wiring.

- [ ] **Step 3: Run the tests.**

  Run:
  ```bash
  cargo test -p rupu-runtime
  cargo test -p rupu-orchestrator
  cargo test -p rupu-cli --test it
  cargo test -p rupu-cli --test serial < /dev/null
  ```
  Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(runtime,cli,orchestrator): build fallback hops from config and wire the chain at every launch site`.

---

### Task 11: Orchestrator cause propagation

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/event.rs`, `crates/rupu-orchestrator/src/runs.rs`, `crates/rupu-orchestrator/src/runner.rs`
- Modify: the consumers the compiler flags (`crates/rupu-cli/src/output/run_model.rs`, `crates/rupu-cli/src/output/live_run.rs`, `crates/rupu-cp/src/api/graph.rs`, `crates/rupu-cp/src/api/events.rs`, `crates/rupu-cli/src/cmd/dispatch.rs`) — add `..` / the new field only
- Test: inline tests in `executor/event.rs` and `runs.rs`; `crates/rupu-orchestrator/tests/it/outcome_cause.rs` (new, registered)

**Interfaces:**
- Produces:
  - `cause: Option<rupu_transcript::OutcomeRecord>` (serde default/skip) on:
    - the executor events `StepFailed`, `UnitCompleted`, `DispatchCompleted`;
    - the records `RunRecord`, `StepResultRecord`, `ItemResultRecord`, `UnitCheckpoint`;
    - the runtime structs `StepResult` and `ItemResult`.
  - `error: Option<String>` on `StepResultRecord`, `ItemResultRecord`, `UnitCheckpoint`, `StepResult`, `ItemResult`.
  - `FanoutItemOutcome` / `ParallelSubOutcome`: the `error` fields lose `#[allow(dead_code)]`, gain a `cause` field, and flow into `ItemResult`.
  - The executor `Event` gets `#[serde(other)] Unknown` (unit). `Event::run_id()` returns `""` for it.

**Behavior:**
- **Where `cause` and `error` are set:**
  - `StepFailed` emissions: `cause: e.outcome().cloned()` when `e` is a `RunWorkflowError::Agent { source, .. }` (add `impl RunWorkflowError { pub fn outcome(&self) -> Option<&OutcomeRecord> }` delegating to `source.outcome()`), else `None`.
  - The `RunEnd::Failed` path sets `RunRecord.cause` the same way.
  - Linear step with `continue_on_error`: `StepResult.error = Some(source.to_string())`, `StepResult.cause = source.outcome().cloned()`.
  - Fan-out / parallel units: `ItemResult.error` from the outcome's error string, and `cause` from `raw_error.as_ref().and_then(RunError::outcome)`. `UnitCompleted.cause` and `UnitCheckpoint.cause`/`error` likewise.
  - `DispatchCompleted.cause` comes from the child run's `RunError::outcome()` / `RunResult.outcome`, in `crates/rupu-cli/src/cmd/dispatch.rs`.
- **Conversions:** every `From` conversion between `StepResult` ↔ `StepResultRecord` and `ItemResult` ↔ `ItemResultRecord` carries both fields.

- [ ] **Step 1: Write the failing tests:**
  1. In `event.rs`, invert `unknown_event_type_errors` → `unknown_event_type_is_skipped_as_unknown`. Assert `Event::Unknown` and `run_id() == ""`.
  2. In `event.rs`, `StepFailed` with `cause` round-trips, and an old line without `cause` parses.
  3. In `runs.rs`, `StepResultRecord` / `UnitCheckpoint` with `error` + `cause` round-trip, and old JSON parses.
  4. In `outcome_cause.rs`:
     - A linear workflow whose agent's mock script refuses with no chain gives `RunFailed`; `RunRecord.cause.class == "refusal"`; the emitted `StepFailed.cause` is set.
     - With `continue_on_error: true`, the step's persisted `StepResultRecord.error`/`cause` are set and the run completes.
     - A `for_each` with 2 units, where one refuses under `continue_on_error`, gives `ItemResultRecord.cause` for that unit only.

  Run:
  ```bash
  cargo test -p rupu-orchestrator --lib executor::event:: runs::
  cargo test -p rupu-orchestrator --test it outcome_cause::
  ```
  Expected: FAIL.

- [ ] **Step 2: Implement.**

- [ ] **Step 3: Run the tests.** Run `cargo test -p rupu-orchestrator`, `cargo test -p rupu-cp` and `cargo test -p rupu-cli --test it`. Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(orchestrator): carry a typed outcome cause on step, unit, dispatch and run failures`.

---

### Task 12: Session history on failure and `rupu run --continue --model/--provider`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs` (`run_turn`)
- Modify: `crates/rupu-cli/src/cmd/run.rs` (`--model`, `--provider`, and the `--continue` failed-run path)
- Modify: `crates/rupu-agent/src/continuation.rs` (`apply_continuation_with`, `prepare_recovery_continuation`)
- Test: inline tests in `continuation.rs`; `crates/rupu-cli/tests/serial/cli_run_continue_model.rs` (new, registered); `crates/rupu-cli/tests/serial/cli_session_failed_turn.rs` (new, registered), or an existing session serial test module if one fits

**Interfaces:**
- Produces:
  ```rust
  pub fn apply_continuation_with(opts: &mut AgentRunOpts, messages: Vec<Message>, seed_source: PathBuf, note: String);
  /// Like prepare_continuation, but a run that ended in error WITH a RunComplete.outcome is Resume
  /// (its conversation is intact; a provider-side failure). Every other classification is unchanged.
  pub fn prepare_recovery_continuation(transcript: &Path) -> Result<Continuation, ContinuationError>;
  ```
  - `rupu run` gains `--model <MODEL>` and `--provider <PROVIDER>`. Both override the agent spec and config for any run, and are allowed with `--continue`.

**Behavior:**
- **Session.**
  - `run_turn` calls `rupu_agent::run_agent_full(opts)`.
  - On `Err`, it sets `session.message_history = exit.messages` and `history_source_transcript = Some(transcript_path)`, so the user prompt and any completed tool work survive.
  - `last_error`:
    - on `Ok` with a non-`Ok` status: `result.error` (falling back to today's text);
    - on `Err`: `err.outcome().map(|o| o.title.clone())` with the error string appended as `title — error`, else the error string.
  - The token and turn totals are unchanged on `Err`; nothing measures them there today.
- **`rupu run --continue <id>`:**
  - It calls `prepare_recovery_continuation` when `--model` or `--provider` is given, and `prepare_continuation` otherwise. That keeps the default behavior unchanged.
  - For a `Resume` that came from a failed run, the opts get `apply_continuation_with(opts, messages, seed_source, recovery_retry_note(&outcome_title, &provider_name, &model))`, using the failed run's `RunComplete.outcome.title`.
  - A `Resume` from an interrupted run keeps using `apply_continuation`.
- **The `--model` / `--provider` override:** `args.provider.as_deref().or(spec.provider.as_deref())` feeds `resolve_provider_name`, and the same pattern feeds `resolve_model`. Both happen before the dispatchable pre-flight.

- [ ] **Step 1: Write the failing tests.**
  - In `continuation.rs`:
    - a transcript ending `RunComplete { status: error, outcome: Some(refusal) }`: `prepare_continuation` gives `Failed` and `prepare_recovery_continuation` gives `Resume`;
    - a transcript ending in error with no outcome: both give `Failed`;
    - `apply_continuation_with` sets the given note as `user_message`, `initial_messages` and `seed_source`.
  - `cli_run_continue_model.rs`:
    - Run 1 refuses with no chain (mock script) and exits non-zero. Its stderr names `--continue <id> --model`.
    - Run 2 is `rupu run <agent> --continue <id> --model mock-2` with a mock `EndTurn`, and exits 0.
    - The new transcript's `UserMessage` contains `A previous attempt at this task stopped (refused`. Its `Seed.source_transcript` names run 1's transcript.
  - Session: a failed turn (mock refusal, no chain) leaves `message_history` containing the turn's user prompt, and `last_error` starts with `refused`.

  Run:
  ```bash
  cargo test -p rupu-agent --lib continuation::
  cargo test -p rupu-cli --test serial cli_run_continue_model:: cli_session_failed_turn:: < /dev/null
  ```
  Expected: FAIL.

- [ ] **Step 2: Implement.**

- [ ] **Step 3: Run the tests.** Run `cargo test -p rupu-agent` and `cargo test -p rupu-cli --test serial < /dev/null`. Expected: PASS.

- [ ] **Step 4: Format and commit.**
  Commit subject: `feat(cli,agent): keep a failed session turn's history; rupu run --continue a failed run on another model`.

---

### Task 13: Docs and whole-workspace verification

**Files:**
- Create: `docs/response-outcomes.md`
- Modify: `CLAUDE.md`. Extend the `rupu-providers` entry with server-side fallbacks. Add a sentence to the `rupu-agent` entry on `outcome` / `recovery`, `RecoveryOpts` / `HopBuilder` and `run_agent_full`; to `rupu-transcript`, via the `rupu-cli`-adjacent list, on the `Outcome`/`Recovery` events and raw `Unknown`; and to `rupu-orchestrator` on `cause`.
- Modify: the agent-file reference doc that documents frontmatter keys (find it with `grep -rln "findingsProfile" docs`), adding `fallbacks:`. Modify the config reference that documents tables (find it with `grep -rln "\[cp\]" docs`), adding `[recovery]`.

**`docs/response-outcomes.md` contents** (written for an operator):
1. **What an outcome is.** The classes, a severity table, and how they appear in a transcript (`Outcome` + `Recovery` lines).
2. **The ladder.** The spec §5.2 table as shipped, with the budgets.
3. **Configuring fallbacks.** The `fallbacks:` frontmatter, `[recovery].fallbacks`, precedence, the rung-1/rung-2 split, and the note that cross-provider fallback sends the conversation to that vendor.
4. **Server-side fallback.** The supported models, API key only, how to turn it off, and what the disable notice means.
5. **When nothing is left.** The failure message and `rupu run --continue <id> --model …`.
6. **Where it shows.** The CLI lines and the CP transcript blocks. Note that the fuller rendering is coming.

- [ ] **Step 1: Full verification.** CI is a release gate, so this is the merge check.

  Run:
  ```bash
  cargo clippy --workspace --all-targets -- -D warnings -A clippy::question_mark
  cargo test -p rupu-transcript
  cargo test -p rupu-config
  cargo test -p rupu-providers
  cargo test -p rupu-agent
  cargo test -p rupu-runtime
  cargo test -p rupu-orchestrator
  cargo test -p rupu-cp
  cargo test -p rupu-coverage
  cargo test -p rupu-cli --lib
  cargo test -p rupu-cli --test it
  cargo test -p rupu-cli --test serial < /dev/null
  ```
  Then, in `crates/rupu-cp/web`: `npx vitest run` and `npx tsc --noEmit`.

  Expected: all green. Fix anything red that this branch caused.

- [ ] **Step 2: Grep for leftovers.**
  - `grep -rn "Event::Unknown\b[^ {]" crates --include='*.rs'` should print nothing.
  - `grep -rn "regardless_of_stop_reason" crates` should print nothing.

- [ ] **Step 3: Commit.**
  Commit subject: `docs: response outcomes for operators; CLAUDE.md entries for outcome, recovery and cause`.

---

## Out of scope for this plan

- Rung 3 as a real decision, with `RecoveryDecider`, `Interactive` / `Park`, recovery gates, `rupu workflow recover`, the CP decision endpoint and `park_timeout_secs`. That is Plan 4.
- The full presentation layer (`rupu_transcript::present`, `lib/outcome.ts` tones and chips), the CP DTO `cause` columns, the awaiting-recovery card, run-graph cause tooltips, and the rendering bugs listed in spec §1. That is Plan 3. This plan's renderers are the minimal one-line form.
- Remote placed units: their outcome cause stays `None` on the coordinator until the host-side unit outcome carries it (a follow-up).
