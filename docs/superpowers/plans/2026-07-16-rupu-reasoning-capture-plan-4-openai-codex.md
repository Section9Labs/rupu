# Reasoning capture — Plan 4 (openai_codex / Responses API)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Capture OpenAI Responses-API reasoning into the transcript and **echo reasoning items back
verbatim**, so reasoning-model tool loops stop risking the documented pairing 400s.

**Architecture:** Same verbatim-replay philosophy as Gemini (Plan 2): store the assistant turn's
**entire `output` items array verbatim** in one `ContentBlock::Reasoning`'s opaque `raw`, and replay it
into the next request's `input` instead of rebuilding from blocks. That preserves reasoning↔function_call
pairing and item IDs exactly as the server emitted them — which is precisely what the pairing errors
police. No shared-`ContentBlock` change; all local to `openai_codex.rs`.

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-reasoning-capture-design.md` + the
"openai_codex addendum (Plan 4)" section.

**Base:** branch `reasoning-plan-4` off `reasoning-plan-3`. Stack: #482 ← #485 ← #486 ← this.
**This PR targets `reasoning-plan-3`.** Plan 1's `ContentBlock::Reasoning` / `Unknown` /
`StreamEvent::ReasoningDelta` / `reasoning_text()` are already available.

## Grounded contract (researched against live OpenAI docs + SDK source)

A reasoning output item:
```json
{ "id": "rs_6876cf...", "type": "reasoning", "summary": [], "encrypted_content": "gAAAAAB...", "status": null }
```

- **Both directions hard-400 on bad pairing:** `"Item 'rs_...' of type 'reasoning' was provided without
  its required following item."` and the inverse `"'function_call' was provided without its required
  'reasoning' item."` The adjacency rule itself is **UNDOCUMENTED**; the only recipe known to work is
  replaying **all** output items verbatim, in order, IDs intact — which is exactly what this plan does.
- **`include: ["reasoning.encrypted_content"]` is no longer required** — stateless mode (`store:false`)
  now returns `encrypted_content` by default and the `include` value is labeled *legacy but still
  accepted*. We send it anyway: `openai/codex` sends it unconditionally, and it protects Azure/proxy
  backends that haven't adopted the new default.
- **`summary` is empty unless requested** via `reasoning: {summary: "auto"|"concise"|"detailed"}`.
  This is the analogue of Anthropic's `display: "summarized"`: without it we capture an opaque blob and
  the transcript shows nothing readable.
- `store: false` is already hardcoded and is correct — ZDR orgs have `store` **silently forced** to
  false, so `store:true` + `previous_response_id` would fail to resolve prior state with no clean error.

## Global Constraints

- **Replay all output items verbatim, in order, with IDs intact.** Never filter, reorder, or strip.
  The pairing errors are expressed in terms of item IDs, and preserving the server's own ordering is
  what keeps reasoning adjacent to its `function_call`.
- Provider tag: `openai_responses`. Gate is **provider tag only, never model**. This tag must be
  distinct from Plan 3's `openai_chat` — they are different wire formats and must never cross.
- `Text`/`ToolUse`/`ToolResult` blocks keep being emitted as today; `Reasoning` is **additive**.
- Backward compatible: a turn with no reasoning block rebuilds exactly as today.
- Capture is best-effort: a malformed item is skipped with `debug!`, never an error.
- `#![deny(clippy::all)]`; no `unsafe`; workspace deps only.
- **Per-file rustfmt only:** `rustfmt --edition 2021 crates/rupu-providers/src/openai_codex.rs`.
  Never `cargo fmt`, never a `lib.rs`/mod root. `git status --short` before each commit.
- **No vacuous tests** (one was caught in Plan 1). Drive the real functions; assert the real body.
- Homebrew toolchain 1.95 vs pinned 1.88: pre-existing lints in untouched files are not yours.

## OUT OF SCOPE — recorded, deliberately NOT fixed

1. **`reasoning.effort` per-model validity.** Valid values are `none|minimal|low|medium|high|xhigh|max`,
   but *"not all reasoning models support every value"* — `minimal`/`xhigh` are not universal, and no
   live per-model matrix exists. Do NOT touch the effort mapping.
2. **`store: true` + `previous_response_id`** as an alternative to echoing. Different architecture.
3. **Codex's ID-stripping divergence** — `openai/codex` strips `id` from input items in stateless mode
   (its own `item_ids_enabled` flag). We keep IDs, matching the one documented-working recipe. Recorded
   as a divergence to revisit if pairing 400s appear on the ChatGPT backend specifically.

---

## Task 1: Request `summary` + `include`, and capture reasoning items verbatim

**Files:**
- Modify: `crates/rupu-providers/src/openai_codex.rs` — the reasoning request block (~:305-323),
  `parse_response` (~:811-873), `process_sse_event` (~:340-451), and the stream accumulator
- Test: same file, `mod tests`

**Interfaces — Produces:** `pub(crate) const PROVIDER_TAG: &str = "openai_responses";` and a
`ContentBlock::Reasoning` carrying `raw = {"output": [<all output items verbatim>]}`.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn request_includes_encrypted_content_and_summary() {
    // body["include"] == ["reasoning.encrypted_content"]
    // body["reasoning"]["summary"] == "auto"   (alongside the existing effort)
    // body["store"] == false  (unchanged)
    // Without summary:"auto" the summary array comes back empty and the
    // transcript would show nothing readable — this is the analogue of
    // Anthropic's display:"summarized".
}

#[test]
fn request_sets_summary_even_without_explicit_effort() {
    // ThinkingLevel::Auto omits `reasoning.effort` (server default) — assert
    // `summary` is still requested, i.e. reasoning capture doesn't depend on
    // an explicit effort being set.
}

#[test]
fn parse_captures_reasoning_item_verbatim() {
    // output: [ {"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"planning"}],
    //            "encrypted_content":"enc_abc"},
    //           {"type":"function_call","call_id":"c1","name":"f","arguments":"{}"} ]
    // ->
    //   exactly one ContentBlock::Reasoning, provider "openai_responses"
    //   text == Some("planning")            (from summary_text)
    //   raw["output"] == the FULL original output array, verbatim, BOTH items,
    //                     encrypted_content and ids intact
    //   the ToolUse block is still emitted as today
}

#[test]
fn parse_captures_reasoning_with_empty_summary() {
    // Unverified orgs / summary-less responses: summary: [] ->
    // text: None, but the item is STILL captured (encrypted_content must
    // round-trip; an unreadable blob is still echo-able).
}

#[test]
fn parse_without_reasoning_items_still_stores_output_for_replay() {
    // Pairing is enforced in BOTH directions ("'function_call' was provided
    // without its required 'reasoning' item"), so a turn's output is stored
    // for replay whenever there are output items at all.
}

#[test]
fn parse_with_empty_output_emits_no_reasoning_block() {
    // Nothing to replay.
}

#[test]
fn sse_captures_reasoning_item_from_output_item_done() {
    // THE streaming bug: `encrypted_content` arrives on
    // response.output_item.done for a reasoning item, NOT on the text deltas.
    // Today output_item.added/done are filtered to function_call only, so the
    // encrypted content is dropped. Assert the reasoning item lands in
    // raw["output"] verbatim.
}

#[test]
fn sse_emits_reasoning_delta_from_summary_text_delta() {
    // response.reasoning_summary_text.delta -> StreamEvent::ReasoningDelta,
    // and NOT a TextDelta.
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- openai_codex` → FAIL.

- [ ] **Step 3: Implement the request side.** Add near the top:

```rust
/// Canonical tag for the OpenAI Responses API wire format.
///
/// Deliberately distinct from `openai_wire`'s `openai_chat`: chat-completions
/// and Responses are different wire formats, and a reasoning payload from one
/// must never be echoed to the other.
pub(crate) const PROVIDER_TAG: &str = "openai_responses";
```

In the reasoning request block (~:305-323), keep the existing `effort` mapping **exactly as-is** and add
`summary`, plus `include` at the top level:

```rust
    // `summary: "auto"` is the analogue of Anthropic's display:"summarized" —
    // without it `summary` comes back empty and the transcript has nothing
    // readable. NOTE: OpenAI may require organization verification before
    // summaries are available on its latest reasoning models.
    //
    // `include: ["reasoning.encrypted_content"]` is now legacy (stateless mode
    // returns encrypted_content by default) but is still accepted, and
    // openai/codex sends it unconditionally — keep it for Azure/proxy backends
    // that haven't adopted the new default.
```

Set `body["reasoning"]["summary"] = "auto"` **whether or not** an explicit effort was set (the
`ThinkingLevel::Auto` path currently omits the whole `reasoning` object — it must still request a
summary), and `body["include"] = ["reasoning.encrypted_content"]`. Read the real block and adapt; do not
transcribe blindly. Leave `store: false` alone.

- [ ] **Step 4: Implement the non-streaming capture.** In `parse_response`, collect every output item
  verbatim into `raw_output: Vec<Value>` as you iterate (before the existing `match item["type"]`), and
  accumulate summary text from reasoning items:

```rust
    // Concatenate summary_text entries for the transcript. `raw` is what goes
    // back on the wire; this text is display only.
```

Keep the existing `"message"` / `"function_call"` arms unchanged; the `_ => {}` arm stops being a silent
drop only in the sense that the item is already captured into `raw_output` above it. Then insert at
index 0:

```rust
    if !raw_output.is_empty() {
        content.insert(0, ContentBlock::Reasoning {
            text: if summary_text.is_empty() { None } else { Some(summary_text) },
            provider: PROVIDER_TAG.to_string(),
            model: model.to_string(),
            raw: serde_json::json!({ "output": raw_output }),
        });
    }
```

Thread the model in if it is not already in scope (Gemini's Plan 2 did the same) — read the call site.

- [ ] **Step 5: Implement the streaming capture.** Add `raw_output: Vec<Value>` and
  `reasoning_summary: String` to the accumulator. In `process_sse_event`:
  - `response.output_item.done`: push the item verbatim into `acc.raw_output` for **every** item type —
    today it is filtered to `function_call`, which is exactly why `encrypted_content` is lost. Keep the
    existing `function_call` handling intact.
  - `response.reasoning_summary_text.delta`: append to `acc.reasoning_summary` and emit
    `StreamEvent::ReasoningDelta`.
  - Leave every other event's behavior unchanged.
  Then build the same `Reasoning` block at index 0 when finalizing. Find the finalize site by reading
  the file.

- [ ] **Step 6:** `cargo test -p rupu-providers --lib -- openai_codex` → PASS; whole crate green.
- [ ] **Step 7:** rustfmt; `cargo clippy -p rupu-providers --no-deps`; `git status --short`; commit:

```bash
git commit -m "feat(providers): capture Responses API reasoning items + request summaries"
```

---

## Task 2: Replay reasoning items into `input`

**Files:**
- Modify: `crates/rupu-providers/src/openai_codex.rs` — `build_request_body` (~:207-272), and the
  `ContentBlock::Reasoning { .. }` placeholder arm (~:264)
- Test: same file, `mod tests`

**Interfaces — Consumes:** Task 1's `PROVIDER_TAG` and `raw["output"]`.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn assistant_turn_replays_stored_output_items_verbatim() {
    // History: assistant Message with
    //   Reasoning{provider:"openai_responses", raw:{"output":[
    //      {"type":"reasoning","id":"rs_1","encrypted_content":"enc_abc","summary":[]},
    //      {"type":"function_call","call_id":"c1","name":"f","arguments":"{}"}]}}
    //   + ToolUse{id:"c1", name:"f", input:{}}
    // -> body["input"] contains BOTH items verbatim (encrypted_content and
    //    id intact), in order, and the function_call appears exactly ONCE
    //    (not duplicated by the ToolUse also being rebuilt).
}

#[test]
fn replay_keeps_reasoning_adjacent_to_its_function_call() {
    // The pairing errors fire in BOTH directions, so assert the reasoning item
    // immediately precedes its function_call exactly as the server emitted.
}

#[test]
fn replay_preserves_item_ids() {
    // The pairing errors are expressed in terms of item IDs
    // ("Item 'rs_...' ..."), so IDs must survive verbatim.
}

#[test]
fn falls_back_to_rebuild_without_reasoning_block() {
    // Backward compat: today's body, unchanged.
}

#[test]
fn foreign_provider_reasoning_is_not_replayed() {
    // Reasoning{provider:"anthropic"| "openai_chat", ...} must NOT be replayed
    // — different wire formats. Falls back to rebuild.
    // Note openai_chat is Plan 3's chat-completions tag: same vendor, DIFFERENT
    // wire format. It must not cross.
}

#[test]
fn malformed_raw_falls_back_to_rebuild() {
    // raw missing "output", or "output" not an array -> rebuild, with a debug!
    // so the resulting 400 is diagnosable.
}

#[test]
fn internal_fields_never_reach_the_wire() {
    // No "provider"/"model"/"raw"/"reasoning"-typed internal block in the body.
}
```

- [ ] **Step 2:** run → FAIL.

- [ ] **Step 3: Implement.** In `build_request_body`'s per-message loop, before building items from
  blocks, check for a replay block — mirroring what Plan 2 did in `google_gemini.rs::convert_messages`
  (read that for the established shape):

```rust
        // Replay the server's own output items verbatim. This is what keeps a
        // reasoning item adjacent to its function_call with IDs intact — the
        // Responses API 400s in BOTH directions on a broken pairing, and the
        // adjacency rule is undocumented, so replaying exactly what arrived is
        // the only recipe known to work.
        //
        // Gate on the provider tag only: a chat-completions or Anthropic
        // payload is a different wire format and must never be replayed here.
```

If a usable replay block is present, extend `input` with those items verbatim and `continue` (skipping
the rebuild for that message, so nothing duplicates). Otherwise fall through to the existing rebuild.
Emit a `debug!` when a tag-matched block has unusable `raw`.

- [ ] **Step 4:** whole crate green.
- [ ] **Step 5:** rustfmt; clippy; `git status --short`; commit:

```bash
git commit -m "fix(providers): replay Responses API reasoning items verbatim into input"
```

---

## Self-Review

**Spec coverage:** `include` + `summary:"auto"` → T1; reasoning items captured verbatim (both the
non-streaming drop and the `output_item.done` streaming drop) → T1; replayed into `input` preserving
pairing + IDs → T2; provider-tag-only gate, distinct from `openai_chat` → T2. Out-of-scope items
(effort per-model validity, `store:true`+`previous_response_id`, Codex's ID-stripping) recorded.

**Type flow:** `PROVIDER_TAG` + `Reasoning{raw:{output}}` (T1) → replayed by `build_request_body` (T2).
T1 is inert alone; T2 activates it. Sequential.

**Risk to flag in the PR:** `summary: "auto"` may require OpenAI **organization verification** on the
latest reasoning models. If matt's org isn't verified this could 400 — it is a one-line revert, and
without it the transcript captures only opaque blobs. Call it out explicitly.

## Execution

Subagent-driven: T1 → review → T2 → review → final whole-branch review → PR targeting
**`reasoning-plan-3`**. No self-merge; matt reviews.
