# rupu response outcomes — design

**Date:** 2026-10-01
**Status:** Design approved in conversation; spec for review
**Scope:** `rupu-providers`, `rupu-runtime`, `rupu-agent`, `rupu-transcript`, `rupu-orchestrator`, `rupu-config`, `rupu-cli`, `rupu-cp` (API + web). The macOS app is deprecated and out of scope.
**Companion to:** the model-limits spec (`2026-09-30-rupu-model-limits-discovery-design.md`), whose §9 deferred this work.
**Builds on:** recover on interrupt (`2026-10-01-rupu-recover-on-interrupt-design.md`). Its PR 1, the continuation primitive (`rupu_agent::continuation`, `rupu run --continue`), merged as #715.

## 1. Problem

rupu discards most of what providers say about why a reply ended. The runner then treats almost every ending as success.

**Providers**
- `StopReason` (`crates/rupu-providers/src/types.rs`) has four variants: `EndTurn`, `MaxTokens`, `StopSequence`, `ToolUse`. It has no fallback variant.
  - Anthropic non-streaming `send` decodes it with derived serde. An unknown value (`refusal`, `pause_turn`, `model_context_window_exceeded`) fails the whole decode as `ProviderError::Http`, and the runner retries that. The compaction summariser uses this path.
  - On the stream, the same value silently becomes `None`.
- Refusal and safety signals map to `EndTurn` everywhere:
  - Gemini `SAFETY`, `RECITATION`, `PROHIBITED_CONTENT` and the rest. A test pins `SAFETY` → `EndTurn`.
  - chat-completions `content_filter`.
  - `message.refusal`.
  - Codex refusal content parts.
- Never read: Anthropic `stop_details` and `pause_turn`, Codex `response.incomplete`, Gemini `promptFeedback.blockReason`.
- Mid-stream error events are dropped:
  - Anthropic SSE `event: error`. A partial reply then comes back as `Ok`.
  - Codex `error` and `response.incomplete`.
  - a chat-completions `{"error":…}` chunk.
  - a Gemini error chunk.
- A tool call whose JSON was cut off by the output cap is handled badly:
  - Anthropic raises `ProviderError::Json`. The runner retries that 10×, and each attempt is billed.
  - chat-completions silently dispatches the tool with `{}`.
- Error bodies are flattened into `Api { status, message }` strings, truncated to 500–4096 bytes. The Anthropic truncation is a byte slice that can panic on a UTF-8 boundary. Overflow detection then string-matches the error's `Display` text.
- `provider_id()` returns `Anthropic` for `local.rs`, `broker_client.rs` and the runner's `MockProvider`.

**Runner**
- `run_agent` ends a run `Ok` whenever a turn has no tool calls, whatever the stop reason. The test `final_answer_without_tool_calls_terminates_regardless_of_stop_reason` pins this.
  - A truncated, refused or blocked answer is therefore a successful run.
  - Its "final output" is the last `AssistantMessage`, which may be the previous turn's interim text when the refused turn produced none.
- Nothing ever continues automatically.
- The compaction summariser never checks its own stop reason.

**Transcript and orchestrator**
- `TurnEnd.stop_reason: Option<String>` is the only stop signal, and nothing reads it.
- `Event::Unknown` drops the payload of events it doesn't know.
- Step failures travel upward as a `success` bool plus a free-form string. `StepResultRecord`, `ItemResult` and the fan-out/parallel outcomes keep no error.

**Renderers**
- About ten CLI and web surfaces render transcripts.
  - Four CLI matches have `_ => {}` arms that drop `Notice`, `TurnEnd`, a failed `RunComplete` and anything new.
  - The live-view dashboard never renders `RunView.error`.
  - `rupu run` prints a "done" footer when no `RunComplete` arrived, and exits 0 on a non-`Ok` terminal status.
- Web:
  - It ignores `stop_reason`.
  - It renders `unknown` blocks as a type name only.
  - Its transcript footer's status glyph matches on statuses that don't exist (`completed`/`failed` vs `ok`/`error`/`aborted`).
  - It hides the footer error when embedded.
  - The run graph drops step error text.

## 2. Goal

Every provider reply is recognized, parsed into a typed form, and rendered in every surface: CLI output, transcripts, and the CP web UI.

- This covers every stop/finish reason, refusals, pauses, incomplete and safety signals, and provider error bodies.
- Anything that is an error is shown as an error everywhere.
- Execution then continues automatically wherever it can, through one recovery ladder.
- No reply falls into an unknown or unrendered bucket.
- A genuinely new shape still parses into a typed "unrecognized" form that carries its raw payload, and is shown.

**This supersedes model-limits §9's sentence** "anything rupu doesn't recognise becomes a surfaced error". Unrecognized replies are not rejected. They are parsed, rendered with their raw payload, and handled by content (§5.1).

**Non-goals**
- Server-side tools (web search, code execution) and their content shapes. rupu sends none. Their blocks parse as typed `Unknown` and are rendered, not interpreted.
- Per-step workflow overrides of the fallback chain (§6). This is a later addition if it's needed.
- Frontmatter overrides of the recovery budgets (§5.2). They are constants in v1.
- The macOS app.

## 3. Decisions (approved)

1. **Recovery ladder.** When rupu can't fix an outcome in place, it climbs one ladder, in this order:
   1. recover with the same provider's model
   2. recover with another provider's model
   3. ask the operator
   4. fail
2. **The ladder applies to every unfixable outcome, not only refusals.** A per-class rule says which rungs apply (§5.2). For example, a dead provider skips straight to another provider.
3. **Rung 1** is:
   - Anthropic server-side `fallbacks: "default"`, on API-key auth only. OAuth impersonates claude-cli's exact `anthropic-beta` list, so it never sends this.
   - then configured same-provider fallback models.
4. **Rung 2** is a configured cross-provider chain. It is opt-in only, because the conversation goes to another vendor.
5. **Rung 3** depends on the context:
   - **Interactive:** `rupu run` with a TTY prompts inline. A foreground `workflow run` parks instead, because its dashboard owns the terminal. The dashboard shows the parked unit and the `rupu workflow recover` command.
   - **Unattended:** workflows under `cp serve`, cron, autoflows and fan-out units park like a gate. The operator decides in the CP or the CLI, and the agent continues from its transcript through the merged continuation primitive. Siblings keep running. A timeout fails the parked attempt.
6. **Architecture (approach A).** Providers parse every reply into a typed outcome. The runner owns classification and the ladder, so every recovery step is written to the transcript and replays exactly. Two alternatives were rejected:
   - provider-internal recovery: invisible to replay, duplicated across six clients, can't cross providers.
   - a `RecoveringProvider` decorator: can't write transcript events or ask the operator.
7. **Errors are always shown as errors,** even when recovery succeeds. The recovery shows as a chip beside the error.

## 4. Provider layer — `rupu-providers`

### 4.1 Typed stop

`LlmResponse.stop_reason: Option<StopReason>` becomes `stop: Stop`:

```rust
pub struct Stop {
    pub reason: StopReason,
    /// Always present: the provider's own words for how the reply ended.
    pub wire: WireStop,
    pub refusal: Option<RefusalDetail>,
    pub served_by: Option<ServedBy>,
}

pub enum StopReason {
    EndTurn,
    ToolUse,
    StopSequence,
    MaxTokens,
    ContextWindowExceeded,
    PauseTurn,
    Refusal,
    Safety,
    MalformedToolCall,
    Incomplete,
    /// The provider reported no stop reason at all.
    Unreported,
    /// A value rupu doesn't know. `wire` carries it.
    Unrecognized,
}

pub struct WireStop {
    pub provider: ProviderId,
    /// e.g. "refusal", "content_filter", "SAFETY", "max_output_tokens"; null when unreported.
    pub value: Option<String>,
    /// Provider extras: stop_sequence, finishMessage, safetyRatings,
    /// incomplete_details, blockReason, truncated_tool, …
    pub details: Option<serde_json::Value>,
}

pub struct RefusalDetail {
    pub category: Option<String>,     // "cyber", "bio", … or null (a valid, permanent value)
    pub explanation: Option<String>,  // display text; never parsed
    pub recommended_model: Option<String>,
    pub source: RefusalSource,        // Classifier | Model | Policy | Unknown
}

pub struct ServedBy {
    pub model: String,
    pub hops: Vec<FallbackHop>,       // { from_model, to_model } per Anthropic `fallback` block
}
```

- `StopReason` gets a tolerant `from_wire` constructor on every path. No provider decodes it with derived serde any more, which fixes the Anthropic `send` decode failure.
- Its serde form, used by transcripts and mock scripts, is `snake_case`, with `#[serde(other)] Unrecognized` on deserialize.
- `MockProvider`'s `ScriptedTurn` can script any `Stop`.

### 4.2 Mapping per provider

Every row below is pinned by an invented fixture test, on both the stream path and the `send` path.

**Anthropic** (`anthropic.rs`)

| Wire | → |
|---|---|
| `end_turn` | `EndTurn` |
| `tool_use` | `ToolUse` |
| `stop_sequence` | `StopSequence`; `details.stop_sequence` holds the matched string |
| `max_tokens` | `MaxTokens` |
| `model_context_window_exceeded` | `ContextWindowExceeded` |
| `pause_turn` | `PauseTurn` |
| `refusal` | `Refusal`, with `stop_details` → `RefusalDetail` |

- Server-side fallback:
  - `fallback` content blocks plus `usage.iterations` entries (`message` / `fallback_message`) → `served_by`.
  - The top-level `model` is the serving model.
  - A response a fallback served ends with the fallback model's own `stop_reason`.
- An SSE `event: error` → `ProviderError::Reply` (§4.4), origin `Stream`.
- A stream that ends after `message_start` without `message_stop` → `ProviderError::IncompleteStream`, not a partial `Ok`.

**Codex / OpenAI Responses** (`openai_codex.rs`)

| Wire | → |
|---|---|
| `response.completed` | `EndTurn`, or `ToolUse` when the output has a `function_call` |
| `response.incomplete` + `incomplete_details.reason` | `max_output_tokens` → `MaxTokens`; `content_filter` → `Safety`; `max_messages` / `steered` / other → `Incomplete` |
| status `cancelled` | `Incomplete` |
| a `refusal` output content part, or `response.refusal.done` | `Refusal`; the refusal text becomes `explanation`, with `source: Model` |
| `response.failed`, or the top-level `error` event | `ProviderError::Reply`. Policy codes (`bio_policy`, `misalignment_policy_violation`) get `ErrorClass::Policy` (§4.4). The invented `Api { status: 500 }` goes away. |

- Unify the precedence of `parse_response` and the stream: a `function_call` means `ToolUse` unless the response is incomplete.

**Chat completions** (`openai_wire.rs`; used by Copilot, OpenAI-compatible/vLLM, and `local.rs`)

| Wire `finish_reason` | → |
|---|---|
| `stop` | `EndTurn` |
| `length` | `MaxTokens` |
| `tool_calls`, `function_call` | `ToolUse` |
| `content_filter` | `Safety` |
| `abort` (vLLM) | `Incomplete` |
| null at end of stream | `Unreported` |
| anything else | `Unrecognized` |

- A non-null `message.refusal` or accumulated `delta.refusal` → `Refusal`, with the refusal text as `explanation`.
- A mid-stream `{"error":…}` chunk → `ProviderError::Reply`.
- Streamed tool arguments that fail to parse never become `{}` again (§4.3).

**Gemini** (`google_gemini.rs`, all three variants)

| Wire `finishReason` | → |
|---|---|
| `STOP` | `EndTurn`, or `ToolUse` when there's a `functionCall` part. This fixes `STOP` overriding `ToolUse`. |
| `MAX_TOKENS` | `MaxTokens` |
| `SAFETY`, `RECITATION`, `LANGUAGE`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `IMAGE_SAFETY`, `IMAGE_PROHIBITED_CONTENT`, `IMAGE_RECITATION`, `IMAGE_OTHER`, `NO_IMAGE` | `Safety`; `details` carries `finishMessage` and `safetyRatings` |
| `MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`, `TOO_MANY_TOOL_CALLS` | `MalformedToolCall` |
| `OTHER` | `Incomplete` |
| `FINISH_REASON_UNSPECIFIED` | `Unreported` |
| unknown | `Unrecognized` |

- `promptFeedback.blockReason` with no candidates → `Safety`, with `details.prompt_blocked: true`. This applies on both `send` and `stream`; today the stream gives `UnexpectedEndOfStream`.
- An error chunk → `ProviderError::Reply`.
- `STOP_SEQUENCE` and `FUNCTION_CALLING` aren't Gemini values and are removed from the table.
- Code Assist variants: the plan verifies against a live call whether replies arrive wrapped in `{ "response": … }`, and unwraps if so.

**Broker / local**
- Parse whatever the wire carries through the same tolerant constructors.
- `ProviderId` gains `Local` and `Broker`. Both clients stop claiming `Anthropic`. `MockProvider` defaults to `ProviderId::Anthropic` (mock-based tests exercise Anthropic-shaped limits) and is configurable with `with_provider_id` for cross-provider tests.
- `router.rs`'s model-prefix routing and the `fetch_models` message read the id, so they now see the truth.

### 4.3 Truncated and malformed tool input

When a streamed or returned tool call's input doesn't parse:
- **Under `MaxTokens`** (the output cap cut it off): the tool block is dropped from `content`, and `stop.wire.details.truncated_tool = { name, id }` is set. No `ProviderError::Json`, no `{}` dispatch.
- **Otherwise:** `StopReason::MalformedToolCall`, with `details.malformed_tool = { name, id, error }`. The block is dropped.

### 4.4 Structured provider errors

```rust
pub struct ApiErrorBody {
    pub provider: ProviderId,
    pub origin: ErrorOrigin,          // Http { status } | Stream
    pub kind: Option<String>,         // Anthropic error.type | OpenAI error.code or .type | Gemini error.status
    pub message: String,
    pub request_id: Option<String>,   // body request_id, or the request-id header
    pub details: Option<serde_json::Value>,  // Gemini error.details (RetryInfo, ErrorInfo), OpenAI param, …
    pub raw: serde_json::Value,       // the body as received, capped at 64 KB (char-safe)
}
```

- `ProviderError::Api { status, message }` becomes `ProviderError::Reply(Box<ApiErrorBody>)`. A body that isn't JSON is kept as a raw string in `raw`, with `kind: None`.
- Every error-body site in the crate builds one: `api_error_from_response` plus the per-client sites listed by the exploration. 429 bodies are no longer discarded.
- `Display` keeps today's readable text, so log lines and `RunComplete.error` strings stay familiar.

Every body classifies into a normalized class:

```rust
pub enum ErrorClass {
    RateLimited, Overloaded, Server, Timeout, InvalidRequest, ContextOverflow,
    Auth, Permission, Quota, NotFound, TooLarge, Policy, Unrecognized,
}
```

- Each provider classifies by its error kind/code first, then by HTTP status:
  - Anthropic `error.type`: `rate_limit_error`, `overloaded_error`, `api_error`, `timeout_error`, `invalid_request_error`, `authentication_error`, `permission_error`, `billing_error` → `Quota`, `not_found_error`, `request_too_large`.
  - OpenAI codes: `rate_limit_exceeded`, `server_error`, `context_length_exceeded` → `ContextOverflow`, `insufficient_quota` → `Quota`, policy codes → `Policy`.
  - Copilot `model_max_prompt_tokens_exceeded` → `ContextOverflow`.
  - Gemini status: `RESOURCE_EXHAUSTED`, `UNAVAILABLE`, `INVALID_ARGUMENT`, `PERMISSION_DENIED`, `UNAUTHENTICATED`, `NOT_FOUND`, `DEADLINE_EXCEEDED`.
- **Retry classification** reads `ErrorClass`: `RateLimited`, `Overloaded`, `Server` and `Timeout` retry. A stream-origin error retries when its class is one of those, or when it carried no kind or code at all (an unknown kind or code may name a permanent condition, so it does not retry; a flat error event's own `"type": "error"` is not a kind). Any kind containing `policy` (case-insensitive) is `Policy`, which is permanent. This applies to `tuned::is_retryable`, the runner's `is_retryable_provider_error`, and `ProviderRouter` failover.
- Errors that aren't from a body are unchanged: `Http`, `SseParse`, `UnexpectedEndOfStream`, `IncompleteStream`, `MissingAuth`, `LongContextUnavailable`, `Terminating`. Each maps to a class through a fixed table. The never-constructed variants (`Unauthorized`, `QuotaExceeded`, `ModelUnavailable`, `BadRequest`) are removed in favor of the class.
- **`parse_context_overflow`** (`runner.rs`) takes the `ProviderError`, not its `Display` string:
  - `ErrorClass::ContextOverflow` is an overflow even when no numbers parse.
  - Numbers come from `body.message` using the model-limits §7 format table.
  - The three generic phrases remain the fallback only for errors without a body.
  - `parse_output_cap_overflow` reads `body.message` the same way.

### 4.5 Content blocks

- `ContentBlock::Unknown` becomes `Unknown { provider: Option<ProviderId>, raw: serde_json::Value }`.
  - Provider parsers emit it for every block type they don't know, instead of dropping it: Anthropic `server_tool_use`, `web_search_tool_result`, `connector_text`, `citations`, etc.
  - It is stored in the assistant message, written to the transcript (§7.1), rendered, and never sent back to a provider. Request builders drop it as today.
  - Deserializing the legacy literal `{"type":"Unknown"}` still works.
- `ContentBlock::Fallback { from_model, to_model }` is typed. Anthropic requires it echoed exactly where it appeared, and other providers drop it on send.
- Echo rule after a mid-output fallback (Anthropic docs): `thinking` / `redacted_thinking` / `tool_use` blocks before the final `fallback` block are omitted when echoing. The Anthropic request builder applies it.

### 4.6 Server-side fallback

The Anthropic client sends `fallbacks: "default"` plus `anthropic-beta: server-side-fallback-2026-07-01` when all of these hold:
- auth is API-key
- `[recovery].server_side_fallback` is true (the default)
- the model is in the documented set: Claude Fable 5 / 5.1, Opus 5 / 5.5, Sonnet 5.5

Two safeguards:
- **OAuth never sends it.** OAuth requests use claude-cli's pinned beta list, and an unexpected beta risks the stricter 429 pool.
- **A 400 that names `fallbacks` disables it** for the rest of that client's life, writes a `Notice { kind: "server_side_fallback_disabled" }`, and the turn is retried once without it. This mirrors the `LongContextUnavailable` pattern.

## 5. Classification and the recovery ladder — `rupu-agent`

Two new modules sit around the turn loop in `runner.rs`: `outcome` (classification) and `recovery` (the ladder, budgets, sticky state).

### 5.1 Classification

`outcome::classify(&LlmResponse | &ProviderError, &TurnContext) -> Option<Outcome>`:

```rust
pub struct Outcome {
    pub id: String,                // ulid; links Recovery events
    pub class: OutcomeClass,
    pub severity: Severity,        // Info | Warning | Error
    pub title: String,             // "refused · cyber", "truncated · output limit (max_tokens 64,000)"
    pub detail: Option<String>,    // explanation, finishMessage, error message
    pub wire: serde_json::Value,   // the WireStop or ApiErrorBody, verbatim
}
```

| Kind of stop | Classes | Severity / handling |
|---|---|---|
| Normal | `EndTurn`, `ToolUse`, `StopSequence` | No outcome; a stop chip only |
| Info | `PauseTurn` | Info |
| Warning | `Unrecognized`, `Unreported` | Warning. Handled by content: no tool calls → turn finished; tool calls → run them |
| Error | `MaxTokens`, `ContextWindowExceeded`, `Refusal`, `Safety`, `MalformedToolCall`, `Incomplete`, `EmptyReply`, every `ProviderError` (with its `ErrorClass`) | Error |

- **`EmptyReply` is derived:** a normal stop with no text and no tool calls. It typically comes right after tool results.

### 5.2 The ladder

Rung 0 fixes the outcome in place. Rungs 1–4 are the ladder from §3.

| Outcome | Rung 0 (in place) | Then |
|---|---|---|
| `PauseTurn` | Re-send with the paused assistant content as the last message and no new user message. The continuation's content is merged into the same assistant message. Budget 5. | `Incomplete` once the budget is spent |
| `MaxTokens`, text only | Keep the truncated text as the assistant message. Append the user note `TRUNCATION_NOTE`: "Your previous reply was cut off at the output limit. Continue exactly where you stopped; don't repeat what you already wrote." Budget 3 per turn. | 1 → 2 → 3 → 4 |
| `MaxTokens` with `truncated_tool` | Retry the turn once with `max_tokens` raised to the model's known output maximum (`limits.output`), when the request's cap was lower. That happens with the 8192 fallback for an unknown output limit, or a cap lowered by model-limits §7. The truncated assistant content is not kept. If no higher cap is known, rung 0 does nothing. | 1 → 2 → 3 → 4 |
| `ContextWindowExceeded` | Compact through the existing machinery (`compact_context`), then continue as text `MaxTokens` | 1 → 2 → 3 → 4 |
| `Refusal`, `Safety`, `ErrorClass::Policy` | None in the runner. Server-side fallback already ran inside the call; a response it served is not a refusal. The partial output of a refused turn is **discarded**: it's never added to the conversation. | 1 → 2 → 3 → 4 |
| `MalformedToolCall` | Append a corrective user message naming the tool and the parse error. Budget 2. | 1 → 2 → 3 → 4 |
| `EmptyReply` | One user nudge, `EMPTY_REPLY_NOTE`: "Your last reply was empty. Continue the task." | 1 → 2 → 3 → 4 |
| `Incomplete` | Retry the turn once | 1 → 2 → 3 → 4 |
| `RateLimited`, `Overloaded`, `Server`, `Timeout`, stream errors | The existing backoff retries (`MAX_HTTP_RETRIES` = 10) | 2 → 3 → 4 |
| `Quota` | none | 2 → 3 → 4 |
| `ContextOverflow` | The existing model-limits pipeline: `LongContextUnavailable` → output-cap lowering → clamp + compact → trim | 2 → 3 → 4 |
| `NotFound` | none | 1 → 2 → 3 → 4 |
| `Auth`, `Permission`, `InvalidRequest`, `TooLarge`, `Unrecognized`, and errors that aren't from a body and aren't retryable | none | 3 → 4 |

- **Sticky fallback.** Once a rung-1 or rung-2 hop serves a turn, the attempt stays on that provider/model for the rest of its turns. `RecoveryState.active` holds the hop.
- **Budgets.** Each rung-0 budget is per turn. A ladder climb starts from the next untried hop. `MAX_RECOVERY_ACTIONS = 20` per run caps everything; reaching it moves straight to rung 3.
- **Rungs 1 and 2 retry the failed turn,** not the whole run, on the hop's provider/model (§6.2).
- **Interaction with model limits.** A hop resolves its own `ModelLimits` through `rupu_runtime::model_limits::resolve`, and writes a `model_limits` notice for the hop. If the conversation's last measured input is above the hop's input limit, it compacts before sending.

### 5.3 Success rule and final output

- **Success rule.** A run ends `RunStatus::Ok` only when the final turn's stop is normal or warning and the reply has content.
  - A run that recovered ends `Ok`, and its outcomes stay in the transcript and on every surface.
  - The test `final_answer_without_tool_calls_terminates_regardless_of_stop_reason` is inverted: a scripted `MaxTokens` with an empty chain and the `Never` decider ends `Error`, with outcome `max_tokens`.
- **Final output.**
  - The final answer is the joined text of the final turn's continuation chain: the truncated piece plus each continuation, in order.
  - One helper, `rupu_transcript::final_output(events)`, replaces `rupu_orchestrator::runner::read_final_assistant_text` and `rupu-cli`'s `dispatch.rs` copy. It understands chains through the `Recovery { action: continued }` events between `AssistantMessage`s.
  - A failed run's final output is empty. Callers show the outcome instead of stale interim text.
- **Compaction summariser.**
  - `compact_messages` checks its response's stop.
  - A summary that is not normal (refused, empty, truncated) is rejected, and compaction reports failure. The existing "compaction failed" fallbacks then apply.
  - It never replaces history with a refused or truncated summary.

### 5.4 Replay lockstep

What the runner sends must be what `rupu_agent::replay::reconstruct_messages` rebuilds from the transcript. That rule is what makes recover-on-interrupt's continuation correct.

- Truncation notes, malformed-tool corrections and empty-reply nudges are written as `UserMessage` events after the turn's `TurnEnd`. Replay already folds those in, and `push_user_turn` merges with a trailing user turn, as it does today.
- **`PauseTurn` merge.** The paused turn's `TurnEnd` is followed by a `Recovery { action: continued, … }` event whose `merge_into_previous: true` makes replay append the next turn's assistant content to the previous assistant message, instead of opening a new one. The runtime does the same merge.
- **Discarded refusal partials.** The refused turn writes its `AssistantDelta`s, as streamed, and then an `Outcome`. Its `TurnEnd` carries `stop.reason = refusal|safety` and `discarded: true`. Replay drops a discarded turn's content.
- **Fallback hops** don't change the messages. Only which provider handles the next turn changes, so replay needs nothing.
- Each rule has a replay round-trip test (§10).

## 6. Fallback chain configuration

### 6.1 Where it's configured

There is one ordered list. Entries on the attempt's own provider, or entries without a `provider`, are rung 1. The rest are rung 2. Order is kept within each rung.

```yaml
# agent frontmatter — wins over global config
fallbacks:
  - model: claude-opus-4-8
  - provider: openai-codex
    model: gpt-5.6-cyber
```

```toml
# config.toml — default chain for agents without `fallbacks:`
[recovery]
fallbacks = [{ model = "claude-opus-4-8" }]
server_side_fallback = true
park_timeout_secs = 3600
```

- `rupu-config` gets `RecoveryConfig` (`fallbacks`, `server_side_fallback`, `park_timeout_secs`), with defaults `[]`, `true` and `3600`.
- `rupu-agent`'s `AgentSpec` gets `fallbacks: Option<Vec<FallbackEntry { provider: Option<String>, model: String }>>`.
- Precedence: agent frontmatter, then global `[recovery]`, then empty.
- Cross-provider entries are never added implicitly.
- `[recovery]` keys are ordinary config keys under the existing dotted-key contract (shown and editable in the CP config editor like any other table). They carry no policy lock.

### 6.2 Executing a hop

- `rupu-runtime` gets `provider_factory::build_for_hop(config, credentials, provider, model)`. It returns the boxed provider plus that model's resolved `ModelLimits`. The runner holds an `Option<HopBuilder>` port in `AgentRunOpts`, so `rupu-agent` stays trait-only (architecture rule 1). `None` means rungs 1–2 are unavailable, and the ladder skips them with reason "no hop builder".
- **A failed hop build is skipped, not silently dropped.** No credentials, an unknown provider or a factory error each write a `Recovery { action: skipped, reason }`, and the ladder moves to the next hop.
- **Reasoning blocks.**
  - Across providers, the existing provider-tag echo gate drops the other provider's reasoning.
  - On the same provider, Anthropic drops blocks the new model can't read on its own end. rupu doesn't strip them.
- **Usage.** Every request's `Usage` event carries the provider and `served_model` that actually ran it. That includes a server-side fallback's serving model, taken from the response's top-level `model`. The usage ledger then attributes cost per serving model.
- **Remote placed units** read the agent's frontmatter chain and the host's own `[recovery]` table.

## 7. Transcript and orchestrator

### 7.1 Transcript events — `rupu-transcript`

- **`TurnEnd`** gains `stop: Option<StopRecord>` (`reason`, `wire`, `refusal`, `served_by`) and `discarded: bool` (serde default `false`). `stop_reason: Option<String>` keeps being written, now with the wire value.
- **New `Event::Outcome { id, turn_idx, class, severity, title, detail, wire }`** is written when classification yields an outcome.
- **New `Event::Recovery { outcome_id, rung, action, attempt, budget, provider, model, reason, merge_into_previous }`** is written once per action.
  - `action` is one of `continued`, `retried`, `compacted`, `fell_back`, `served_by_fallback`, `skipped`, `asked`, `parked`, `failed`.
  - `served_by_fallback` records a server-side fallback seen on a response.
  - It replaces the `provider_retry` notice. Old transcripts still render their notices.
- **`RunComplete`** gains `outcome: Option<OutcomeRecord>`: the terminal cause on any non-`Ok` end.
  - A parked attempt writes `status: aborted` with `outcome.parked: true`. `RunStatus` gains no variant, so older readers still parse it.
  - The merged `prepare_continuation` already classifies `aborted` as `Resume`, which is exactly what a parked attempt needs.
- **`Event::Unknown`** becomes `Unknown { tag: String, data: serde_json::Value }`.
  - The custom `Deserialize` keeps the payload.
  - A custom `Serialize` writes `{"type": tag, "data": data}` verbatim, so `transcript show --format json`, the CP pass-through and SSE stream it intact.
- **Registration.** `outcome` and `recovery` are added to `KNOWN_EVENT_TAGS`. Every exhaustive match over `Event` handles them; the compiler enforces this.
- **Rust presentation layer.** A pure module, `rupu_transcript::present`, turns `Outcome` / `Recovery` / `StopRecord` / `Unknown` into:

  ```rust
  pub struct Presentation {
      pub tone: Tone,              // Danger | Warn | Info | Good | Dim
      pub glyph: char,             // ✗ ! · ↺ → ⏸ ✓
      pub title: String,
      pub chips: Vec<String>,
      pub detail_lines: Vec<String>,
      pub raw: Option<String>,     // pretty JSON, collapsed by default
  }
  ```

  Every CLI renderer uses it (§8.1). The web mirrors it in `lib/outcome.ts`, and both are checked against the same fixture set (§10).

### 7.2 Orchestrator — `rupu-orchestrator`

- **Typed cause, carried upward.** `OutcomeRecord` (re-exported from `rupu-transcript`) is added as `cause: Option<OutcomeRecord>` on:
  - executor `Event`s: `StepFailed`, `UnitCompleted`, `DispatchCompleted`
  - persisted records: `StepResultRecord`, `ItemResult`, and the fan-out/parallel outcomes, all with `error: Option<String>` as well
  - `RunRecord`, alongside `error_message`

  The `#[allow(dead_code)]` error fields in `FanoutItemOutcome` / `ParallelSubOutcome` become persisted fields.
- **`dispatch_one`**'s `terminal_error()` fold carries the cause. With §5.3, a refused or truncated answer is no longer `Ok`.
- **The executor `Event`** gets `#[serde(other)] Unknown`, so an older reader skips new variants instead of failing the line.

### 7.3 Sessions

- A turn that reaches rung 3 ends `Failed`, and its outcome is rendered. The operator's next `session send` is the answer to rung 3, so sessions need no parking.
- **Fix:** today an `Err` from `run_agent_with_limits` leaves `message_history` unchanged, which drops the user prompt and any tool work done in that turn.
  - `RunResult` and the error path both return the conversation as it stood when the outcome happened, minus a discarded partial.
  - The session writes it in both cases, as it already does for `final_limits`.
  - `last_error` carries the outcome title instead of "turn ended with status error".

## 8. Rung 3 — asking the operator

### 8.1 `RecoveryDecider` port — `rupu-agent`

The port sits next to the permission `Decider`:

```rust
pub trait RecoveryDecider: Send + Sync {
    async fn decide(&self, req: RecoveryRequest) -> RecoveryDecision;
}
pub struct RecoveryRequest { outcome: Outcome, tried: Vec<RecoveryRecord>, chain: Vec<FallbackEntry> }
pub enum RecoveryDecision {
    RetryWith { provider: Option<String>, model: String },
    Guidance { text: String },
    Fail,
    Park,                       // unattended: stop this attempt so the orchestrator parks it
}
```

There are three implementations:
- **`Interactive`**, used by foreground `rupu run` when stdin and stdout are a TTY. A foreground `workflow run` uses `Park`, because the live-view dashboard owns the terminal. The operator decides from another shell (`rupu workflow recover`) or from the CP, and the foreground runner picks up the decision through the existing handoff.
  - It renders the outcome through `present`, then offers:
    - pick a chain entry, or type `provider/model`
    - type guidance
    - fail
  - `RetryWith` continues in-process through the hop path (§6.2).
  - `Guidance` appends the text as a user message (`push_user_turn`) and continues.
  - Serial tests run with `< /dev/null`, so they get `Never`.
- **`Park`**, used for every orchestrator-driven run: workflows (foreground or under `cp serve`), cron, autoflows, fan-out and parallel units. It always returns `Park`.
- **`Never`**, used by a non-TTY standalone `rupu run`. It returns `Fail`. The terminal error carries a hint: `continue with: rupu run <agent> --continue <run_id> --model <provider/model>`.
  - This needs `rupu run --continue` to accept `--model`/`--provider` overrides. The merged command builds from the agent's current definition, so this is a small addition.

### 8.2 Parking — `rupu-orchestrator`, `rupu-cli`, `rupu-cp`

A parked recovery reuses the gate machinery as **a gate of kind `recovery`**. That gives it the same run-lock decision storage, single-runner handoff (`claim_runner`), notify hooks, timeout sweep, CP decision surface and CLI commands.

1. **The attempt.**
   - It writes `Recovery { action: parked }`, then `RunComplete { status: aborted, outcome: { parked: true, … } }`.
   - It returns `RunResult { parked: true, … }`. This is a new flag next to `paused`.
2. **The orchestrator records the pending recovery** in `RunRecord.pending_recoveries` under the run lock:
   - fields: `{ step_id, unit (index | sub_id | none), agent_run_id, transcript, outcome, deadline }`
   - It emits `StepAwaitingRecovery { step_id, unit, outcome }` and marks the unit `awaiting_recovery`.
   - Siblings keep running. The step settles only when every unit has settled.
   - The run's status is `AwaitingApproval`, as for any gate, and the gate's kind says `recovery`.
3. **The decision** is recorded through the existing gate decision path (`RunStore::approve_gate`-style). `GateDecision` gains an optional `unit` key, because gate decisions are step-scoped today and a recovery belongs to one unit. It also gets a richer payload: `retry_with { provider?, model } | guidance { text } | fail`. Decisions come from:
   - the CLI: `rupu workflow recover <run> <step>[#<unit>] --model <provider/model> | --guidance <text> | --fail`
   - the CP: `POST /api/runs/:id/recoveries/:key`
4. **Applying it.** The runner that claims the decision, via the existing handoff, applies it:
   - **`retry_with` / `guidance`:**
     - It calls `prepare_continuation(transcript)`. A parked transcript ends `aborted`, so this yields `Resume`.
     - It then calls a new `apply_continuation_with(opts, messages, seed_source, note)`:
       - for `retry_with`, `note` is `RECOVERY_RETRY_NOTE` ("A previous attempt at this step stopped: <outcome title>. You are continuing on <provider/model>.")
       - for `guidance`, `note` is the operator's text
     - For `retry_with`, it also overrides the opts' provider/model with that hop. The `Resume` continuation is dispatched through the normal unit path, keeping its semaphore, placement, events and codename.
   - **`fail`:** fails the unit with its cause.
5. **Timeout.** `park_timeout_secs` (`[recovery]`, default 3600) sets the gate deadline. The existing `cp serve` gate sweep fails expired recoveries through the gate-reject path.
6. **Notify.** An optional workflow-level `recovery: { notify: [...] }` fires best-effort when a unit parks. It uses the gate `notify:` machinery: never blocking, and never on an auto-decision.
7. **Remote placed units** park on the coordinator. The decision continues the unit on its host through recover-on-interrupt's `UnitDispatch.continue_from` and the `agent.continue` capability (that spec's PR 3). Until PR 3 lands, or for a peer that doesn't advertise the capability, the unit restarts on the decided model and is flagged `restarted` with the reason.

## 9. Rendering

### 9.1 CLI — `rupu-cli`

Every surface renders `Outcome`, `Recovery`, the `TurnEnd` stop chip (Full mode), the raw `Unknown`, and a failed `RunComplete` with its outcome, all through `rupu_transcript::present`. Wildcard arms over `Event` are replaced with exhaustive matches.

| Surface | Changes |
|---|---|
| `rupu run` live printer (`cmd/run.rs`) | • Outcome and recovery lines.<br>• No "done" footer without a `RunComplete`.<br>• The exit code and `RunStatus` honor `terminal_error()`. |
| Live-view dashboard (`output/live_view/mux.rs::project_event`) | • Outcome rows in `Danger` / `SoftFailed` / `Dim`.<br>• Recovery chips (`Retrying`).<br>• A notice keeps its kind.<br>• A failed `RunComplete` gets a row.<br>• `RunView.error` and `cause` render in the structure pane and in `run_summary`.<br>• Parked units render `⏸ awaiting recovery`. |
| Workflow line printer and child renderers (`output/workflow_printer.rs`), `watch --replay`, autoflow serve live lines | Exhaustive matches plus outcome lines |
| Retained workflow view, session TUI, `session show`, `transcript show` (pretty), `session attach` | Outcome and recovery rows, the stop chip, and `Unknown` with its tag plus raw JSON (collapsed) |
| `transcript show --format json/jsonl` | `Unknown` round-trips verbatim |
| `workflow show-run`, completion summary | A failed or parked unit lists its cause; parked units list the `workflow recover` command |

Example rendering:

```
  ✗ refused · cyber — "This request was declined because it could enable cyber harm."
    ↺ rung 1 · fell back to anthropic/claude-opus-4-8          ✓ served
  ✗ truncated · output limit (max_tokens 64,000)
    ↺ rung 0 · continued 1/3
  ⏸ awaiting recovery · safety (gemini SAFETY: HARM_CATEGORY_DANGEROUS_CONTENT)
    decide: rupu workflow recover <run> review#41 --model … | --guidance … | --fail
  ! unrecognized stop · anthropic "brand_new_reason"  {raw…}
```

### 9.2 CP backend — `rupu-cp`

- **Transcript endpoints** stay a typed pass-through. They now carry `outcome`, `recovery`, `TurnEnd.stop`/`discarded`, and `Unknown { tag, data }` verbatim.
- **Run DTOs:**
  - `query_run_detail` steps carry `error` and `cause`.
  - `RunListRow` and `AgentRunRow` gain `cause`.
  - Run detail gains `pending_recoveries`.
- **Recoveries** are served with the gate endpoints, plus a new `POST /api/runs/:id/recoveries/:key` that takes `{ action: retry_with|guidance|fail, provider?, model?, text? }`. Like every CP mutation, it records the decision; `cp serve`'s runner applies it.
- **The needs-you aggregate** includes parked recoveries.

### 9.3 CP web — `crates/rupu-cp/web/src`

This matches the mock approved in conversation.

- **`lib/transcript.ts`:**
  - typed `outcome` and `recovery` events
  - `turn_end` with `stop_reason`, `stop` and `discarded`
  - `run_complete.outcome`
  - an `unknown` that keeps its data
- **`lib/outcome.ts`** is the presentation mirror of `present`: tone, glyph, title, chips.
- **`transcriptView.ts` / `Turn.tsx`:**
  - A new `outcome` turn block, rendered as a left-border block in `err`/`warn`/`info` tone. It shows the provider's words and a recovery timeline (rung · action · result). Raw details sit behind the existing `ErrorDetail` Parsed/Raw toggle.
  - Each turn's header gets a stop chip and a "recovered" chip.
  - `unknown` blocks render their raw payload.
  - A turn's `summary.result` is `err` when a non-recovered error outcome or a failed `run_complete` belongs to it.
- **`TranscriptPanel.tsx`** fixes:
  - `statusGlyph` handles `ok` / `error` / `aborted`.
  - The footer error shows when embedded.
  - The `onComplete` check for `run_failed` goes away.
- **Run graph:** `lib/runGraphModel.ts` stores `cause` on the `GraphNode`, and nodes render it in their tooltip.
- **Awaiting recovery** gets a node treatment on the run graph, and the run detail gets an **awaiting-recovery card** showing:
  - the outcome and the rungs already tried (including skips and their reasons)
  - the three operator actions: continue with a model (chain entries plus free entry), continue with guidance, fail unit
  - a countdown to the park deadline

  The same card appears in the needs-you queue and the Situation Room (`lib/situationRoom/cards.ts` gets `step_awaiting_recovery`).
- **Run detail and the Situation Room** show an error card per step/unit cause.

## 10. Testing

All fixtures are invented from scratch. The repo is public.

- **Provider mappings.** Each row of the §4.2 tables gets a fixture on the stream path and the `send` path, covering:
  - an unknown value and a missing value
  - refusal, safety and prompt-blocked payloads
  - mid-stream error events
  - truncated and malformed tool input
  - a server-side fallback response with `fallback` blocks and `usage.iterations`
  - an error body per provider, for every `ErrorClass` row

  Inverted tests: Gemini `SAFETY → EndTurn`, and Codex's invented `Api { 500 }`. A one-off live check verifies the Gemini Code Assist reply envelope; it's recorded in the plan, not in CI.
- **Error plumbing.**
  - `parse_context_overflow` gets a code-first case per provider, with the model-limits §7 table kept as its message fixtures.
  - Retry classification gets a test per `ErrorClass`.
  - A char-safe truncation test uses multibyte bodies.
- **The ladder, end to end (mock providers).** One test per §5.2 row asserts:
  - the rungs taken
  - the `Outcome` / `Recovery` events written
  - the final `RunStatus`
  - the final output

  The tests also cover:
  - budget exhaustion and the global cap
  - sticky fallback
  - a skipped hop with no credentials
  - server-side fallback turned off after a 400
  - a cross-provider hop, with two mock providers built through the factory and `ENV_LOCK` held
- **Replay lockstep.** For each §5.4 rule, `reconstruct_messages(transcript)` must equal the messages the runtime actually sent. It's checked against the mock provider's recorded requests.
- **Compatibility:**
  - A new transcript parses under the old tag set: new events become `Unknown`, and `RunStatus` is unchanged.
  - `Unknown { tag, data }` round-trips byte-for-byte through `transcript show --format json` and the CP transcript API.
  - The executor `Event` skips an unknown variant.
  - The legacy `{"type":"Unknown"}` content block still deserializes.
- **Parking (orchestrator integration):**
  - Park, then each decision (`retry_with`, `guidance`, `fail`), with continuation through `prepare_continuation`.
  - A timeout fails the unit through the sweep.
  - Siblings finish while one unit is parked.
  - A decision recorded while a runner is live is handed off to that runner.
  - A remote unit without `agent.continue` is restarted and flagged.
- **Sessions.** A failed turn keeps its prompt and tool work in `message_history`.
- **Rendering.** A shared fixture set holds one invented transcript per outcome class, plus `Unknown`, a parked run and a recovered run. It is rendered by:
  - every CLI surface (insta snapshots; the four "every variant gets a row" guards are extended)
  - the web (vitest, including a "nothing dropped" test over every class)

  `present` and `lib/outcome.ts` are checked against the same expected titles and tones.
- **Local verification before every PR** (CI is a release gate):
  - `cargo clippy --workspace --all-targets`, minding the CI 1.95 lints (`!x.is_some_and(..)`, match arms that contain only an `if`)
  - `cargo test -p` for each touched crate
  - `cargo test -p rupu-cli --test serial` with `< /dev/null`
  - `npm run test` and `tsc` in `crates/rupu-cp/web`

## 11. Docs

- New `docs/response-outcomes.md`: outcome classes, the ladder table, `fallbacks:` and `[recovery]`, server-side fallback, parking and `rupu workflow recover`, and how each surface shows outcomes.
- The model-limits spec gets a note on §9 marking its "unrecognized becomes a surfaced error" sentence as superseded by this spec's §2.
- CLAUDE.md crate entries for `rupu-providers` (typed `Stop`, `ApiErrorBody` / `ErrorClass`), `rupu-agent` (`outcome` / `recovery`, `RecoveryDecider`), `rupu-transcript` (`Outcome` / `Recovery` events, `present`, raw `Unknown`) and `rupu-orchestrator` (`cause`, recovery gates).
- The agent-file reference documents `fallbacks:`, and the config reference documents `[recovery]`.

## 12. Plans

One spec, four plans. Each is a PR, merged in order.

1. **Provider outcomes** (`rupu-providers`, minimal runner adaptation):
   - typed `Stop` and every §4.2 mapping
   - `ApiErrorBody` / `ErrorClass` and retry classification
   - mid-stream errors and `IncompleteStream`
   - truncated / malformed tool input
   - `Unknown { raw }` / `Fallback` content blocks
   - `provider_id` fixes
   - a tolerant `send` decode
   - `MockProvider` scripting

   The runner only adapts to the new types: it writes `TurnEnd.stop_reason` from `stop.wire`. Behavior changes come in plan 2.
2. **Classification and recovery** (`rupu-agent`, `rupu-transcript`, `rupu-runtime`, `rupu-config`, `rupu-orchestrator`, `rupu-cli` session):
   - `outcome` and `recovery` modules; rungs 0–2 and rung 3 as `Never` only
   - `Outcome` / `Recovery` events, `TurnEnd.stop`, `RunComplete.outcome`, raw `Unknown`
   - replay lockstep
   - the success rule, `final_output`, and the summariser stop check
   - server-side fallback and the `fallbacks:` / `[recovery]` config, with `build_for_hop`
   - orchestrator `cause` propagation and the executor `#[serde(other)]`
   - the session history fix
   - the `rupu run --continue --model` override and its hint
3. **Rendering** (`rupu-transcript::present`, `rupu-cli` output, `rupu-cp` DTOs, web): every §9 surface, plus the rendering bugs listed in §1.
4. **Ask and park — rung 3:**
   - `RecoveryDecider` with `Interactive` and `Park`
   - recovery gates with the richer decision payload, and `apply_continuation_with`
   - `rupu workflow recover`, the CP recovery endpoint, the awaiting-recovery card, the needs-you entry, notify and timeout
   - remote continuation through recover-on-interrupt PR 3 when it has landed; restart-and-flag until then
