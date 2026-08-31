# Transcript Fidelity Plan 1 — Recording & Replay (schema v2, runner emission, reconstruction, CLI renderers)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the transcript JSONL a complete, ordered, replayable record — first-class thinking events (with provider `raw`), user prompt + seed, compaction/notice events, live thinking deltas — with a verified `reconstruct_messages` round-trip, and every event kind rendered by the CLI views.

**Architecture:** Additive schema v2 on `rupu_transcript::Event` (adjacently tagged, legacy lines keep parsing); `run_agent` emits per-block in provider order instead of collapsing reasoning; new `rupu-agent::replay` module inverts the emission contract; CLI renderers gain arms for every variant (compiler-enforced exhaustiveness finds all sites).

**Tech Stack:** Rust (workspace, tokio, serde, thiserror), Swift/TS untouched (Plans 2–3).

**Spec:** `docs/superpowers/specs/2026-08-31-rupu-transcript-fidelity-design.md`

## Global Constraints

- All work on branch `feat/transcript-fidelity-plan-1`, merged via PR — never commit to `main` (matt's standing rule).
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden; workspace deps only (versions pinned in root `Cargo.toml`).
- NEVER run package-wide `cargo fmt` — main is fmt-dirty under the pinned toolchain. Format only the files you touched: `rustfmt --edition 2021 <file>` (or leave formatting matching surrounding code).
- Schema changes must be additive: every legacy JSONL line must still parse; new fields are `Option` + `#[serde(default)]` + `skip_serializing_if`.
- `Event` derives `PartialEq, Eq` — keep new variants compatible (serde_json `Value` is `Eq`; fine).
- Run `cargo test -p rupu-transcript -p rupu-agent -p rupu-cli -p rupu-orchestrator` and `cargo clippy --workspace --all-targets` before every commit claim.
- `make macos-fixtures` + `cargo test -p rupu-cp` at the end — fixture drift for the macOS app fails rupu-cp tests otherwise.

---

### Task 1: Schema v2 — new Event variants + additive fields

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs`
- Modify (compile-fix stubs only, upgraded in Task 4): `crates/rupu-cli/src/cmd/transcript.rs`, `crates/rupu-cli/src/cmd/session.rs`, `crates/rupu-cli/src/output/workflow_printer.rs`, plus any other match site `cargo build --workspace` flags (e.g. `crates/rupu-cli/src/cmd/autoflow.rs`)

**Interfaces:**
- Produces (later tasks + Plans 2–3 rely on these exact wire shapes):
  - `Event::Thinking { text: Option<String>, provider: String, model: String, raw: serde_json::Value }` → `{"type":"thinking","data":{...}}`
  - `Event::ThinkingDelta { content: String }` → `"thinking_delta"`
  - `Event::UserMessage { content: String }` → `"user_message"`
  - `Event::Seed { message_count: u32, sha256: String, source_transcript: Option<String>, messages: Option<serde_json::Value> }` → `"seed"` — exactly one of `source_transcript` (dedup reference, spec §3 seed dedup rule) / `messages` (inline fallback) is `Some`
  - `Event::Compaction { seq: u32, summarized_messages: u32, backup_path: String, messages: serde_json::Value }` → `"compaction"`
  - `Event::Notice { kind: String, message: String }` → `"notice"` (kinds this plan writes: `"context_trim"`, `"provider_retry"`)
  - `Event::Unknown` — unit variant, `#[serde(other)]`, never written
  - `RunStart` gains `schema: Option<u32>`, `system_prompt: Option<String>`
  - `TurnEnd` gains `stop_reason: Option<String>`, `response_id: Option<String>`

- [ ] **Step 1: Write the failing tests** — append to `crates/rupu-transcript/src/event.rs` `mod tests`:

```rust
#[test]
fn thinking_event_roundtrips_with_raw_payload() {
    let e = Event::Thinking {
        text: Some("weighing options".into()),
        provider: "anthropic".into(),
        model: "claude-sonnet-5".into(),
        raw: serde_json::json!({"type":"thinking","thinking":"weighing options","signature":"sig-x"}),
    };
    let s = serde_json::to_string(&e).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["type"], "thinking");
    assert_eq!(v["data"]["raw"]["signature"], "sig-x");
    assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);

    // Redacted: text omitted from JSON entirely, parses back as None.
    let redacted = Event::Thinking {
        text: None,
        provider: "anthropic".into(),
        model: "claude-sonnet-5".into(),
        raw: serde_json::json!({"type":"redacted_thinking","data":"opaque"}),
    };
    let s2 = serde_json::to_string(&redacted).unwrap();
    assert!(!s2.contains("\"text\""));
    assert_eq!(serde_json::from_str::<Event>(&s2).unwrap(), redacted);
}

#[test]
fn new_v2_variants_roundtrip() {
    for (e, tag) in [
        (Event::ThinkingDelta { content: "chunk".into() }, "thinking_delta"),
        (Event::UserMessage { content: "do the task".into() }, "user_message"),
        (
            Event::Seed {
                message_count: 3,
                sha256: "ab".repeat(32),
                source_transcript: None,
                messages: Some(serde_json::json!([{"role":"user","content":[]}])),
            },
            "seed",
        ),
        (
            Event::Seed {
                message_count: 7,
                sha256: "cd".repeat(32),
                source_transcript: Some("/w/.rupu/transcripts/run_prev.jsonl".into()),
                messages: None,
            },
            "seed",
        ),
        (
            Event::Compaction {
                seq: 1,
                summarized_messages: 12,
                backup_path: "/tmp/b.json".into(),
                messages: serde_json::json!([]),
            },
            "compaction",
        ),
        (Event::Notice { kind: "context_trim".into(), message: "trimmed".into() }, "notice"),
    ] {
        let s = serde_json::to_string(&e).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], tag);
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }
}

#[test]
fn unrecognized_event_type_parses_as_unknown_not_error() {
    let line = r#"{"type":"hologram_projection","data":{"x":1}}"#;
    assert_eq!(serde_json::from_str::<Event>(line).unwrap(), Event::Unknown);
}

#[test]
fn run_start_and_turn_end_additive_fields_are_optional_and_roundtrip() {
    // Legacy line without the new fields still parses.
    let legacy = r#"{"type":"turn_end","data":{"turn_idx":0,"tokens_in":1,"tokens_out":2}}"#;
    assert!(matches!(
        serde_json::from_str::<Event>(legacy).unwrap(),
        Event::TurnEnd { stop_reason: None, response_id: None, .. }
    ));
    let e = Event::TurnEnd {
        turn_idx: 4,
        tokens_in: Some(10),
        tokens_out: Some(20),
        stop_reason: Some("tool_use".into()),
        response_id: Some("msg_01".into()),
    };
    let s = serde_json::to_string(&e).unwrap();
    assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);

    let legacy_start = r#"{"type":"run_start","data":{"run_id":"r","workspace_id":"w","agent":"a","provider":"p","model":"m","started_at":"2026-08-31T00:00:00Z","mode":"bypass"}}"#;
    assert!(matches!(
        serde_json::from_str::<Event>(legacy_start).unwrap(),
        Event::RunStart { schema: None, system_prompt: None, .. }
    ));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-transcript` → expected: compile errors (variants/fields don't exist).

- [ ] **Step 3: Implement in `event.rs`** — add to the `Event` enum (keep existing variants untouched):

```rust
    /// One model reasoning block, in its true position within the turn's
    /// block order. `raw` is the producing provider's byte-exact block
    /// (signature payloads included) — persisted for replay, never for
    /// display. `text: None` means redacted / display-omitted reasoning:
    /// the block existed, its content is not human-readable.
    Thinking {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        text: Option<String>,
        provider: String,
        model: String,
        raw: Value,
    },
    /// A streamed chunk of reasoning text — the thinking counterpart of
    /// `AssistantDelta`. After-the-fact renderers drop it (the committed
    /// `Thinking` event carries the whole block); live tails may stream it.
    ThinkingDelta {
        content: String,
    },
    /// The user turn appended for this run (`opts.user_message`). Absent on
    /// seed-only resumes.
    UserMessage {
        content: String,
    },
    /// The pre-existing conversation this run was seeded with
    /// (`opts.initial_messages`, e.g. a session's history or an orchestrator
    /// pause/resume seed). Stored ONCE, not re-embedded per turn (spec §3
    /// seed dedup rule): `source_transcript` names a transcript whose
    /// replay-reconstruction equals the seed byte-exact (chains resolve
    /// recursively); `messages` is the full inline `Vec<Message>` fallback
    /// (reasoning `raw` intact) when no caller vouches for a source.
    /// Exactly one of the two is `Some`. `sha256` is over the canonical
    /// seed JSON either way, so replay verifies the chain instead of
    /// trusting it.
    Seed {
        message_count: u32,
        sha256: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        source_transcript: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        messages: Option<Value>,
    },
    /// Context compaction rewrote the conversation. `messages` is the full
    /// post-compaction `Vec<Message>` — the only way a reader can know the
    /// conversation state the following turns were actually built from.
    Compaction {
        seq: u32,
        summarized_messages: u32,
        backup_path: String,
        messages: Value,
    },
    /// A runtime intervention worth showing but not part of the
    /// conversation: `kind` ∈ {"context_trim", "provider_retry"} today.
    Notice {
        kind: String,
        message: String,
    },
    /// Forward-compatibility catch-all (mirrors
    /// `rupu_providers::ContentBlock::Unknown`): an unrecognized `type` tag
    /// lands here instead of failing the line. Never written.
    #[serde(other)]
    Unknown,
```

And the additive fields:

```rust
    RunStart {
        run_id: String,
        workspace_id: String,
        agent: String,
        provider: String,
        model: String,
        started_at: DateTime<Utc>,
        mode: RunMode,
        /// Transcript schema version. `None` = v1 (pre-2026-08-31 writer).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        schema: Option<u32>,
        /// The effective system prompt actually sent (post coverage-section
        /// append). `None` on legacy transcripts.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        system_prompt: Option<String>,
    },
```

```rust
    TurnEnd {
        turn_idx: u32,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        tokens_in: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        tokens_out: Option<u64>,
        /// Provider-reported stop reason for the turn (snake_case of
        /// `rupu_providers::StopReason`), when known.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        stop_reason: Option<String>,
        /// Provider response id for the turn, when non-empty.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        response_id: Option<String>,
    },
```

- [ ] **Step 4: Fix every exhaustive-match compile error with temporary no-render arms.** `cargo build --workspace` enumerates the sites (known: `crates/rupu-cli/src/cmd/transcript.rs` `transcript_event_lines`, the RunStart/TurnEnd destructuring sites there, `crates/rupu-cli/src/cmd/session.rs`, `crates/rupu-cli/src/output/workflow_printer.rs`, likely `crates/rupu-cli/src/cmd/autoflow.rs`). At each renderer match add, for now:

```rust
        TranscriptEvent::Thinking { .. }
        | TranscriptEvent::ThinkingDelta { .. }
        | TranscriptEvent::UserMessage { .. }
        | TranscriptEvent::Seed { .. }
        | TranscriptEvent::Compaction { .. }
        | TranscriptEvent::Notice { .. }
        | TranscriptEvent::Unknown => Vec::new(), // upgraded to real rows in Task 4
```

(Adapt the `Vec::new()` to each site's return shape; at struct-destructuring sites just add `..` or the new fields as `_`.) These stubs exist ONLY so Task 1 lands green; Task 4 in this same PR replaces them — never merge the branch with them still in place.

- [ ] **Step 5: Run** — `cargo test -p rupu-transcript && cargo build --workspace` → PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-transcript/src/event.rs crates/rupu-cli crates/rupu-orchestrator
git commit -m "feat(transcript): schema v2 — thinking/seed/user/compaction/notice events, additive RunStart/TurnEnd fields"
```

---

### Task 2: Runner emission — in-order blocks, prompt/seed, live thinking, honest notices

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` (the RunStart write at :762-774, the messages build at :907-919, the stream closure at :977-1004, the trim/retry writes at :1040/:1067, `compact_context` at :412-487, the block-emission section at :1157-1200, the unknown-tool result at :1259-1266, the TurnEnd write at :1343-1347)
- Test: same file's `mod tests`

**Interfaces:**
- Consumes: Task 1's `Event` variants exactly as defined there.
- Produces: the emission-order contract Task 3 inverts — per turn: `TurnStart`, `Usage`, [`Compaction`], then per-block-in-provider-order (`Thinking` | `AssistantMessage{thinking: None}` | `ToolCall`), then `ToolResult`s (+ derived `FileEdit`/`CommandRun`) in dispatch order, then `TurnEnd{stop_reason, response_id}`. Run prologue: `RunStart{schema: Some(2), system_prompt}`, [`Seed`], [`UserMessage`]. Also makes `clamp_tool_result_text` `pub(crate)`.

- [ ] **Step 1: Rewrite the two existing reasoning tests to pin the NEW contract, and add ordering/prologue tests.** In `runner.rs` `mod tests`, replace `assistant_message_carries_reasoning_text` and `reasoning_only_turn_still_records_thinking` with:

```rust
    /// Collect (type-tag, data) pairs from the transcript for order assertions.
    fn event_tags(path: &std::path::Path) -> Vec<String> {
        rupu_transcript::JsonlReader::iter(path)
            .expect("open transcript")
            .filter_map(Result::ok)
            .map(|e| {
                let v = serde_json::to_value(&e).unwrap();
                v["type"].as_str().unwrap().to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn thinking_is_a_first_class_event_in_block_order() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![
                reasoning("thought"),
                ContentBlock::Text { text: "answer".into() },
            ],
            stop: StopReason::EndTurn,
        }]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let tags = event_tags(&transcript_path);
        let thinking_idx = tags.iter().position(|t| t == "thinking").expect("thinking event");
        let msg_idx = tags.iter().position(|t| t == "assistant_message").expect("assistant_message");
        assert!(thinking_idx < msg_idx, "thinking must precede the text it motivated: {tags:?}");
        // v2 writer never populates the legacy field.
        for ev in rupu_transcript::JsonlReader::iter(&transcript_path).unwrap().filter_map(Result::ok) {
            if let rupu_transcript::Event::AssistantMessage { thinking, .. } = ev {
                assert!(thinking.is_none(), "legacy thinking field must stay None");
            }
        }
    }

    #[tokio::test]
    async fn tool_only_turn_records_thinking_before_the_tool_call() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantBlocks {
                content: vec![
                    reasoning("pick a tool"),
                    ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "no_such_tool".into(),
                        input: serde_json::json!({}),
                    },
                ],
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantBlocks {
                content: vec![ContentBlock::Text { text: "done".into() }],
                stop: StopReason::EndTurn,
            },
        ]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let tags = event_tags(&transcript_path);
        let thinking_idx = tags.iter().position(|t| t == "thinking").unwrap();
        let call_idx = tags.iter().position(|t| t == "tool_call").unwrap();
        assert!(thinking_idx < call_idx, "on-disk order must match block order: {tags:?}");
    }

    #[tokio::test]
    async fn prologue_records_seed_user_message_and_v2_run_start() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop: StopReason::EndTurn,
        }]);
        let mut opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        opts.initial_messages = vec![Message::user("earlier"), Message::assistant("noted")];
        run_agent(opts).await.expect("run completes");

        let events: Vec<rupu_transcript::Event> = rupu_transcript::JsonlReader::iter(&transcript_path)
            .unwrap().filter_map(Result::ok).collect();
        match &events[0] {
            rupu_transcript::Event::RunStart { schema, system_prompt, .. } => {
                assert_eq!(*schema, Some(2));
                assert!(system_prompt.is_some());
            }
            other => panic!("first event must be run_start, got {other:?}"),
        }
        assert!(matches!(&events[1], rupu_transcript::Event::Seed { message_count: 2, .. }));
        assert!(matches!(&events[2], rupu_transcript::Event::UserMessage { .. }));
    }

    #[tokio::test]
    async fn unknown_tool_transcript_error_matches_what_the_model_is_fed() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "c1".into(),
                tool_name: "no_such_tool".into(),
                tool_input: serde_json::json!({}),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantBlocks {
                content: vec![ContentBlock::Text { text: "done".into() }],
                stop: StopReason::EndTurn,
            },
        ]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        let result = run_agent(opts).await.expect("run completes");
        // The recorded error string and the fed tool_result content must agree.
        let recorded_error = rupu_transcript::JsonlReader::iter(&transcript_path).unwrap()
            .filter_map(Result::ok)
            .find_map(|e| match e {
                rupu_transcript::Event::ToolResult { error: Some(err), .. } => Some(err),
                _ => None,
            })
            .expect("tool_result with error");
        let fed = result.final_messages.iter()
            .flat_map(|m| m.content.iter())
            .find_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .expect("fed tool_result block");
        assert_eq!(fed, format!("error: {recorded_error}\n"));
    }
```

(If `RunResult` has no `final_messages` populated on success in the last test, assert against the transcript + a `paused` run instead — check the struct at runner.rs:~690-705 first and keep whichever field actually carries the conversation; the invariant under test is record==fed.) Reuse the existing `reasoning(..)` test helper; extend it to include a `signature` key in `raw` if it doesn't already.

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-agent thinking_is_a_first_class` etc. → FAIL (old emission).

- [ ] **Step 3: Implement, in order through `runner.rs`:**

3a. **Move the `RunStart` write** from :765-774 to immediately after the coverage prompt-section append (after the `if let Some(bundle) = &coverage { ... push_str ... }` block), so `system_prompt` records the effective prompt. Keep `JsonlWriter::create`/`append` where they are (top). New write:

```rust
    writer.write(&Event::RunStart {
        run_id: opts.run_id.clone(),
        workspace_id: opts.workspace_id.clone(),
        agent: opts.agent_name.clone(),
        provider: opts.provider_name.clone(),
        model: opts.model.clone(),
        started_at: Utc::now(),
        mode: parse_mode_for_event(&opts.mode_str),
        schema: Some(2),
        system_prompt: Some(opts.agent_system_prompt.clone()),
    })?;
    writer.flush()?;
```

3b. **Seed + UserMessage** — first add to `AgentRunOpts`:

```rust
    /// Path of a transcript whose replay-reconstruction equals
    /// `initial_messages` byte-exact (spec §3 seed dedup rule). When set,
    /// the Seed event stores this reference instead of re-embedding the
    /// messages; the caller owns that invariant and replay verifies it via
    /// the recorded sha256. `None` → the seed is embedded inline in full.
    pub seed_source: Option<PathBuf>,
```

(every existing `AgentRunOpts` literal gains `seed_source: None`), plus a small helper next to `clamp_tool_result_text` (add `sha2` to `crates/rupu-agent/Cargo.toml` as `sha2 = { workspace = true }` — it is already pinned in the root for `rupu-update`; add the root pin if it turns out to live only in that crate's table):

```rust
/// Canonical seed hash: sha256 over the exact serde_json string of the
/// seeded `Vec<Message>`, hex-encoded. Replay recomputes this with the
/// same serializer to verify a seed reference chain.
pub(crate) fn seed_sha256(messages: &[Message]) -> String {
    use sha2::{Digest, Sha256};
    let canonical = serde_json::to_string(messages).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    format!("{:x}", h.finalize())
}
```

Then replace the block at :907-919 with:

```rust
    let mut messages: Vec<Message> = opts.initial_messages.clone();
    if !opts.initial_messages.is_empty() {
        let (source_transcript, inline) = match &opts.seed_source {
            // Stored once at the source; this transcript keeps a verifiable
            // reference instead of the 1000th copy.
            Some(src) => (Some(src.display().to_string()), None),
            None => (
                None,
                Some(serde_json::to_value(&opts.initial_messages).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to serialize seed messages for transcript");
                    serde_json::Value::Null
                })),
            ),
        };
        writer.write(&Event::Seed {
            message_count: opts.initial_messages.len() as u32,
            sha256: seed_sha256(&opts.initial_messages),
            source_transcript,
            messages: inline,
        })?;
    }
    // (keep the existing seed-only comment block verbatim)
    if !opts.user_message.is_empty() {
        messages.push(Message::user(&opts.user_message));
        writer.write(&Event::UserMessage { content: opts.user_message.clone() })?;
    }
    writer.flush()?;
```

3c. **Live thinking** — replace the `StreamEvent::ReasoningDelta(_) => {}` no-op at :1002 with:

```rust
                            StreamEvent::ReasoningDelta(chunk) => {
                                if !chunk.is_empty() && stream_transcript_error.is_none() {
                                    if let Err(err) = writer
                                        .write(&Event::ThinkingDelta { content: chunk.clone() })
                                        .and_then(|_| writer.flush())
                                    {
                                        stream_transcript_error = Some(err);
                                    }
                                }
                            }
```

3d. **Honest notices** — replace the two bracketed `AssistantDelta` writes:
at :1040 → `Event::Notice { kind: "context_trim".into(), message: format!("context trimmed to fit window; retrying (attempt {trim_attempts})") }`;
at :1067 → `Event::Notice { kind: "provider_retry".into(), message: format!("transient provider error (retry {http_retries}/{MAX_HTTP_RETRIES}): {e_str}") }`.

3e. **Compaction event** — in `compact_context` (:459-479), replace the `AssistantDelta` note write with:

```rust
            let post = serde_json::to_value(&*messages).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to serialize post-compaction messages");
                serde_json::Value::Null
            });
            if let Err(e) = writer
                .write(&Event::Compaction {
                    seq,
                    summarized_messages: middle_len as u32,
                    backup_path: backup_display.clone(),
                    messages: post,
                })
                .and_then(|_| writer.flush())
            {
                tracing::warn!(error = %e, "failed to write compaction event to transcript");
            }
```

3f. **In-order block emission** — replace :1157-1200 (the `turn_thinking` machinery, the block loop, and the post-loop flush) with:

```rust
            // Emit the turn's content blocks in the provider's own order —
            // thinking, text, and tool_use land in the transcript exactly as
            // the model produced them (spec §3 emission-order contract).
            let mut tool_uses: Vec<(String, String, serde_json::Value)> = Vec::new();
            for block in &resp.content {
                match block {
                    ContentBlock::Text { text } => {
                        writer.write(&Event::AssistantMessage {
                            content: text.clone(),
                            thinking: None,
                        })?;
                    }
                    ContentBlock::Reasoning { text, provider, model, raw } => {
                        writer.write(&Event::Thinking {
                            text: text.clone(),
                            provider: provider.clone(),
                            model: model.clone(),
                            raw: raw.clone(),
                        })?;
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        writer.write(&Event::ToolCall {
                            call_id: id.clone(),
                            tool: name.clone(),
                            input: input.clone(),
                        })?;
                        tool_uses.push((id.clone(), name.clone(), input.clone()));
                    }
                    ContentBlock::ToolResult { .. } => {}
                    ContentBlock::Unknown => {}
                }
            }
```

3g. **Record==fed for unknown tools** — at :1266 change `tool_results.push((call_id, String::new(), Some("unknown_tool".into())));` to reuse the same string written to the transcript:

```rust
                        let err = format!("unknown tool: {tool_name}");
                        writer.write(&Event::ToolResult {
                            call_id: call_id.clone(),
                            output: String::new(),
                            error: Some(err.clone()),
                            duration_ms: 0,
                            structured: None,
                        })?;
                        tool_results.push((call_id, String::new(), Some(err)));
```

3h. **TurnEnd extras** — extend the write at :1343-1347:

```rust
            writer.write(&Event::TurnEnd {
                turn_idx,
                tokens_in: Some(resp.usage.input_tokens as u64),
                tokens_out: Some(billable_output_tokens),
                stop_reason: resp.stop_reason.as_ref().map(|s| {
                    match s {
                        StopReason::EndTurn => "end_turn",
                        StopReason::MaxTokens => "max_tokens",
                        StopReason::StopSequence => "stop_sequence",
                        StopReason::ToolUse => "tool_use",
                    }
                    .to_string()
                }),
                response_id: {
                    let id = resp.id.trim();
                    if id.is_empty() { None } else { Some(id.to_string()) }
                },
            })?;
```

3i. Make `clamp_tool_result_text` `pub(crate)` (Task 3 uses it from `replay.rs`).

3j. **Wire the session worker's seed reference** (the O(n²) case the dedup exists for). In `crates/rupu-cli/src/cmd/session.rs`, the send path builds `initial_messages` from `SessionRecord.message_history` (~:6994). The record already knows each turn's run/transcript (the per-turn transcripts SessionConversation stacks); thread the PREVIOUS turn's transcript path into the new `seed_source` field when (a) a previous turn exists and (b) its transcript file still exists — otherwise leave `None` (inline embed; first turns have no seed at all). Verify the invariant with a test in `session.rs`'s test module: run two scripted turns through the session send path, read turn 2's transcript, assert its `Seed` has `source_transcript == Some(<turn 1 transcript path>)` and `messages == None`. (Task 3 then extends this same test with the end-to-end assertion that `rupu_agent::replay::reconstruct_transcript(<turn 2 path>)` returns the full two-turn conversation — the function doesn't exist yet at this step.) Orchestrator pause/resume deliberately keeps `seed_source: None` this arc — one inline embed per resume is bounded, and its seed passes through `split_seed_for_resume`, so the reconstruction-equality invariant is not guaranteed there.

- [ ] **Step 4: Run** — `cargo test -p rupu-agent` → PASS (fix any other runner tests that pinned the old `assistant_message.thinking` behavior — update them to the new contract, don't weaken them).

- [ ] **Step 5: Clippy + per-file fmt, commit**

```bash
git add crates/rupu-agent/src/runner.rs
git commit -m "feat(agent): transcript v2 emission — in-order thinking events, seed/user prologue, live thinking deltas, compaction/notice events, record==fed tool errors"
```

---

### Task 3: `replay` module — reconstruct the exact conversation, round-trip verified

**Files:**
- Create: `crates/rupu-agent/src/replay.rs`
- Modify: `crates/rupu-agent/src/lib.rs` (add `pub mod replay;`)

**Interfaces:**
- Consumes: Task 1 `Event` shapes, Task 2 emission order, `pub(crate) clamp_tool_result_text`.
- Produces:
  - `pub fn reconstruct_messages(events: &[rupu_transcript::Event]) -> Result<Vec<rupu_providers::types::Message>, ReplayError>` — pure; a referenced (non-inline) seed yields `ReplayError::SeedUnresolved`.
  - `pub fn reconstruct_transcript(path: &Path) -> Result<Vec<Message>, ReplayError>` — reads the file, resolves `source_transcript` chains recursively (depth cap 1024, absolute-path links), and verifies each link against the recorded `sha256` — the entrypoint every seed-aware consumer uses.
  - `pub enum ReplayError` (thiserror): `Seed(serde_json::Error)`, `Compaction(serde_json::Error)`, `SeedUnresolved { path: String }`, `SeedSource { path: String, source: rupu_transcript::ReadError }`, `SeedHashMismatch { path: String }`, `SeedChainTooDeep`.

- [ ] **Step 1: Write the failing round-trip tests** in `replay.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{run_agent, MockProvider, ScriptedTurn /* + the opts_for-equivalent test helpers */};
    use rupu_providers::types::{ContentBlock, Message, StopReason};

    fn reasoning(text: &str) -> ContentBlock {
        ContentBlock::Reasoning {
            text: Some(text.into()),
            provider: "mock".into(),
            model: "mock-1".into(),
            raw: serde_json::json!({"type":"thinking","thinking": text, "signature":"sig-abc"}),
        }
    }

    fn read_events(path: &std::path::Path) -> Vec<rupu_transcript::Event> {
        rupu_transcript::JsonlReader::iter(path).unwrap().filter_map(Result::ok).collect()
    }

    #[tokio::test]
    async fn round_trip_reasoning_and_tool_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript_path = tmp.path().join("run.jsonl");
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantBlocks {
                content: vec![
                    reasoning("pick a tool"),
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "no_such_tool".into(),
                        input: serde_json::json!({"q": 1}),
                    },
                ],
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantBlocks {
                content: vec![reasoning("summarize"), ContentBlock::Text { text: "done".into() }],
                stop: StopReason::EndTurn,
            },
        ]);
        // Same opts constructor the runner tests use, user_message = "task".
        let opts = crate::runner::tests::opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let rebuilt = reconstruct_messages(&read_events(&transcript_path)).expect("reconstruct");
        let expected: Vec<Message> = vec![
            Message::user("task"),
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![
                    reasoning("pick a tool"),
                    ContentBlock::ToolUse { id: "c1".into(), name: "no_such_tool".into(), input: serde_json::json!({"q": 1}) },
                ],
            },
            Message {
                role: rupu_providers::types::Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "error: unknown tool: no_such_tool\n".into(),
                    is_error: true,
                }],
            },
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![reasoning("summarize"), ContentBlock::Text { text: "done".into() }],
            },
        ];
        assert_eq!(
            serde_json::to_value(&rebuilt).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "transcript must rebuild the exact conversation, raw signatures included"
        );
    }

    #[tokio::test]
    async fn round_trip_preserves_seed_and_redacted_thinking() {
        let tmp = tempfile::tempdir().unwrap();
        let transcript_path = tmp.path().join("run.jsonl");
        let seed = vec![
            Message::user("earlier"),
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![ContentBlock::Reasoning {
                    text: None, // redacted
                    provider: "mock".into(),
                    model: "mock-1".into(),
                    raw: serde_json::json!({"type":"redacted_thinking","data":"opaque"}),
                }, ContentBlock::Text { text: "noted".into() }],
            },
        ];
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop: StopReason::EndTurn,
        }]);
        let mut opts = crate::runner::tests::opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        opts.initial_messages = seed.clone();
        run_agent(opts).await.expect("run completes");

        let rebuilt = reconstruct_messages(&read_events(&transcript_path)).unwrap();
        assert_eq!(
            serde_json::to_value(&rebuilt[..2]).unwrap(),
            serde_json::to_value(&seed).unwrap(),
            "inline seed must survive the transcript byte-exact, redacted raw included"
        );
    }

    #[tokio::test]
    async fn referenced_seed_chain_resolves_and_verifies_hash() {
        let tmp = tempfile::tempdir().unwrap();

        // Turn 1: a fresh run, its own transcript.
        let t1 = tmp.path().join("turn1.jsonl");
        let p1 = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![reasoning("first"), ContentBlock::Text { text: "hello".into() }],
            stop: StopReason::EndTurn,
        }]);
        run_agent(crate::runner::tests::opts_for(Box::new(p1), tmp.path(), t1.clone()))
            .await
            .expect("turn 1 completes");
        let turn1_convo = reconstruct_transcript(&t1).expect("turn 1 reconstructs");

        // Turn 2: seeded BY REFERENCE to turn 1's transcript — no re-embed.
        let t2 = tmp.path().join("turn2.jsonl");
        let p2 = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![ContentBlock::Text { text: "again".into() }],
            stop: StopReason::EndTurn,
        }]);
        let mut opts = crate::runner::tests::opts_for(Box::new(p2), tmp.path(), t2.clone());
        opts.initial_messages = turn1_convo.clone();
        opts.seed_source = Some(t1.clone());
        run_agent(opts).await.expect("turn 2 completes");

        // The seed is a reference, not a copy.
        let seed_ev = read_events(&t2).into_iter().find_map(|e| match e {
            rupu_transcript::Event::Seed { source_transcript, messages, .. } => {
                Some((source_transcript, messages))
            }
            _ => None,
        });
        let (src, inline) = seed_ev.expect("turn 2 has a seed event");
        assert_eq!(src.as_deref(), Some(t1.display().to_string().as_str()));
        assert!(inline.is_none(), "referenced seed must not re-embed the messages");

        // Chain-resolved reconstruction yields the full conversation.
        let full = reconstruct_transcript(&t2).expect("chain resolves");
        assert_eq!(
            serde_json::to_value(&full[..turn1_convo.len()]).unwrap(),
            serde_json::to_value(&turn1_convo).unwrap()
        );
        assert!(full.len() > turn1_convo.len(), "turn 2's own messages follow the seed");

        // The pure entrypoint refuses to guess.
        assert!(matches!(
            reconstruct_messages(&read_events(&t2)),
            Err(ReplayError::SeedUnresolved { .. })
        ));

        // Tampering with the source is caught, never silently absorbed.
        std::fs::write(&t1, "").unwrap();
        assert!(matches!(
            reconstruct_transcript(&t2),
            Err(ReplayError::SeedHashMismatch { .. }) | Err(ReplayError::SeedSource { .. })
        ));
    }
}
```

(Make `runner::tests::opts_for` reachable — either `pub(crate)` the helper in a `#[cfg(test)] pub(crate) mod tests` or duplicate the ~20-line opts literal locally; prefer exposing the helper.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-agent replay` → FAIL (module doesn't exist).

- [ ] **Step 3: Implement `replay.rs`:**

```rust
//! Rebuild the exact provider conversation from a v2 transcript (spec §4).
//!
//! Inverse of the runner's emission contract: per turn, `Thinking` /
//! `AssistantMessage` / `ToolCall` events (in on-disk order) fold into one
//! assistant `Message`; the turn's `ToolResult`s fold into the following
//! user message using the SAME error-formatting + clamp the runner feeds
//! the model; `Seed` initializes state; `Compaction` replaces it. A turn
//! with no `TurnEnd` (paused / aborted mid-turn) is dropped, matching the
//! runner, which never committed it to `messages` either.
//!
//! Legacy (v1) transcripts reconstruct without reasoning blocks — the v1
//! `assistant_message.thinking` string has no `raw` payload to rebuild.

use rupu_providers::types::{ContentBlock, Message, Role};
use rupu_transcript::Event;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("malformed seed messages: {0}")]
    Seed(serde_json::Error),
    #[error("malformed compaction messages: {0}")]
    Compaction(serde_json::Error),
    #[error("seed references transcript {path}; use reconstruct_transcript to resolve chains")]
    SeedUnresolved { path: String },
    #[error("seed references transcript {path} but it could not be read: {source}")]
    SeedSource {
        path: String,
        source: rupu_transcript::ReadError,
    },
    #[error("seed hash mismatch for referenced transcript {path} (chain edited, pruned, or cross-version serialization drift)")]
    SeedHashMismatch { path: String },
    #[error("seed reference chain exceeds depth limit")]
    SeedChainTooDeep,
}

#[derive(Default)]
struct TurnAccum {
    assistant_blocks: Vec<ContentBlock>,
    call_order: Vec<String>,
    results: HashMap<String, (String, Option<String>)>,
}

impl TurnAccum {
    fn flush_into(&mut self, messages: &mut Vec<Message>) {
        if !self.assistant_blocks.is_empty() {
            messages.push(Message {
                role: Role::Assistant,
                content: std::mem::take(&mut self.assistant_blocks),
            });
        }
        let blocks: Vec<ContentBlock> = self
            .call_order
            .drain(..)
            .filter_map(|id| {
                self.results.remove(&id).map(|(output, error)| {
                    let is_error = error.is_some();
                    let content = match error {
                        Some(e) => crate::runner::clamp_tool_result_text(&format!(
                            "error: {e}\n{output}"
                        )),
                        None => output,
                    };
                    ContentBlock::ToolResult { tool_use_id: id, content, is_error }
                })
            })
            .collect();
        if !blocks.is_empty() {
            messages.push(Message { role: Role::User, content: blocks });
        }
        self.results.clear();
    }
}

/// Pure reconstruction: inline seeds only. A referenced seed errors with
/// `SeedUnresolved` — callers with filesystem access use
/// [`reconstruct_transcript`], which resolves chains.
pub fn reconstruct_messages(events: &[Event]) -> Result<Vec<Message>, ReplayError> {
    reconstruct_with(events, &mut |path| {
        Err(ReplayError::SeedUnresolved { path: path.to_string() })
    })
}

/// Read `path` and reconstruct its conversation, recursively resolving
/// `Seed.source_transcript` chains (each hop re-enters this function) and
/// verifying every resolved seed against its recorded `sha256`. Depth-capped
/// so a cyclic/hostile chain terminates.
pub fn reconstruct_transcript(path: &std::path::Path) -> Result<Vec<Message>, ReplayError> {
    reconstruct_transcript_at_depth(path, 0)
}

const MAX_SEED_CHAIN_DEPTH: u32 = 1024;

fn reconstruct_transcript_at_depth(
    path: &std::path::Path,
    depth: u32,
) -> Result<Vec<Message>, ReplayError> {
    if depth > MAX_SEED_CHAIN_DEPTH {
        return Err(ReplayError::SeedChainTooDeep);
    }
    let read = |p: &std::path::Path| -> Result<Vec<Event>, rupu_transcript::ReadError> {
        Ok(rupu_transcript::JsonlReader::iter(p)?
            .filter_map(Result::ok)
            .collect())
    };
    let events = read(path).map_err(|e| ReplayError::SeedSource {
        path: path.display().to_string(),
        source: e,
    })?;
    reconstruct_with(&events, &mut |src| {
        reconstruct_transcript_at_depth(std::path::Path::new(src), depth + 1)
    })
}

fn reconstruct_with(
    events: &[Event],
    resolve: &mut dyn FnMut(&str) -> Result<Vec<Message>, ReplayError>,
) -> Result<Vec<Message>, ReplayError> {
    let mut messages: Vec<Message> = Vec::new();
    let mut accum = TurnAccum::default();

    for ev in events {
        match ev {
            Event::Seed { sha256, source_transcript, messages: inline, .. } => {
                let seed: Vec<Message> = match (inline, source_transcript) {
                    (Some(m), _) => {
                        serde_json::from_value(m.clone()).map_err(ReplayError::Seed)?
                    }
                    (None, Some(src)) => {
                        let resolved = resolve(src)?;
                        // Verify the chain instead of trusting it.
                        if crate::runner::seed_sha256(&resolved) != *sha256 {
                            return Err(ReplayError::SeedHashMismatch { path: src.clone() });
                        }
                        resolved
                    }
                    (None, None) => Vec::new(), // malformed but tolerated: empty seed
                };
                messages = seed;
            }
            Event::UserMessage { content } => messages.push(Message::user(content)),
            Event::Compaction { messages: m, .. } => {
                messages = serde_json::from_value(m.clone()).map_err(ReplayError::Compaction)?;
            }
            Event::Thinking { text, provider, model, raw } => {
                accum.assistant_blocks.push(ContentBlock::Reasoning {
                    text: text.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    raw: raw.clone(),
                });
            }
            Event::AssistantMessage { content, .. } => {
                // v1 wrote a synthetic empty-content message to carry
                // thinking on tool-only turns; the model never saw an empty
                // text block, so skip those.
                if !content.is_empty() {
                    accum.assistant_blocks.push(ContentBlock::Text { text: content.clone() });
                }
            }
            Event::ToolCall { call_id, tool, input } => {
                accum.assistant_blocks.push(ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: tool.clone(),
                    input: input.clone(),
                });
                accum.call_order.push(call_id.clone());
            }
            Event::ToolResult { call_id, output, error, .. } => {
                accum.results.insert(call_id.clone(), (output.clone(), error.clone()));
            }
            Event::TurnEnd { .. } => accum.flush_into(&mut messages),
            // Non-conversation events.
            Event::RunStart { .. }
            | Event::TurnStart { .. }
            | Event::AssistantDelta { .. }
            | Event::ThinkingDelta { .. }
            | Event::FileEdit { .. }
            | Event::CommandRun { .. }
            | Event::ActionEmitted { .. }
            | Event::GateRequested { .. }
            | Event::Usage { .. }
            | Event::RunComplete { .. }
            | Event::ToolAudit { .. }
            | Event::NetFlow { .. }
            | Event::Notice { .. }
            | Event::Unknown => {}
        }
    }
    // No trailing flush: a turn without TurnEnd was never committed by the
    // runner either.
    Ok(messages)
}
```

- [ ] **Step 4: Run** — `cargo test -p rupu-agent replay` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-agent/src/replay.rs crates/rupu-agent/src/lib.rs crates/rupu-agent/src/runner.rs
git commit -m "feat(agent): replay::reconstruct_messages — transcript round-trips to the exact conversation"
```

---

### Task 4: CLI renderers — every variant gets a real row, thinking uncapped in full view

**Files:**
- Modify: `crates/rupu-cli/src/cmd/transcript.rs` (`transcript_event_lines` :748-1080, streaming path ~:1258)
- Modify: `crates/rupu-cli/src/cmd/session.rs` (~:4371-4396), `crates/rupu-cli/src/output/workflow_printer.rs` (~:1297-1310), and any other site stubbed in Task 1
- Test: `transcript.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1 variants. Rendering contract (spec §5): `thinking` full-body in Compact/Full (Focused keeps a 96-char line; redacted → `[redacted reasoning · {provider}]`); `user_message` → `prompt` row; `seed` → one-liner; `notice`/`compaction` rows; `net_flow` one-liner (currently dropped); `Unknown` → dim `unrecognized event` row; `thinking_delta` dropped in after-the-fact views like `assistant_delta`.

- [ ] **Step 1: Write failing unit tests** against the pure `transcript_event_lines`:

```rust
    #[test]
    fn thinking_event_renders_full_body_in_full_view_and_redacted_marker() {
        let prefs = crate::cmd::ui::UiPrefs::default();
        let long = "x".repeat(300);
        let ev = TranscriptEvent::Thinking {
            text: Some(long.clone()),
            provider: "anthropic".into(),
            model: "m".into(),
            raw: serde_json::json!({}),
        };
        let full = transcript_event_lines(&ev, &prefs, LiveViewMode::Full);
        let joined: String = full.iter().map(|l| l.text.as_str()).collect();
        assert!(joined.contains(&long), "--view full must show the whole thinking body");

        let focused = transcript_event_lines(&ev, &prefs, LiveViewMode::Focused);
        assert!(focused.iter().any(|l| l.text.contains('…') || l.text.len() < 300));

        let redacted = TranscriptEvent::Thinking {
            text: None, provider: "anthropic".into(), model: "m".into(), raw: serde_json::json!({}),
        };
        let lines = transcript_event_lines(&redacted, &prefs, LiveViewMode::Full);
        assert!(lines[0].text.contains("redacted reasoning"));
    }

    #[test]
    fn v2_rows_exist_for_every_new_variant_and_netflow() {
        let prefs = crate::cmd::ui::UiPrefs::default();
        let cases: Vec<TranscriptEvent> = vec![
            TranscriptEvent::UserMessage { content: "do it".into() },
            TranscriptEvent::Seed { message_count: 4, messages: serde_json::json!([]) },
            TranscriptEvent::Notice { kind: "context_trim".into(), message: "trimmed".into() },
            TranscriptEvent::Compaction { seq: 1, summarized_messages: 8, backup_path: "/b".into(), messages: serde_json::json!([]) },
            TranscriptEvent::Unknown,
        ];
        for ev in &cases {
            assert!(
                !transcript_event_lines(ev, &prefs, LiveViewMode::Compact).is_empty(),
                "no row for {ev:?} — silent drop"
            );
        }
        // thinking_delta stays dropped, like assistant_delta.
        assert!(transcript_event_lines(
            &TranscriptEvent::ThinkingDelta { content: "c".into() }, &prefs, LiveViewMode::Full
        ).is_empty());
    }
```

(Construct a `NetFlow` case too, reusing the `FlowRecord` literal from `rupu-transcript/src/event.rs`'s `netflow_event_round_trips` test, and assert its lines are non-empty.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-cli transcript_event` → FAIL (stub arms return empty; thinking capped).

- [ ] **Step 3: Implement the arms in `transcript_event_lines`** (replacing Task 1's stub arm):

```rust
        TranscriptEvent::Thinking { text, provider, .. } => {
            match text.as_deref().filter(|t| !t.trim().is_empty()) {
                None => vec![transcript_event_line(
                    Status::Active, 0, false,
                    transcript_event_text(
                        Status::Active, "thinking",
                        &format!("[redacted reasoning · {provider}]"),
                    ),
                )],
                Some(t) => match view_mode {
                    LiveViewMode::Focused => vec![transcript_event_line(
                        Status::Active, 0, false,
                        transcript_event_text(Status::Active, "thinking", &truncate_single_line(t, 96)),
                    )],
                    LiveViewMode::Compact | LiveViewMode::Full => {
                        let payload = crate::output::rich_payload::render_payload(t, prefs);
                        render_payload_body_lines(Status::Active, "thinking", &payload.rendered, 0)
                    }
                },
            }
        }
        TranscriptEvent::ThinkingDelta { .. } => Vec::new(),
        TranscriptEvent::UserMessage { content } => match view_mode {
            LiveViewMode::Focused => vec![transcript_event_line(
                Status::Active, 0, false,
                transcript_event_text(Status::Active, "prompt", &truncate_single_line(content, 96)),
            )],
            LiveViewMode::Compact | LiveViewMode::Full => {
                let payload = crate::output::rich_payload::render_payload(content, prefs);
                render_payload_body_lines(Status::Active, "prompt", &payload.rendered, 0)
            }
        },
        TranscriptEvent::Seed { message_count, source_transcript, .. } => {
            let detail = match source_transcript {
                Some(src) => format!(
                    "{message_count} prior messages seed this run  ·  from {}",
                    truncate_single_line(src, 48)
                ),
                None => format!("{message_count} prior messages seed this run"),
            };
            vec![transcript_event_line(
                Status::Active, 0, false,
                transcript_event_text(Status::Active, "seed", &detail),
            )]
        }
        TranscriptEvent::Notice { kind, message } => vec![transcript_event_line(
            Status::Awaiting, 0, false,
            transcript_event_text(
                Status::Awaiting, "notice",
                &format!("{kind}  ·  {}", truncate_single_line(message, 96)),
            ),
        )],
        TranscriptEvent::Compaction { seq, summarized_messages, backup_path, .. } => {
            vec![transcript_event_line(
                Status::Active, 0, false,
                transcript_event_text(
                    Status::Active, "compaction",
                    &format!("seq {seq}  ·  summarized {summarized_messages} messages  ·  backup {backup_path}"),
                ),
            )]
        }
        TranscriptEvent::NetFlow { flow } => {
            let ok = flow.status.is_some_and(|s| s < 400) && flow.error.is_none();
            let status = if ok { Status::Complete } else { Status::Failed };
            vec![transcript_event_line(
                status, 0, false,
                transcript_event_text(
                    status, "net flow",
                    &format!(
                        "{} {}{}  ·  {}  ·  {}ms",
                        flow.method, flow.host, flow.path,
                        flow.status.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
                        flow.duration_ms.unwrap_or(0),
                    ),
                ),
            )]
        }
        TranscriptEvent::Unknown => vec![transcript_event_line(
            Status::Active, 0, false,
            transcript_event_text(Status::Active, "event", "unrecognized event type (newer rupu wrote this transcript)"),
        )],
```

Also in the legacy `AssistantMessage` arm (:787-800): keep the 96-char line for Focused, but in Compact/Full replace `truncate_single_line(thinking, 96)` with the same `render_payload` full-body treatment as the new Thinking arm.

- [ ] **Step 4: Mirror the same treatment at the other stubbed sites.** Each already renders `AssistantMessage.thinking` with its local line-builder; give the new `Thinking` arm that site's exact thinking-row code (swapping the 96-cap for the full body wherever that site's full-payload mode applies — `workflow_printer.rs` gates on `view_mode.shows_full_payloads()`, use that), and add the other variants' one-liners with the same labels as above (`prompt`, `seed`, `notice`, `compaction`, `net flow`, unrecognized). `ThinkingDelta` → dropped everywhere after-the-fact; the live streaming path in `transcript.rs` (~:1258) should render thinking deltas the way it renders text deltas, dimmed with the `thinking` label. `cargo build --workspace` proves no stub remains: `grep -rn "upgraded to real rows in Task 4" crates/` must return nothing.

- [ ] **Step 5: Run everything** — `cargo test -p rupu-cli -p rupu-agent -p rupu-transcript -p rupu-orchestrator && cargo clippy --workspace --all-targets` → PASS. Then `make macos-fixtures && cargo test -p rupu-cp` (schema change ⇒ fixture regen; commit regenerated `apps/rupu-macos/Fixtures/*.json`).

- [ ] **Step 6: Commit + PR**

```bash
git add crates/rupu-cli crates/rupu-agent apps/rupu-macos/Fixtures
git commit -m "feat(cli): render every transcript event — full thinking bodies, prompt/seed/notice/compaction/netflow rows"
git push origin feat/transcript-fidelity-plan-1
gh pr create --title "Transcript fidelity 1/3: v2 recording + verified replay" --body "Implements docs/superpowers/plans/2026-08-31-rupu-transcript-fidelity-plan-1-recording-and-replay.md (spec: docs/superpowers/specs/2026-08-31-rupu-transcript-fidelity-design.md).

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
