# Reasoning capture — Plan 1 (shared type + Anthropic)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Capture Anthropic's reasoning end-to-end — parse `thinking`/`redacted_thinking`, echo them
back verbatim on the next turn, and surface the summary in the transcript — on a provider-agnostic
`ContentBlock::Reasoning` that Plans 2–4 extend to the other providers.

**Architecture:** A new internally-tagged `ContentBlock::Reasoning { text, provider, model, raw }`
carries an opaque provider-tagged payload. `raw` is echoed byte-exact to its producing provider and
dropped for any other. `ContentBlock`'s serde is rupu-internal, so Anthropic — which today builds its
request body by *generic* serde over `ContentBlock` — gains explicit translation in both directions.

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-reasoning-capture-design.md`

**Tech Stack:** Rust 2021, `serde`/`serde_json`, `tokio`, `tracing`.

## Global Constraints

- **Echo gate is the provider tag ONLY — never the model, never the text.** Anthropic's replay
  contract: thinking blocks are not origin-locked and replay across models fine; *stripping* them is
  what triggers ordering/signature 400s. A same-provider/different-model block **must still be echoed**.
- **`raw` round-trips byte-exact.** Never parse, edit, reorder, or reconstruct it. The API rejects
  *modified* blocks, not blocks you read — rendering `text` is fine.
- **A block with empty thinking text is still captured and still echoed** (the default
  `display: "omitted"` case). Never skip a block for having no readable text.
- **Reasoning blocks precede text/tool_use** in a rebuilt assistant turn (Anthropic ordering rule).
- Capture is best-effort: an unparseable reasoning block is dropped with `debug!`, never an error.
- Backward compatible: a response with no reasoning behaves exactly as today; existing tests stay green.
- `#![deny(clippy::all)]`; no `unsafe`; `thiserror`; workspace deps only (never pin in crate Cargo.toml).
- **Per-file rustfmt only:** `rustfmt --edition 2021 <file>` on non-mod-root files. Never `cargo fmt`,
  never on `lib.rs`/mod roots (rustfmt follows `mod` and reformats the tree → ~16-file drift).
  `--skip-children` does not exist in rustfmt 1.9.0. Check `git status --short` before each commit and
  `git restore` stray drift by name.
- The worktree runs Homebrew toolchain 1.95; pre-existing lints in untouched files are not yours.

## Canonical provider tags

`anthropic`, `google_gemini`, `openai_codex`, `openai_chat` (the shared `openai_wire` dialect —
Copilot and OpenAI-compatible interoperate within it), `local`. Plan 1 only introduces `anthropic`.

---

## Task 1: Shared `Reasoning` block + compile arms

Adding a variant breaks every exhaustive `match` in the workspace at once, so this task adds the
variant **and** every arm needed for a green build. All non-Anthropic arms are deliberate drops —
Plans 2–4 fill them in. No behavior change.

**Files:**
- Modify: `crates/rupu-providers/src/types.rs` (enum at `:14-32`, `LlmResponse` impl at `:201-217`,
  `StreamEvent` at `:222-231`)
- Modify (drop-arms only): `crates/rupu-providers/src/google_gemini.rs:526-559`,
  `crates/rupu-providers/src/openai_codex.rs:221-265`, `crates/rupu-providers/src/openai_wire.rs:46-67`,
  `crates/rupu-providers/src/openai_compatible.rs:184-194`, `crates/rupu-agent/src/runner.rs:1081-1101`,
  `crates/rupu-cli/src/cmd/session.rs:6036-6044` and `:6461-6465`
- Test: `crates/rupu-providers/src/types.rs` `mod tests` (`:233+`)

**Interfaces — Produces:** `ContentBlock::Reasoning { text: Option<String>, provider: String, model: String, raw: serde_json::Value }`; `ContentBlock::Unknown`; `StreamEvent::ReasoningDelta(String)`; `LlmResponse::reasoning_text() -> Option<String>`.

- [ ] **Step 1: Write the failing tests** in `types.rs` `mod tests`:

```rust
#[test]
fn reasoning_block_serde_round_trip() {
    let block = ContentBlock::Reasoning {
        text: Some("weighing options".into()),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        raw: serde_json::json!({"type": "thinking", "thinking": "weighing options", "signature": "abc123"}),
    };
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["type"], "reasoning");
    let back: ContentBlock = serde_json::from_value(json).unwrap();
    assert_eq!(back, block);
}

#[test]
fn reasoning_block_round_trips_with_empty_text() {
    // display:"omitted" returns thinking blocks whose text is empty; they must
    // survive the round trip so they can be echoed back unchanged.
    let block = ContentBlock::Reasoning {
        text: None,
        provider: "anthropic".into(),
        model: "claude-opus-4-8".into(),
        raw: serde_json::json!({"type": "thinking", "thinking": "", "signature": "sig"}),
    };
    let back: ContentBlock = serde_json::from_value(serde_json::to_value(&block).unwrap()).unwrap();
    assert_eq!(back, block);
}

#[test]
fn unknown_block_type_deserializes_instead_of_erroring() {
    // Regression guard: a strict tagged enum used to fail the whole turn.
    let json = serde_json::json!({"type": "some_future_block", "payload": 1});
    let block: ContentBlock = serde_json::from_value(json).expect("unknown block must not error");
    assert_eq!(block, ContentBlock::Unknown);
}

#[test]
fn reasoning_does_not_leak_into_text_or_tool_calls() {
    let resp = test_response(vec![
        ContentBlock::Reasoning {
            text: Some("hmm".into()),
            provider: "anthropic".into(),
            model: "m".into(),
            raw: serde_json::json!({}),
        },
        ContentBlock::Text { text: "answer".into() },
    ]);
    assert_eq!(resp.text(), Some("answer"));
    assert!(resp.tool_calls().is_empty());
}

#[test]
fn reasoning_text_concatenates_blocks_and_skips_textless() {
    let resp = test_response(vec![
        ContentBlock::Reasoning {
            text: Some("first".into()),
            provider: "anthropic".into(),
            model: "m".into(),
            raw: serde_json::json!({}),
        },
        ContentBlock::Reasoning {
            text: None, // redacted: opaque, nothing readable
            provider: "anthropic".into(),
            model: "m".into(),
            raw: serde_json::json!({}),
        },
        ContentBlock::Reasoning {
            text: Some("second".into()),
            provider: "anthropic".into(),
            model: "m".into(),
            raw: serde_json::json!({}),
        },
    ]);
    assert_eq!(resp.reasoning_text().as_deref(), Some("first\n\nsecond"));
}

#[test]
fn reasoning_text_is_none_without_reasoning_blocks() {
    let resp = test_response(vec![ContentBlock::Text { text: "hi".into() }]);
    assert_eq!(resp.reasoning_text(), None);
}
```

Add this helper next to the tests (keep it private to `mod tests`):

```rust
fn test_response(content: Vec<ContentBlock>) -> LlmResponse {
    LlmResponse {
        id: "msg_1".into(),
        model: "m".into(),
        content,
        stop_reason: None,
        usage: Usage { input_tokens: 0, output_tokens: 0, cached_tokens: 0 },
    }
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- types::tests` → FAIL (no `Reasoning` variant).

- [ ] **Step 3: Implement.** In `types.rs`, add to `ContentBlock` (after `ToolResult`, `:31`):

```rust
    /// Model reasoning/thinking, provider-agnostic.
    ///
    /// `raw` is the producing provider's original block, echoed back to that
    /// provider **byte-exact** on the next turn. It is never parsed, edited, or
    /// reconstructed — the API rejects modified blocks. `text` is the readable
    /// summary for the transcript/UI only.
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Canonical provider tag. Gates the echo: a provider emits `raw` iff
        /// this matches its own tag. Deliberately NOT gated on `model` —
        /// thinking blocks replay across models fine, and stripping them is
        /// what triggers ordering/signature 400s.
        provider: String,
        /// Informational only (transcript/debugging). Never an echo gate.
        model: String,
        raw: serde_json::Value,
    },

    /// Forward-compatibility catch-all: an unrecognized block type lands here
    /// instead of failing the whole turn's deserialization.
    #[serde(other)]
    Unknown,
```

If `#[serde(other)]` conflicts with a `rename` on the same variant, keep `other` and drop the rename —
`Unknown` is deserialize-only in practice and is never sent to a provider.

Add to `StreamEvent` (`:231`, after `InputJsonDelta`):

```rust
    /// A chunk of reasoning/thinking text.
    ReasoningDelta(String),
```

Add to `impl LlmResponse` (after `tool_calls()`, `:216`):

```rust
    /// Concatenated readable reasoning for this turn, if any.
    ///
    /// Blocks with no readable text (redacted, or `display: "omitted"`) are
    /// skipped here but still round-trip via their `raw` payload.
    pub fn reasoning_text(&self) -> Option<String> {
        let parts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Reasoning { text: Some(t), .. } if !t.is_empty() => Some(t.as_str()),
                _ => None,
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
```

- [ ] **Step 4: Add the compile arms.** `cargo build --workspace` and fix each non-exhaustive match.
  Every arm below is a deliberate drop with a `// Plan N` marker so the follow-up is greppable:
  - `google_gemini.rs:526-559` (`convert_messages`):
    `ContentBlock::Reasoning { .. } => { /* Plan 2: capture + echo thoughtSignature */ }`
    and `ContentBlock::Unknown => {}`
  - `openai_codex.rs:221-265` (`build_request_body`):
    `ContentBlock::Reasoning { .. } => { /* Plan 4: reasoning items + encrypted_content */ }`
    and `ContentBlock::Unknown => {}`
  - `openai_wire.rs:46-67` (`build_chat_request_body`):
    `ContentBlock::Reasoning { .. } => { /* Plan 3: reasoning_content */ }`
    and `ContentBlock::Unknown => {}`
  - `openai_compatible.rs:184-194` (`emit_response_events`): same two arms, both empty.
  - `rupu-agent/src/runner.rs:1081-1101`: `ContentBlock::Reasoning { .. } => {}` with
    `// Task 4 populates AssistantMessage.thinking from these.` and `ContentBlock::Unknown => {}`
  - `rupu-cli/src/cmd/session.rs:6036-6044` **and** `:6461-6465` (token-size estimators): count the
    `text` length when present, matching how sibling arms estimate; `Unknown => 0` (or the arm's
    equivalent zero). Read the surrounding function before choosing — match its existing style.
  - `local.rs:96-99` and `types.rs:204-207` already have `_ =>` wildcards — leave them.

- [ ] **Step 5:** `cargo test -p rupu-providers --lib -- types::tests` → PASS. Then
  `cargo build --workspace` and `cargo test -p rupu-providers -p rupu-agent --lib` → green.

- [ ] **Step 6:** rustfmt each changed file individually (`rustfmt --edition 2021 crates/rupu-providers/src/types.rs` etc. — none are mod roots). `cargo clippy -p rupu-providers --no-deps`. `git status --short`, then commit:

```bash
git commit -m "feat(providers): provider-agnostic ContentBlock::Reasoning + Unknown catch-all"
```

---

## Task 2: Anthropic response-side capture

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs` — SSE `content_block_start` (`:1344-1371`),
  `content_block_delta` (`:1373-1397`), `content_block_stop` (`:1399-1418`), `StreamAccumulator`
  (`:1457-1470`), `into_response` (`:1477-1499`), `AnthropicResponse` (`:1503-1520`), `send()` (`:941`)
- Test: `crates/rupu-providers/src/anthropic.rs` `mod tests`

**Interfaces — Consumes:** Task 1's `ContentBlock::Reasoning`, `Unknown`, `StreamEvent::ReasoningDelta`.
**Produces:** `pub(crate) const PROVIDER_TAG: &str = "anthropic";` and
`fn parse_content_blocks(raw: Vec<serde_json::Value>, model: &str) -> Vec<ContentBlock>`.

- [ ] **Step 1: Write the failing tests.** Follow the existing SSE-accumulator test at `:2496` for the
  harness shape (how events are fed and the response built) — mirror it, don't invent a new harness.

```rust
#[test]
fn stream_captures_thinking_block_with_signature() {
    // content_block_start(thinking) -> thinking_delta -> signature_delta -> stop
    // Assert: one ContentBlock::Reasoning, provider "anthropic",
    //   text Some("let me think"),
    //   raw == {"type":"thinking","thinking":"let me think","signature":"sig_xyz"}
}

#[test]
fn stream_captures_thinking_block_with_empty_text() {
    // display:"omitted" -> a thinking block arrives with no thinking_delta,
    // only a signature_delta. It MUST still produce a Reasoning block so it
    // can be echoed back unchanged. text is None; raw keeps thinking: "".
}

#[test]
fn stream_captures_redacted_thinking_block() {
    // content_block_start with {"type":"redacted_thinking","data":"enc..."}
    // Assert: Reasoning { text: None, raw: <the block verbatim> }
}

#[test]
fn stream_emits_reasoning_delta_events() {
    // Assert on_event received StreamEvent::ReasoningDelta("let me think")
    // and that no TextDelta was emitted for thinking content.
}

#[test]
fn into_response_places_reasoning_before_text() {
    // Regression guard for the ordering bug: Anthropic requires thinking
    // blocks first in an assistant turn. Accumulate a thinking block plus
    // text; assert content == [Reasoning, Text], NOT [Text, Reasoning].
}

#[test]
fn non_streaming_response_with_thinking_block_deserializes() {
    // Regression guard for the latent crash: this previously failed with
    // "unknown variant". Parse a full non-streaming body containing
    // thinking + text + tool_use; assert all three blocks land.
}

#[test]
fn non_streaming_unknown_block_type_is_dropped_not_fatal() {
    // A block type we don't know must not fail the turn.
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- anthropic` → new tests FAIL.

- [ ] **Step 3: Implement the streaming path.** Add to `StreamAccumulator` (`:1457`):

```rust
    current_reasoning_text: Option<String>,
    current_reasoning_signature: Option<String>,
```

In `content_block_start` (`:1344`), extend the existing `if block_type == "tool_use"` into a match on
`block_type`, keeping the `tool_use` arm's body **verbatim**:

```rust
    "thinking" => {
        // Seed with any text present on the start event; deltas append.
        acc.current_reasoning_text = Some(
            block.get("thinking").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        );
        acc.current_reasoning_signature = block
            .get("signature")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    "redacted_thinking" => {
        // Opaque and self-contained: no deltas follow. Push immediately,
        // preserving the block verbatim for the echo.
        acc.content_blocks.push(ContentBlock::Reasoning {
            text: None,
            provider: PROVIDER_TAG.to_string(),
            model: acc.model.clone(),
            raw: block.clone(),
        });
    }
```

In `content_block_delta` (`:1373`), add two arms before the `_ => debug!(...)` catch-all:

```rust
    "thinking_delta" => {
        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
            acc.current_reasoning_text.get_or_insert_with(String::new).push_str(t);
            on_event(StreamEvent::ReasoningDelta(t.to_string()));
        }
    }
    "signature_delta" => {
        if let Some(sig) = delta.get("signature").and_then(|v| v.as_str()) {
            // Signatures arrive whole, but append defensively rather than
            // overwrite: a truncated signature is rejected by the API.
            acc.current_reasoning_signature
                .get_or_insert_with(String::new)
                .push_str(sig);
        }
    }
```

In `content_block_stop` (`:1399`), add **before** the existing tool-use finalize block (so a stop event
resolves at most one pending block, and reasoning is never mistaken for a tool):

```rust
    // Finalize a pending thinking block. Reconstruct raw in Anthropic's own
    // wire shape so it echoes back byte-identical to what arrived. Note the
    // block is emitted even when the text is empty (display: "omitted") —
    // it must still round-trip.
    if let Some(text) = acc.current_reasoning_text.take() {
        let mut raw = serde_json::json!({ "type": "thinking", "thinking": text });
        if let Some(sig) = acc.current_reasoning_signature.take() {
            raw["signature"] = serde_json::Value::String(sig);
        }
        acc.content_blocks.push(ContentBlock::Reasoning {
            text: if text.is_empty() { None } else { Some(text.clone()) },
            provider: PROVIDER_TAG.to_string(),
            model: acc.model.clone(),
            raw,
        });
    }
```

Fix the ordering bug in `into_response` (`:1483-1486`) — replace `insert(0, ...)`:

```rust
        if !self.text.is_empty() {
            // Anthropic requires reasoning blocks first in an assistant turn,
            // so text goes after any leading reasoning — not at index 0. (The
            // old `insert(0, ..)` predates reasoning capture, when nothing
            // depended on block order.)
            let idx = self
                .content_blocks
                .iter()
                .position(|b| !matches!(b, ContentBlock::Reasoning { .. }))
                .unwrap_or(self.content_blocks.len());
            self.content_blocks.insert(idx, ContentBlock::Text { text: self.text });
        }
```

- [ ] **Step 4: Implement the non-streaming path.** Add the tag near the top of the file:

```rust
/// Canonical provider tag stamped on Reasoning blocks and used as the echo gate.
pub(crate) const PROVIDER_TAG: &str = "anthropic";
```

Change `AnthropicResponse.content` (`:1506`) from `Vec<ContentBlock>` to `Vec<serde_json::Value>`, and
have `into_llm_response` call a new explicit parser. **This is the fix for the latent crash:** the
derive could not represent `thinking`, and Task 1's `Unknown` catch-all would silently swallow it.

```rust
/// Parse Anthropic's wire content blocks into rupu's internal representation.
///
/// Explicit rather than derived: `ContentBlock`'s serde is rupu's internal
/// format, not Anthropic's wire format, and the two must not be coupled.
fn parse_content_blocks(raw: Vec<serde_json::Value>, model: &str) -> Vec<ContentBlock> {
    raw.into_iter()
        .filter_map(|block| {
            let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or_default();
            match block_type {
                "text" => Some(ContentBlock::Text {
                    text: block.get("text").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                }),
                "tool_use" => Some(ContentBlock::ToolUse {
                    id: block.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    // NOTE: deliberately NOT desanitized — the derive path this
                    // replaces never desanitized either, and changing that is a
                    // separate behavior change (see spec, latent defect #4).
                    name: block.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    input: block.get("input").cloned().unwrap_or(serde_json::Value::Null),
                }),
                "thinking" | "redacted_thinking" => {
                    let text = block
                        .get("thinking")
                        .and_then(|v| v.as_str())
                        .filter(|t| !t.is_empty())
                        .map(|t| t.to_string());
                    Some(ContentBlock::Reasoning {
                        text,
                        provider: PROVIDER_TAG.to_string(),
                        model: model.to_string(),
                        raw: block,
                    })
                }
                other => {
                    debug!(block_type = other, "dropping unrecognized content block");
                    None
                }
            }
        })
        .collect()
}
```

`into_llm_response` becomes `content: parse_content_blocks(self.content, &self.model)`. Keep
`tool_result` out of the parser: models never emit it (the existing runner arm at `:1096` says so).

- [ ] **Step 5:** `cargo test -p rupu-providers --lib -- anthropic` → PASS, including all existing
  request-side thinking tests (`:2100-2330`) and the SSE accumulator test (`:2496`).

- [ ] **Step 6:** `rustfmt --edition 2021 crates/rupu-providers/src/anthropic.rs`;
  `cargo clippy -p rupu-providers --no-deps`; `git status --short`; commit:

```bash
git commit -m "feat(providers): capture Anthropic thinking blocks (streaming + non-streaming)"
```

---

## Task 3: Anthropic request-side — `display` + verbatim echo

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs` — `build_request_body` (`:1106`), thinking block
  (`:1210-1242`), messages serialization (`:1110-1111`)
- Test: same file, `mod tests`

**Interfaces — Consumes:** Task 2's `PROVIDER_TAG`. **Produces:** `fn restore_reasoning_blocks(messages: &mut serde_json::Value, self_tag: &str)`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn adaptive_thinking_requests_summarized_display() {
    // ThinkingLevel::Auto -> body["thinking"] == {"type":"adaptive","display":"summarized"}
    // Without this, display defaults to "omitted" on Opus 4.7/4.8 + Sonnet 5
    // and every captured thinking text would be empty.
}

#[test]
fn oauth_implicit_adaptive_thinking_requests_summarized_display() {
    // The implicit OAuth path (:1236-1242) gets the same shape.
}

#[test]
fn budget_tokens_thinking_does_not_set_display() {
    // ThinkingLevel::High -> {"type":"enabled","budget_tokens":10000} with NO
    // display key: that path targets pre-4.6 models, which predate `display`.
}

#[test]
fn reasoning_block_is_restored_to_anthropic_wire_shape() {
    // A history message carrying ContentBlock::Reasoning{provider:"anthropic", raw}
    // serializes into the request as `raw` verbatim — byte-identical, no
    // internal fields (text/provider/model) leaking onto the wire.
}

#[test]
fn foreign_provider_reasoning_block_is_dropped_from_request() {
    // provider:"google_gemini" -> block removed. A Gemini thoughtSignature must
    // never reach Anthropic.
}

#[test]
fn same_provider_different_model_reasoning_block_is_still_echoed() {
    // Guard against reintroducing a model gate: thinking blocks are not
    // origin-locked, and stripping them triggers ordering/signature 400s.
}

#[test]
fn unknown_block_is_dropped_from_request() {
    // ContentBlock::Unknown must not serialize onto the wire.
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- anthropic` → FAIL.

- [ ] **Step 3: Implement `display`.** Both adaptive sites become:

```rust
body["thinking"] = serde_json::json!({ "type": "adaptive", "display": "summarized" });
```

— the `ThinkingLevel::Auto` arm (`:1213-1214`) and the implicit OAuth path (`:1241`). Leave the
`budget_tokens` arm untouched. Update the block comment above `:1210` to record why:

```rust
        //   * `display: "summarized"` — opt in to readable thinking text.
        //     Only ever set alongside "adaptive": `display` accepts exactly
        //     "summarized" | "omitted" (there is no raw/full — the raw chain of
        //     thought is not exposed on any Claude model), and the
        //     budget_tokens path targets pre-4.6 models that predate it.
        //     Without this, display defaults to "omitted" on Opus 4.7/4.8 and
        //     Sonnet 5, whose thinking blocks then carry an empty text field.
```

- [ ] **Step 4: Implement the echo.** Add next to `sanitize_messages_tool_names` (`:44`) — the
  established precedent for post-processing the generically-serialized message array:

```rust
/// Rewrite internal `Reasoning` blocks into Anthropic's own wire shape.
///
/// `build_request_body` serializes `request.messages` with generic serde, which
/// works only because our Text/ToolUse/ToolResult shapes coincide with
/// Anthropic's. `Reasoning` is internal-only, so it is restored from its `raw`
/// payload here — byte-exact, since the API rejects modified blocks.
///
/// Blocks produced by another provider are dropped: a foreign continuity token
/// is an alien wire format. Blocks from this provider are echoed regardless of
/// model — thinking blocks are not origin-locked, and *stripping* them is what
/// triggers ordering/signature 400s.
fn restore_reasoning_blocks(messages: &mut serde_json::Value, self_tag: &str) {
    let Some(msgs) = messages.as_array_mut() else { return };
    for msg in msgs {
        let Some(blocks) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else { continue };
        let mut restored = Vec::with_capacity(blocks.len());
        for block in blocks.iter() {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("reasoning") => {
                    let same_provider =
                        block.get("provider").and_then(|v| v.as_str()) == Some(self_tag);
                    if same_provider {
                        if let Some(raw) = block.get("raw") {
                            restored.push(raw.clone());
                        }
                    }
                    // else: foreign provider — drop.
                }
                Some("Unknown") => {} // never goes on the wire
                _ => restored.push(block.clone()),
            }
        }
        *blocks = restored;
    }
}
```

Call it in `build_request_body` immediately after the existing tool-name sanitization (`:1110-1111`),
so both post-processes operate on the same serialized array:

```rust
    let mut messages = serde_json::to_value(&request.messages)?;
    sanitize_messages_tool_names(&mut messages);
    restore_reasoning_blocks(&mut messages, PROVIDER_TAG);
```

Match the existing call's exact variable name and shape — read `:1106-1115` first and adapt rather
than transcribing blindly. If `ContentBlock::Unknown` serializes under a different tag than
`"Unknown"` (serde uses the variant name unless renamed), use whatever Task 1 actually produces —
assert it in the test rather than guessing.

- [ ] **Step 5:** `cargo test -p rupu-providers --lib -- anthropic` → PASS (new + all existing).

- [ ] **Step 6:** rustfmt the file; `cargo clippy -p rupu-providers --no-deps`; `git status --short`; commit:

```bash
git commit -m "feat(providers): echo Anthropic thinking blocks verbatim + request summarized display"
```

---

## Task 4: Populate the transcript's `thinking` field

The render path already exists (`cmd/session.rs:2299-2525`, `cmd/transcript.rs:664-674`,
`output/workflow_printer.rs:1247-1257`) and already renders `thinking` when non-empty. Only the write
site is missing.

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs` — the content-block loop (`:1080-1101`), specifically the
  hardcoded `thinking: None` at `:1085`
- Test: `crates/rupu-agent/src/runner.rs` `mod tests` (use the existing `MockProvider` + `BypassDecider`)

**Interfaces — Consumes:** Task 1's `LlmResponse::reasoning_text()`.

- [ ] **Step 1: Write the failing tests** using the existing `MockProvider` harness:

```rust
#[test] // or #[tokio::test], matching the neighbouring runner tests
fn assistant_message_carries_reasoning_text() {
    // MockProvider returns [Reasoning{text:Some("thought")}, Text{"answer"}].
    // Assert the transcript's AssistantMessage has
    //   content == "answer" AND thinking == Some("thought").
}

fn reasoning_only_turn_still_records_thinking() {
    // A tool-call turn with reasoning but NO text block:
    // [Reasoning{text:Some("planning")}, ToolUse{..}]
    // Assert an AssistantMessage with empty content and
    // thinking == Some("planning") is still written — reasoning must not be
    // lost on tool-only turns, which is exactly where it matters most.
}

fn assistant_message_thinking_is_none_without_reasoning() {
    // Backward compat: today's behavior, unchanged.
}

fn thinking_attaches_once_across_multiple_text_blocks() {
    // [Reasoning, Text("a"), Text("b")] -> the first AssistantMessage carries
    // the thinking; the second carries None. Not duplicated.
}
```

- [ ] **Step 2:** `cargo test -p rupu-agent --lib` → new tests FAIL.

- [ ] **Step 3: Implement.** Before the `for block in &resp.content` loop (`:1080`):

```rust
            // Reasoning precedes text in an assistant turn, so compute it up
            // front and attach it to the turn's first assistant message.
            let mut turn_thinking = resp.reasoning_text();
```

In the `ContentBlock::Text` arm (`:1082-1087`), replace `thinking: None`:

```rust
                    ContentBlock::Text { text } => {
                        writer.write(&Event::AssistantMessage {
                            content: text.clone(),
                            thinking: turn_thinking.take(),
                        })?;
                    }
```

After the loop, flush reasoning from a turn that produced no text block (tool-only turns):

```rust
            // A tool-only turn has no Text block to hang the reasoning on, but
            // the reasoning is still worth recording — that is the turn where
            // the model decided which tool to call.
            if let Some(thinking) = turn_thinking.take() {
                writer.write(&Event::AssistantMessage {
                    content: String::new(),
                    thinking: Some(thinking),
                })?;
            }
```

Place it after the block loop and **before** tool dispatch (`:1103`), so transcript order stays
chronological. Replace the placeholder `ContentBlock::Reasoning { .. } => {}` arm's comment from Task 1
with a note that the text is consumed via `turn_thinking`.

- [ ] **Step 4:** `cargo test -p rupu-agent --lib` → PASS. Then
  `cargo test -p rupu-providers -p rupu-agent -p rupu-orchestrator --lib` → green.

- [ ] **Step 5:** `rustfmt --edition 2021 crates/rupu-agent/src/runner.rs` (not a mod root);
  `cargo clippy -p rupu-agent --no-deps`; `git status --short`; commit:

```bash
git commit -m "feat(agent): record model reasoning in the transcript's thinking field"
```

---

## Self-Review

**Spec coverage:** shared `Reasoning` + `Unknown` + `ReasoningDelta` + `reasoning_text()` → T1;
Anthropic response capture (streaming + non-streaming), latent bug #1 (non-streaming deserialize) and
#2 (block ordering) → T2; `display: "summarized"` + verbatim echo with the provider-only gate → T3;
transcript population → T4. Plans 2–4 (Gemini / openai_wire / openai_codex) are out of scope by design
and are marked in-code with `// Plan N` drop arms.

**Global constraints represented:** provider-only echo gate → T3 tests (foreign-provider dropped,
same-provider/different-model still echoed); byte-exact `raw` → T3; empty-text capture → T1 + T2;
reasoning-before-text ordering → T2; backward compat → T1/T4 ("none without reasoning") + all existing
request-side thinking tests staying green.

**Type flow:** `ContentBlock::Reasoning`/`Unknown`/`ReasoningDelta`/`reasoning_text()` (T1) →
`PROVIDER_TAG` + `parse_content_blocks` (T2) → `restore_reasoning_blocks` (T3) → `reasoning_text()`
consumed in the runner (T4). Each task compiles and tests green on its own; strictly sequential.

**Known scope exclusions** (recorded in the spec, not defects of this plan): `local.rs` dropping
`ToolUse`/`ToolResult`; non-streaming tool names not desanitized; older binaries failing to read newer
`reasoning` blocks (`serde(other)` helps future additions, not this one).

## Execution

Subagent-driven: T1 → review → T2 → review → T3 → review → T4 → review → final whole-branch review →
PR to main. **No self-merge — matt reviews.** No GUI surface here (transcript rendering already exists
and is covered by tests), so no runtime GUI validation is required for this PR; the end-to-end check
matt may want is a real `rupu run` against Anthropic showing thinking text in the transcript.
