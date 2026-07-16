# Reasoning capture — Plan 3 (openai_wire: Copilot + OpenAI-compatible)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Capture reasoning from chat-completions reasoning models (DeepSeek, Qwen, GLM, vLLM, Ollama)
into the transcript, **and fix DeepSeek multi-turn tool calling**, which is currently broken: DeepSeek
requires `reasoning_content` to be echoed back on tool-call turns and returns a **400** if it is stripped.

**Architecture:** `openai_wire.rs` captures whichever reasoning field the endpoint actually sent
(`reasoning_content` *or* `reasoning`) into a `ContentBlock::Reasoning` whose opaque `raw` holds those
fields **under their original keys**; the request builder copies those exact keys back onto the
assistant message. We echo back only what that same endpoint gave us, under the key it used — safe by
construction, and the same verbatim-replay philosophy already approved for Gemini.

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-reasoning-capture-design.md` (+ read the
"openai_wire addendum (Plan 3)" section — the spec's original one-line Plan 3 description said only
"read `reasoning_content`", which is insufficient).

**Base:** branch `reasoning-plan-3` off `reasoning-plan-2` (Plan 2, PR #485 → which targets
`reasoning-capture`, PR #482). Neither is merged. **This PR targets `reasoning-plan-2`.**

Plan 1's `ContentBlock::Reasoning` / `Unknown` / `StreamEvent::ReasoningDelta` / `reasoning_text()`
are already available.

## Why this is not simple: one dialect, many contracts

`openai_wire.rs` is shared by `github_copilot.rs` and `openai_compatible.rs`, and "OpenAI-compatible"
is a *dialect* spoken by many servers that **disagree**:

| Backend | Reasoning field | Echo policy |
|---|---|---|
| DeepSeek | `reasoning_content` | **Required on tool-call turns — 400 if stripped** |
| Qwen / DashScope | `reasoning_content` | Ignored by default (`preserve_thinking: true` to opt in) |
| GLM / Z.ai | `reasoning_content` | Stripped by default (`clear_thinking` defaults true) |
| vLLM ≥ 0.12 | **`reasoning`** (renamed from `reasoning_content`) | Silently dropped on input (open bug) |
| Ollama (`/v1/chat/completions`) | `reasoning` (undocumented; from source) | — |
| OpenRouter | `reasoning` + `reasoning_details` | Required **and order-sensitive** |
| GitHub Copilot | `reasoning_text` / `reasoning_opaque` (undocumented) | Unknown |

There is **no single field name and no single echo policy**. Hence the rule below.

## Global Constraints

- **Echo back only what arrived, under the key it arrived on.** Never invent a reasoning field for an
  endpoint that did not send one. This is what makes a shared code path safe across backends that
  disagree: an endpoint that never sends `reasoning_content` never receives one.
- **Read both `reasoning_content` and `reasoning`.** Reading one is provably insufficient — vLLM
  renamed the field at 0.12, and the server version is not ours to control.
- `raw` holds the reasoning fields **verbatim under their original keys**; the echo copies those keys
  back. Never rename, translate, or normalize between them.
- Provider tag: `openai_chat` (the dialect tag — Copilot and OpenAI-compatible interoperate within it).
  Gate is **provider tag only, never model**.
- `Text`/`ToolUse`/`ToolResult` handling is unchanged. `Reasoning` is **additive**.
- Backward compatible: an endpoint that sends no reasoning behaves exactly as today.
- Capture is best-effort: a malformed/absent field is skipped, never an error.
- `#![deny(clippy::all)]`; no `unsafe`; workspace deps only.
- **Per-file rustfmt only:** `rustfmt --edition 2021 crates/rupu-providers/src/openai_wire.rs`.
  Never `cargo fmt`, never a `lib.rs`/mod root. `git status --short` before each commit.
- **No vacuous tests** (one was caught in Plan 1). Drive the real functions; assert real output.
- Homebrew toolchain 1.95 vs pinned 1.88: pre-existing lints in untouched files are not yours.

## OUT OF SCOPE — recorded, deliberately NOT fixed here

Keep this PR one thing. Record these in the spec; do not implement:
1. **OpenRouter `reasoning_details`** — structured and order-sensitive; flattening it to a string
   breaks Anthropic/Gemini-via-OpenRouter tool calling. Needs its own design. rupu is no worse than
   today.
2. **Copilot's `reasoning_text` / `reasoning_opaque`** — no vendor documentation exists at all; the
   only evidence is Zed's client source, which may drift. Do not guess a contract from one reader.
3. **`reasoning_effort` invalid values** (pre-existing, request-side): we send `minimal`/`xhigh`, but
   supported values are model-dependent — DeepSeek accepts only `high`/`max`; Groq and xAI reject both
   `minimal` and `xhigh`; `low|medium|high` is the only universally safe subset. Do NOT touch the
   request-side effort mapping in this plan.

---

## Task 1: Capture reasoning from chat-completions responses

**Files:**
- Modify: `crates/rupu-providers/src/openai_wire.rs` — `parse_chat_completion` (~:140), the SSE
  handler `process_completion_sse`, and its accumulator
- Test: same file, `mod tests`

**Interfaces — Produces:** `pub(crate) const PROVIDER_TAG: &str = "openai_chat";` and
`fn extract_reasoning_fields(msg_or_delta: &serde_json::Value) -> Option<(String, serde_json::Value)>`
returning `(field_name, value)` for the first present of `reasoning_content` then `reasoning`.

Read the file first and match its existing structure — the exact function/accumulator names below are
indicative; ground them in the real code.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn parse_captures_reasoning_content_field() {
    // choice.message.reasoning_content = "step by step" ->
    //   exactly one ContentBlock::Reasoning, provider "openai_chat",
    //   text == Some("step by step"),
    //   raw == {"reasoning_content": "step by step"}   // original key preserved
    // and message.content still parses to a Text block as today.
}

#[test]
fn parse_captures_renamed_reasoning_field() {
    // vLLM >= 0.12 renamed the field to `reasoning`. Same assertions, but
    // raw == {"reasoning": "..."} — the key it arrived on, NOT normalized.
}

#[test]
fn parse_prefers_reasoning_content_when_both_present() {
    // Defensive: if a server sends both, take reasoning_content (the DeepSeek/
    // Qwen/GLM name, the one with a hard echo requirement) and record that key.
}

#[test]
fn parse_without_reasoning_emits_no_reasoning_block() {
    // Backward compat: today's behavior, unchanged.
}

#[test]
fn parse_ignores_empty_reasoning_string() {
    // An empty string carries nothing to echo and nothing to show.
}

#[test]
fn sse_accumulates_reasoning_deltas_and_emits_events() {
    // delta.reasoning_content arriving in chunks -> StreamEvent::ReasoningDelta
    // per chunk, concatenated into one Reasoning block at the end. No TextDelta
    // is emitted for reasoning.
}

#[test]
fn sse_accumulates_renamed_reasoning_deltas() {
    // Same via delta.reasoning; raw uses the `reasoning` key.
}

#[test]
fn sse_without_reasoning_is_unchanged() {
    // Backward compat.
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- openai_wire` → FAIL.

- [ ] **Step 3: Implement.** Add near the top:

```rust
/// Canonical tag for the OpenAI chat-completions dialect. Copilot and the
/// generic OpenAI-compatible client both speak it and interoperate within it.
pub(crate) const PROVIDER_TAG: &str = "openai_chat";

/// The reasoning field names seen in the wild, in priority order.
///
/// There is no consensus: DeepSeek/Qwen/GLM use `reasoning_content`; vLLM
/// renamed it to `reasoning` at 0.12; Ollama's compat endpoint uses `reasoning`.
/// Reading one name is provably insufficient, and the server version is not
/// ours to control — so we read both and remember which one arrived, because
/// the echo must go back under the SAME key.
const REASONING_FIELDS: [&str; 2] = ["reasoning_content", "reasoning"];

/// Return the first present reasoning field as (name, value).
fn extract_reasoning_fields(v: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    REASONING_FIELDS.iter().find_map(|f| {
        let val = v.get(*f)?;
        // Skip empty/absent: nothing to show and nothing to echo.
        if val.as_str().map(|s| s.is_empty()).unwrap_or(false) {
            return None;
        }
        Some(((*f).to_string(), val.clone()))
    })
}
```

In `parse_chat_completion`, after the existing content/tool_calls handling, build the block:

```rust
    // Capture reasoning under the key it arrived on. `raw` is echoed back
    // verbatim: DeepSeek REQUIRES reasoning_content on tool-call turns and
    // returns a 400 if it is stripped.
    if let Some((field, value)) = extract_reasoning_fields(message) {
        let text = value.as_str().map(|s| s.to_string());
        content.insert(
            0,
            ContentBlock::Reasoning {
                text,
                provider: PROVIDER_TAG.to_string(),
                model: model.to_string(),
                raw: serde_json::json!({ field: value }),
            },
        );
    }
```

Insert at index 0 (reasoning precedes text — consistent with the other providers). If
`parse_chat_completion` has no model in scope, thread it in from the caller as Gemini's Plan 2 did;
read the call sites in `github_copilot.rs` / `openai_compatible.rs` and match their style.

For SSE: add `reasoning_text: String` and `reasoning_field: Option<String>` to the accumulator; on each
delta call `extract_reasoning_fields(delta)`, append the string value to `reasoning_text`, remember the
field name (first one wins), and emit `StreamEvent::ReasoningDelta(chunk)`. When finalizing, if
`reasoning_text` is non-empty, insert the same `Reasoning` block at index 0 with
`raw = { <remembered field>: reasoning_text }`.

- [ ] **Step 4:** `cargo test -p rupu-providers --lib -- openai_wire` → PASS; whole
  `cargo test -p rupu-providers --lib` green (this file is shared by Copilot AND OpenAI-compatible —
  both crates' tests must stay green).
- [ ] **Step 5:** rustfmt; `cargo clippy -p rupu-providers --no-deps`; `git status --short`; commit:

```bash
git commit -m "feat(providers): capture chat-completions reasoning (reasoning_content + reasoning)"
```

---

## Task 2: Echo reasoning back (fixes the DeepSeek 400)

**Files:**
- Modify: `crates/rupu-providers/src/openai_wire.rs` — `build_chat_request_body` (~:12-80), and the
  `ContentBlock::Reasoning { .. }` placeholder arm at ~:69
- Test: same file, `mod tests`

**Interfaces — Consumes:** Task 1's `PROVIDER_TAG` and the `Reasoning` block's `raw`.

DeepSeek's documented replay shape puts reasoning **on the assistant message**, beside `content` and
`tool_calls` — not in a content array:

```python
messages.append({'role': 'assistant', 'content': ..., 'reasoning_content': ..., 'tool_calls': ...})
```

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn assistant_message_echoes_reasoning_content_key() {
    // History: assistant turn with Reasoning{provider:"openai_chat",
    //   raw:{"reasoning_content":"thinking"}} + Text + ToolUse
    // -> the assistant message in the body carries
    //    "reasoning_content": "thinking" as a SIBLING of content/tool_calls,
    //    and content/tool_calls are unchanged.
}

#[test]
fn assistant_message_echoes_under_the_original_key() {
    // raw:{"reasoning":"thinking"} -> the body carries "reasoning", NOT
    // "reasoning_content". We never invent a field the endpoint didn't send.
}

#[test]
fn foreign_provider_reasoning_is_not_echoed() {
    // Reasoning{provider:"anthropic", raw:{"type":"thinking","signature":...}}
    // must NOT put anything on the message. An Anthropic signature must never
    // reach a chat-completions endpoint.
}

#[test]
fn internal_fields_never_reach_the_wire() {
    // Assert the serialized body contains no "provider"/"model"/"raw"/"reasoning"
    // -typed block anywhere — only the echoed key.
}

#[test]
fn reasoning_only_assistant_turn_is_not_dropped() {
    // The existing guard emits the message only when text_parts or tool_calls
    // are non-empty. A turn with reasoning + tool_calls must still be emitted;
    // assert reasoning rides along.
}

#[test]
fn unknown_block_is_not_echoed() {
    // ContentBlock::Unknown must never reach the wire.
}

#[test]
fn no_reasoning_block_leaves_body_unchanged() {
    // Backward compat: today's body byte-for-byte.
}
```

- [ ] **Step 2:** run → FAIL.

- [ ] **Step 3: Implement.** In `build_chat_request_body`'s `blocks` arm, collect the reasoning fields
  while iterating, then merge them onto the assistant message:

```rust
                    ContentBlock::Reasoning { provider, raw, .. } if provider == PROVIDER_TAG => {
                        // Echo back exactly the reasoning fields this endpoint
                        // sent, under their original keys. DeepSeek REQUIRES
                        // reasoning_content on tool-call turns (400 if stripped);
                        // backends that don't want it ignore or drop it. We never
                        // invent a field an endpoint didn't send.
                        if let Some(obj) = raw.as_object() {
                            reasoning_fields.extend(obj.clone());
                        }
                    }
                    ContentBlock::Reasoning { .. } => {
                        // Foreign provider: an alien wire format. Drop it.
                    }
```

Declare `let mut reasoning_fields = serde_json::Map::new();` alongside `text_parts`/`tool_calls`, and
after `msg_json` is built (and before it is pushed) merge:

```rust
                    for (k, v) in reasoning_fields {
                        msg_json[k] = v;
                    }
```

**Check the emit guard.** It currently reads `if !text_parts.is_empty() || !tool_calls.is_empty()`.
Reasoning alone must not resurrect an otherwise-empty message, but a turn with reasoning + tool_calls
must carry it — verify the merge happens on the path that is actually pushed. Read the real code; do
not assume the snippet's shape.

Also confirm the single-block fast paths at the top (`[ContentBlock::Text { text }]`,
`[ContentBlock::ToolResult { .. }]`) cannot swallow a reasoning-bearing turn: a `[Reasoning, Text]`
turn must fall through to the `blocks` arm. Add a test if that is not already obvious from the match.

- [ ] **Step 4:** `cargo test -p rupu-providers --lib` green.
- [ ] **Step 5:** rustfmt; clippy; `git status --short`; commit:

```bash
git commit -m "fix(providers): echo chat-completions reasoning back under its original key"
```

---

## Self-Review

**Spec coverage:** read both field names → T1; capture into the transcript (Plan 1's runner already
writes `thinking`) → T1; echo under the original key, fixing DeepSeek's documented 400 → T2;
provider-tag-only gate → T2's foreign-provider test. Out-of-scope items (OpenRouter
`reasoning_details`, Copilot's undocumented fields, `reasoning_effort` invalid values) are recorded in
the spec and deliberately untouched.

**Type flow:** `PROVIDER_TAG` + `extract_reasoning_fields` + `Reasoning{raw}` (T1) → merged onto the
assistant message (T2). T1 is inert alone; T2 activates it. Sequential.

**Blast radius:** `openai_wire.rs` is shared by Copilot and OpenAI-compatible. Both are additive-only:
an endpoint that sends no reasoning gets a byte-identical body to today, and one that does gets its own
field back. No shared-type change, no other provider touched.

## Execution

Subagent-driven: T1 → review → T2 → review → final whole-branch review → PR targeting
**`reasoning-plan-2`** (NOT main — the stack is #482 ← #485 ← this). No self-merge; matt reviews.
