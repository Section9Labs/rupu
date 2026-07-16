# rupu — Capture model reasoning across all providers

Status: Approved (design) — representation resolved 2026-07-16
Date: 2026-07-16

## Context

rupu discards every model's reasoning/thinking output, on every provider. Verified in-code:

- **`ContentBlock`** (`crates/rupu-providers/src/types.rs:14-32`) — the single type every provider's
  response funnels through (`LlmResponse.content: Vec<ContentBlock>`, `:196`) — has exactly three
  variants: `Text`, `ToolUse`, `ToolResult`. Reasoning has nowhere to land, so it is dropped at parse.
- **`StreamEvent`** (`types.rs:222-231`) has four variants (`TextDelta`, `UsageSnapshot`,
  `ToolUseStart`, `InputJsonDelta`) — no reasoning delta exists in the streaming protocol.
- Every provider already *asks* for reasoning on the request side and then throws the response away:
  - `anthropic.rs:1210-1242` sends `thinking: {"type":"adaptive"}` (and never sets `display`);
    the SSE handler has no `thinking`/`redacted_thinking` branch (`:1344-1371`), and
    `thinking_delta`/`signature_delta` fall into `_ => debug!("unknown delta type")` (`:1393`).
  - `google_gemini.rs:325-360` sends `thinkingConfig.includeThoughts: true` — explicitly asking the
    server to emit thoughts — then **skips every `thought: true` part** (`:588-591` non-streaming,
    `:716-719` streaming). We pay for thoughts on every call and discard them. No `thoughtSignature`
    handling exists anywhere in the file.
  - `openai_codex.rs:305-323` sends `reasoning: {effort}`; the response parse handles only
    `message` / `function_call` and drops `reasoning` items via `_ => {}` (`:846`).
  - `openai_wire.rs` (shared by `github_copilot.rs` + `openai_compatible.rs`) sends
    `reasoning_effort` but **never reads `message.reasoning_content`** — the de-facto field for
    DeepSeek-R1 / Qwen QwQ / GLM / vLLM reasoning deployments.
  - `local.rs` ignores `request.thinking` entirely.
- **The render path already exists.** `Event::AssistantMessage { content, thinking: Option<String> }`
  (`crates/rupu-transcript/src/event.rs:28-32`) is already consumed and rendered by
  `rupu-cli/src/cmd/session.rs:2299-2525`, `cmd/transcript.rs:664-674`, and
  `output/workflow_printer.rs:1247-1257`. The **only** production write site,
  `rupu-agent/src/runner.rs:1085`, hardcodes `thinking: None`. **The gap is capture, not display.**

Two latent bugs surfaced while mapping this:

1. **Anthropic non-streaming deserialize failure.** `send()` does
   `response.json::<AnthropicResponse>()` (`anthropic.rs:941`) where `content: Vec<ContentBlock>` is a
   strictly-tagged enum with no catch-all. Thinking is enabled on every request, so a `thinking` block
   in a non-streaming response **fails to deserialize** ("unknown variant"). Streaming silently drops;
   non-streaming errors. `broker_client.rs:95` has the same strict-deserialize shape.
2. **`local.rs:96-99`** (`extract_text_content`) drops `ToolUse`/`ToolResult` too, not just reasoning —
   multi-turn tool loops are already lossy there. Out of scope; recorded as a known defect.

### Why "summarized" and not "full"

`thinking.display` accepts exactly `"summarized"` or `"omitted"`. **There is no `"full"`/`"raw"` — the
raw chain of thought is not exposed on any Claude model.** `"summarized"` is the ceiling of available
fidelity. `display` controls visibility only: thinking happens and is billed identically either way.
The default is `"omitted"` (empty thinking text) on Opus 4.7/4.8 and Sonnet 5, and `"summarized"` on
Opus 4.6 / Sonnet 4.6. So on today's newer models, capturing without setting `display` would capture
empty strings — rupu must opt in explicitly.

## Goal

Capture each turn's reasoning on **every** provider, persist it into the transcript
(`AssistantMessage.thinking`, already rendered), and **echo the provider's opaque continuity token
back verbatim** on the next turn so multi-turn tool loops stay correct.

## Non-goals

- Raw/unsummarized chain of thought — does not exist on Claude (see above).
- New rendering. The transcript/session/workflow-printer render paths already exist and are used.
- A new transcript event vocabulary. Reuse `AssistantMessage.thinking`.
- Fixing `local.rs`'s pre-existing `ToolUse`/`ToolResult` loss.
- Cross-provider reasoning translation. Reasoning is echoed only to its producer.

## Architecture

### The shared block (`rupu-providers/src/types.rs`)

```rust
#[serde(rename = "reasoning")]
Reasoning {
    /// Human-readable summary for the transcript/UI. `None` when the provider
    /// returns no readable text (redacted/encrypted blocks, or display="omitted").
    text: Option<String>,
    /// Canonical tag of the provider that produced this block.
    provider: String,
    /// Model that produced it. Continuity tokens are model-bound.
    model: String,
    /// The provider's original block, echoed back verbatim to that provider.
    raw: serde_json::Value,
},
```

Plus a forward-compatibility catch-all so an unknown block type never fails a whole turn again:

```rust
#[serde(other)]
Unknown,
```

**Echo rule:** a provider serializes `raw` verbatim **iff** `provider == <its own tag>` **and**
`model == the model of the current request`; otherwise it drops the block. This mirrors API reality
(Anthropic rejects tampered signatures; other models silently drop foreign thinking) and guarantees an
Anthropic `signature` can never leak into a Gemini request. Canonical tags: `anthropic`,
`google_gemini`, `openai_codex`, `openai_chat` (the shared `openai_wire` dialect — Copilot and
OpenAI-compatible interoperate within it), `local`.

**`ContentBlock`'s serde is rupu's internal persistence format, not any provider's wire format.** This
is the load-bearing correction: `anthropic.rs` today builds its request body by *generic*
`serde_json::to_value(&request.messages)` (`:1110`), which works only by the coincidence that our
three variants happen to match Anthropic's wire shapes. A `reasoning`-tagged block would be sent to
Anthropic as-is and rejected. Each provider therefore owns translation in **both** directions:

- **Out:** Anthropic post-processes the generic serde output to rewrite `Reasoning` → its native
  `{"type":"thinking",…}` / `{"type":"redacted_thinking",…}` from `raw`, alongside the existing
  `sanitize_messages_tool_names` post-process (`:44`) — the established precedent for exactly this.
  Every other provider already hand-matches `ContentBlock` exhaustively and gains an explicit arm.
- **In:** Anthropic's non-streaming `send()` stops relying on derive-`Deserialize` for `content` and
  parses blocks explicitly (`Vec<Value>` → `Vec<ContentBlock>`), which also fixes latent bug #1.
  Without this the `#[serde(other)]` catch-all would silently map a real `thinking` block to `Unknown`.

`LlmResponse::text()`/`tool_calls()` are unaffected (`Reasoning` is neither). Add
`LlmResponse::reasoning_text()` returning the concatenated `text` of the turn's reasoning blocks.

### Streaming

Add `StreamEvent::ReasoningDelta(String)`, symmetric with `TextDelta`, so reasoning surfaces
progressively rather than only at block completion.

### Agent runtime (`rupu-agent/src/runner.rs`)

The turn-to-turn echo is **already generic** — `:1223` pushes
`Message { role: Assistant, content: resp.content.clone() }` — so once a provider populates
`Reasoning`, echo works with no new code. Two changes only: an arm in the `match` at `:1081-1101`, and
replacing the hardcoded `thinking: None` (`:1085`) with the turn's `reasoning_text()`.

### Per-provider work

| Provider | Request change | Response change |
|---|---|---|
| `anthropic` | set `display: "summarized"` — **only** on the `{"type":"adaptive"}` path (4.6+), never on the legacy `budget_tokens` path, which predates `display` | parse `thinking` (text + `signature`) and `redacted_thinking` (`data`) in both SSE (`content_block_start`/`thinking_delta`/`signature_delta`/`content_block_stop`) and the explicit non-streaming parse |
| `google_gemini` | none (`includeThoughts: true` already sent) | capture `thought: true` parts as `Reasoning` instead of `continue`; capture and echo `thoughtSignature` — this closes a real multi-turn function-calling correctness gap per Google's contract, and stops paying for discarded thoughts |
| `openai_wire` (Copilot + OpenAI-compatible) | none | read `message.reasoning_content` and `delta.reasoning_content` |
| `openai_codex` | needs `include: ["reasoning.encrypted_content"]` — with `store: false` (`:276`) and no `include`, the API returns nothing echo-able today | parse `reasoning` output items + `response.reasoning_summary_text.delta` |
| `local` | none | none — its `_ => None` wildcard already ignores the new variant inertly |
| `broker_client` | none (forwards `thinking` through) | same explicit-parse safety as Anthropic's non-streaming path |

## Errors & safety

- Reasoning capture is **best-effort and never fails a turn**: an unparseable reasoning block is
  dropped with a `debug!`, never an error.
- The `#[serde(other)] Unknown` catch-all makes future block types degrade instead of erroring.
  **Known limitation:** it does not help *older* binaries reading *newer* artifacts — a pre-change
  rupu reading a transcript or broker payload containing `reasoning` blocks will still fail to
  deserialize. Broker and transcripts are local/internal; accepted and documented.
- Continuity tokens (`signature`, `thoughtSignature`, `encrypted_content`) are echoed **byte-exact**;
  they are never parsed, edited, or synthesized.
- Reasoning text is persisted to on-disk transcripts — that is the intent of the feature.
  `redacted_thinking` carries no readable text (`text: None`); only its opaque `data` round-trips.
- `#![deny(clippy::all)]`; no `unsafe`; `thiserror`; workspace deps only. Per-file rustfmt.

## Testing

- **Shared:** `Reasoning` serde round-trip; `Unknown` catch-all absorbs an unknown tagged block
  instead of erroring; `text()`/`tool_calls()` unaffected; `reasoning_text()` concatenates.
- **Anthropic:** SSE fixture with `thinking` + `signature_delta` → a `Reasoning` block with the
  signature intact in `raw`; a `redacted_thinking` fixture → `text: None`, `data` preserved;
  **non-streaming fixture containing a thinking block deserializes** (regression test for latent bug
  #1); request body carries `display: "summarized"` on the adaptive path and **not** on the
  `budget_tokens` path; a `Reasoning` block in outgoing history is rewritten to `{"type":"thinking"}`
  verbatim; a foreign-provider or foreign-model `Reasoning` block is dropped from the request.
- **Gemini:** `test_parse_response_skips_thinking` (`:1125`) currently **asserts the drop** — it flips
  to assert capture; this is a deliberate behavior change, not an extension. Plus `thoughtSignature`
  capture + echo.
- **openai_wire / codex:** net-new — no response-side reasoning test exists for either.
- Existing request-side thinking tests (`anthropic.rs:2100-2330`, `google_gemini.rs:962/992/1308`,
  `openai_codex.rs:1126/1156`, `github_copilot.rs:516`) stay green.

## Decomposition (plans)

Each is an independent PR. Additive everywhere except the compile-forced arms, so each lands alone.

- **Plan 1 — shared type + Anthropic + the latent non-streaming fix.** Adds `Reasoning`, `Unknown`,
  `ReasoningDelta`, `reasoning_text()`, the runner arm + `thinking` population, Anthropic both
  directions, `display: "summarized"`, and the explicit arms every other provider needs to compile
  (drop-only for now). After this PR, Anthropic reasoning is captured and rendered end-to-end.
- **Plan 2 — Gemini:** capture thoughts + thought-signature echo.
- **Plan 3 — `openai_wire`:** `reasoning_content` for Copilot + OpenAI-compatible. Likely the widest
  real-model impact (DeepSeek-R1 / Qwen / GLM / local vLLM).
- **Plan 4 — `openai_codex`:** `include: ["reasoning.encrypted_content"]` + reasoning-item parse.

## Resolved design decisions (2026-07-16)

- **Representation — RESOLVED:** provider-agnostic `Reasoning { text, provider, model, raw }` with an
  opaque provider-tagged payload. Rejected: Anthropic-shaped `Thinking`/`RedactedThinking` variants —
  they buy a free Anthropic round-trip (generic serde) but make Anthropic's field names the shared
  vocabulary, forcing Gemini thought-signatures and OpenAI reasoning items into fields whose semantics
  don't match. Rejected: Anthropic-shaped now + refactor later — migrates every `ContentBlock` match
  site twice.
- **Echo scope — RESOLVED:** echo `raw` only when both the provider tag **and** the model match;
  otherwise drop. Model-matching is cheap insurance against signature-verification 400s.
- **Wire coupling — RESOLVED:** `ContentBlock` serde is rupu-internal; Anthropic's incidental reliance
  on it for wire format is removed in both directions rather than preserved.
