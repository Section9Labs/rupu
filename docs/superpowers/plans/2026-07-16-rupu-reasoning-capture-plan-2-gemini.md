# Reasoning capture — Plan 2 (Gemini)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Capture Gemini's thought summaries into the transcript **and fix Gemini multi-turn function
calling**, which is currently broken: rupu never returns `thoughtSignature`, and omitting it on the
first `functionCall` part is a documented hard 400.

**Architecture:** Gemini's parse stores the assistant turn's **original `parts` array verbatim** in a
single `ContentBlock::Reasoning`'s opaque `raw`; `convert_messages` replays those parts verbatim
instead of rebuilding them from blocks. Signatures land on exactly the parts they arrived on, with **no
change to the shared `ContentBlock`** — all of this stays inside `google_gemini.rs`.

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-reasoning-capture-design.md` — **read the "Gemini
addendum (Plan 2)" section**, it supersedes the spec's original one-line Plan 2 description.

**Tech Stack:** Rust 2021, `serde_json`, `tokio`, `tracing`.

**Base:** branch `reasoning-plan-2` off `reasoning-capture` (Plan 1, PR #482 — not yet merged).
Plan 1's `ContentBlock::Reasoning` / `Unknown` / `StreamEvent::ReasoningDelta` / `reasoning_text()` are
already available.

## Global Constraints

- **The signature must return on the exact part it arrived on.** *"You must return this signature in
  the exact part where it was received."* Omitting it on the first `functionCall` part is a **hard
  400**; omitting elsewhere is silent degradation. Never edit, reorder, or synthesize a signature.
- **Verbatim replay means verbatim.** Replay the stored parts array as-is, including `thought: true`
  parts. This matches what Google's own SDKs do (append the model's `content` straight into
  `contents`). Do not filter parts — a filter risks dropping a signed part (Gemini 2.5 signs "the
  first part regardless of type").
- **No change to the shared `ContentBlock`** and no change to any other provider. The meaning of `raw`
  is provider-private; everything here is local to `google_gemini.rs`.
- Provider tag: `google_gemini`. The echo gate is the **provider tag only, never the model** — a
  foreign-provider block is dropped; a same-provider block is replayed. (Cross-model replay is
  explicitly UNVERIFIED for Gemini — we neither special-case nor guard it. Do not add a model check.)
- `Text`/`ToolUse` blocks keep being emitted exactly as today — rupu's transcript, tool dispatch,
  `text()` and `tool_calls()` all depend on them. The `Reasoning` block is **additive**.
- Backward compatible: a turn with no reasoning block replays through the existing rebuild path
  unchanged.
- Capture is best-effort: a malformed part is dropped with `debug!`, never an error.
- `#![deny(clippy::all)]`; no `unsafe`; workspace deps only (never pin in a crate Cargo.toml).
- **Per-file rustfmt only:** `rustfmt --edition 2021 crates/rupu-providers/src/google_gemini.rs`
  (not a mod root). Never `cargo fmt`, never a `lib.rs`/mod root. `git status --short` before each
  commit; `git restore` stray drift by name.
- Homebrew toolchain 1.95 vs repo-pinned 1.88: pre-existing lints in untouched files are not yours.
- **No vacuous tests.** A test that re-implements production logic locally and asserts against its own
  copy is worse than no test — one was caught in Plan 1. Tests must drive the real functions.

## OUT OF SCOPE (recorded in the spec; do NOT fix here)

Three separate pre-existing Gemini bugs, all independent of reasoning:
1. `thinkingLevel` + `thinkingBudget` sent together (`:353-357`) — cannot coexist on Gemini 3 → 400.
2. Level values sent uppercase (`"MINIMAL"`); docs specify lowercase.
3. `usageMetadata.thoughtsTokenCount` not read.

Do not touch the request-side `thinkingConfig` at all in this plan.

---

## Task 1: Capture thought parts + store the verbatim parts array

**Files:**
- Modify: `crates/rupu-providers/src/google_gemini.rs` — `parse_generate_content_response` (`:574-634`),
  `process_gemini_sse` (`:696+`), `GeminiAccumulator`, and the `send()` call site that invokes the parse
- Test: same file, `mod tests`

**Interfaces — Produces:** `pub(crate) const PROVIDER_TAG: &str = "google_gemini";` and a
`ContentBlock::Reasoning` block appended to each assistant turn that has parts.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn parse_response_captures_thinking_as_reasoning_block() {
    // REPLACES test_parse_response_skips_thinking (:1127), which asserts the
    // OLD drop behavior (content.len() == 1). This is a deliberate behavior
    // change: that test must flip from asserting the drop to asserting capture.
    // Given parts [{thought:true,text:"Let me think..."}, {text:"The answer is 42."}]:
    //   - response.text() == Some("The answer is 42.")   (unchanged)
    //   - exactly one ContentBlock::Reasoning with provider "google_gemini"
    //   - its text == Some("Let me think...")
    //   - its raw["parts"] == the ORIGINAL parts array, verbatim (both parts)
}

#[test]
fn parse_response_preserves_thought_signature_in_raw() {
    // parts: [{"functionCall":{"name":"f","args":{}},"thoughtSignature":"sig_abc"}]
    // Assert raw["parts"][0]["thoughtSignature"] == "sig_abc" — the signature
    // survives into raw, which is what Task 2 replays. Also assert the ToolUse
    // block is still emitted as today.
}

#[test]
fn parse_response_without_thoughts_still_stores_parts_for_replay() {
    // Gemini 3 signs the first functionCall part even with no thought parts, so
    // a turn with no thoughts STILL needs its parts stored for replay.
    // Assert: a Reasoning block exists with text: None and raw["parts"] present.
}

#[test]
fn parse_response_with_no_parts_emits_no_reasoning_block() {
    // Empty/absent parts -> no Reasoning block (nothing to replay).
}

#[test]
fn sse_captures_thought_parts_and_emits_reasoning_delta() {
    // Streaming: a chunk with {"thought":true,"text":"thinking..."} emits
    // StreamEvent::ReasoningDelta("thinking...") and NOT a TextDelta.
}

#[test]
fn sse_accumulates_all_parts_verbatim_across_chunks() {
    // Two chunks; assert the final Reasoning block's raw["parts"] contains every
    // part from both chunks, in arrival order, unmodified (signatures intact).
}

#[test]
fn sse_thought_part_with_function_call_still_yields_tool_use() {
    // Regression guard: the old code `continue`d on thought:true BEFORE checking
    // for a functionCall on that same part, so a part carrying both lost the tool
    // call entirely. Assert the ToolUse block is emitted.
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- google_gemini` → new tests FAIL.

- [ ] **Step 3: Implement the non-streaming parse.** Add near the top of the file:

```rust
/// Canonical provider tag stamped on Reasoning blocks; gates the replay.
pub(crate) const PROVIDER_TAG: &str = "google_gemini";
```

`parse_generate_content_response` currently takes only `json`. It needs the model (the `Reasoning`
block records it, and `LlmResponse.model` is currently left empty). Add a `model: &str` parameter and
pass `&request.model` from the `send()` call site — read the call site first and match its style.

Rewrite the parts loop so it (a) no longer `continue`s past thought parts, (b) collects every part
verbatim, and (c) accumulates thought text:

```rust
    let mut raw_parts: Vec<serde_json::Value> = Vec::new();
    let mut thought_text = String::new();
    // ... inside the `for part in parts` loop, FIRST:
    raw_parts.push(part.clone());

    if part.get("thought").and_then(|t| t.as_bool()) == Some(true) {
        // A thought part: its text is reasoning, not answer text. Collect it for
        // the transcript, but do NOT emit a Text block. The part itself is already
        // in raw_parts and replays verbatim (it may carry a signature).
        if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
            if !thought_text.is_empty() {
                thought_text.push_str("\n\n");
            }
            thought_text.push_str(t);
        }
        continue;
    }
    // ... existing text / functionCall handling unchanged below
```

Keep the existing `text` and `functionCall` handling exactly as-is for non-thought parts.

After the loop, append the replay block (reasoning first, mirroring Anthropic's ordering and so
`reasoning_text()` reads naturally — note Gemini's own wire order comes from `raw["parts"]`, so block
order here is for rupu's internal consistency only):

```rust
    // Store the turn's parts verbatim so convert_messages can replay them.
    // This is what carries thoughtSignature back on the exact part it arrived
    // on — omitting it on the first functionCall part is a hard 400.
    if !raw_parts.is_empty() {
        content.insert(
            0,
            ContentBlock::Reasoning {
                text: if thought_text.is_empty() { None } else { Some(thought_text) },
                provider: PROVIDER_TAG.to_string(),
                model: model.to_string(),
                raw: serde_json::json!({ "parts": raw_parts }),
            },
        );
    }
```

- [ ] **Step 4: Implement the streaming parse.** Add to `GeminiAccumulator`:

```rust
    raw_parts: Vec<serde_json::Value>,
    thought_text: String,
    model: String,
```

In `process_gemini_sse`, replace the `continue`-on-thought with the same shape: push every part into
`acc.raw_parts` first; for a thought part accumulate `acc.thought_text` and emit
`on_event(StreamEvent::ReasoningDelta(text.to_string()))` instead of `TextDelta`, then `continue`.
Leave the non-thought text/functionCall handling as-is.

Set `acc.model` from the request where the accumulator is constructed (read `stream()` and match how
other fields get seeded). Then, where the accumulator builds its `LlmResponse`, insert the same
`Reasoning` block at index 0 built from `acc.raw_parts` / `acc.thought_text`. Find that construction
site by reading the file — do not guess.

- [ ] **Step 5:** `cargo test -p rupu-providers --lib -- google_gemini` → PASS, and the whole
  `cargo test -p rupu-providers --lib` stays green.

- [ ] **Step 6:** rustfmt the file; `cargo clippy -p rupu-providers --no-deps`; `git status --short`; commit:

```bash
git commit -m "feat(providers): capture Gemini thought parts + store verbatim parts for replay"
```

---

## Task 2: Replay the verbatim parts (fixes the multi-turn 400)

**Files:**
- Modify: `crates/rupu-providers/src/google_gemini.rs` — `convert_messages` (`:504-569`)
- Test: same file, `mod tests`

**Interfaces — Consumes:** Task 1's `PROVIDER_TAG` and the `Reasoning` block carrying `raw["parts"]`.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn convert_messages_replays_stored_parts_verbatim() {
    // An assistant Message containing:
    //   Reasoning{provider:"google_gemini", raw:{"parts":[
    //       {"functionCall":{"name":"f","args":{"x":1}},"thoughtSignature":"sig_abc"}]}}
    //   + ToolUse{id:"gemini_tc_1", name:"f", input:{"x":1}}
    // -> contents[0]["parts"] == the stored parts VERBATIM, signature intact,
    //    and the functionCall appears exactly ONCE (not duplicated by the
    //    ToolUse block also being rebuilt).
}

#[test]
fn convert_messages_replay_preserves_thought_signature_on_its_part() {
    // The whole point: assert the signature is still attached to the SAME part
    // it arrived on, not hoisted or moved.
}

#[test]
fn convert_messages_falls_back_to_rebuild_without_reasoning_block() {
    // Backward compat: an assistant message with no Reasoning block converts
    // exactly as today (existing behavior unchanged).
}

#[test]
fn convert_messages_drops_foreign_provider_reasoning_block() {
    // Reasoning{provider:"anthropic", raw:{"type":"thinking",...}} in history ->
    // NOT replayed (it is an alien wire format), and the message falls back to
    // the rebuild path. An Anthropic signature must never reach Gemini.
}

#[test]
fn convert_messages_ignores_replay_block_with_malformed_raw() {
    // raw missing "parts", or "parts" not an array -> fall back to rebuild
    // rather than sending garbage.
}

#[test]
fn convert_messages_still_maps_tool_result_names_with_replay_present() {
    // The user-side functionResponse name lookup uses tool_name_map, which is
    // built from ToolUse blocks. Assert replay on the assistant turn does not
    // break the following user turn's functionResponse naming.
}
```

- [ ] **Step 2:** `cargo test -p rupu-providers --lib -- google_gemini` → FAIL.

- [ ] **Step 3: Implement.** In `convert_messages`, keep the `tool_name_map` pre-scan exactly as-is
  (the user-side `functionResponse` lookup still needs it). Inside the per-message loop, before
  building `parts`, check for a replay block:

```rust
        // If this turn carries a verbatim parts replay from Gemini, send those
        // parts back exactly as they arrived. This is what returns
        // thoughtSignature on the exact part it was received on — Google
        // requires it, and omitting it on the first functionCall part is a hard
        // 400. Google's own SDKs replay the model's content the same way.
        //
        // The gate is the provider tag only: a foreign provider's block is an
        // alien wire format and is ignored (falling back to the rebuild).
        let replay = msg.content.iter().find_map(|b| match b {
            ContentBlock::Reasoning { provider, raw, .. } if provider == PROVIDER_TAG => {
                raw.get("parts").and_then(|p| p.as_array()).cloned()
            }
            _ => None,
        });
        if let Some(parts) = replay {
            if !parts.is_empty() {
                contents.push(serde_json::json!({"role": role, "parts": parts}));
                continue;
            }
            debug!("gemini replay block had empty parts; rebuilding from blocks");
        }
```

Everything below stays as the existing rebuild path (the fallback). Leave the
`ContentBlock::Reasoning { .. } => {}` arm in the rebuild match as a no-op drop and update its comment:
a reasoning block is either consumed by the replay above or (foreign provider) deliberately ignored.

- [ ] **Step 4:** `cargo test -p rupu-providers --lib -- google_gemini` → PASS; whole
  `cargo test -p rupu-providers --lib` green.

- [ ] **Step 5:** rustfmt; `cargo clippy -p rupu-providers --no-deps`; `git status --short`; commit:

```bash
git commit -m "fix(providers): replay Gemini parts verbatim so thoughtSignature returns on its part"
```

---

## Self-Review

**Spec coverage (Gemini addendum):** verbatim parts stored in `raw` → T1; replayed on the wire → T2;
thought summaries into `reasoning_text()`/transcript → T1 (Plan 1's runner already writes it — no
rupu-agent change needed); the documented 400 fixed → T2; provider-tag-only gate → T2's foreign-provider
test. Out-of-scope Gemini bugs (thinkingLevel/budget coexistence, uppercase levels, thoughtsTokenCount)
are recorded in the spec and deliberately untouched.

**Type flow:** `PROVIDER_TAG` + `Reasoning{raw:{parts}}` (T1) → consumed by `convert_messages` (T2).
T1 is inert on its own (a stored block nobody reads); T2 activates it. Sequential.

**Behavior changes (deliberate):** `test_parse_response_skips_thinking` flips from asserting the drop to
asserting capture. Assistant turns that carry a replay block now go on the wire as the stored parts
rather than a rebuild — that is the fix.

**No new crates, no shared-type change, no other provider touched.**

## Execution

Subagent-driven: T1 → review → T2 → review → final whole-branch review → PR targeting
**`reasoning-capture`** (NOT main — Plan 1 is unmerged). No self-merge; matt reviews.
