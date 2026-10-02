# Response outcomes — Plan 1: provider outcomes — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every provider reply parses into a typed `Stop`, and every provider error into a structured `ApiErrorBody` with a normalized `ErrorClass`. Nothing a provider says about how a reply ended is dropped any more.

**Architecture:**
- Two new pure modules in `rupu-providers`:
  - `stop` holds the typed stop and the per-provider mapping tables.
  - `reply_error` holds the structured error body, its parser and classifier.
- One more module, `overflow`, receives the context-overflow string table moved out of `rupu-agent`.
- Each client (Anthropic, Codex/Responses, chat completions, Gemini, broker, local) switches to these.
- `LlmResponse.stop_reason: Option<StopReason>` becomes `stop: Stop`.
- `ProviderError::Api` and `ProviderError::RateLimited` become `ProviderError::Reply(Box<ApiErrorBody>)`.
- The runner only adapts to the new types. All behavior changes in the runner (classification, ladder, success rule) belong to Plan 2.

**Tech stack:** Rust 2021, serde / serde_json, thiserror, tokio, reqwest. Tests use the crate's existing inline `#[cfg(test)]` modules plus `crates/<c>/tests/it/`.

**Spec:** `docs/superpowers/specs/2026-10-01-rupu-response-outcomes-design.md` — this plan implements §4 (provider layer) and the provider-side half of §12 Plan 1.

## Global constraints

- **Formatting:** never run package-wide `cargo fmt`. Format only the files you touched, check first:
  ```bash
  rustfmt --edition 2021 --check <file>
  rustfmt --edition 2021 <file>
  ```
  Never run it on `lib.rs` or any `mod.rs`.
- **Git:** never run `git stash` in any form.
- **Commits:** every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Commit on the branch only; this plan opens no PR.
- **Clippy:** must pass with `-D warnings`. CI's clippy is 1.95, so write `x.is_none_or(..)` / `!matches!(..)` instead of `!x.is_some_and(..)`, and never write a `match` arm whose only body is an `if`.
- **Workspace rules:**
  - Dependency versions live only in the root `Cargo.toml`.
  - `rupu-agent` stays trait-only.
  - `rupu-cli` stays thin.
- **Fixtures:** invent all of them from scratch. This is a public repo, and real assessment data is never used.
- **Integration tests:**
  - New integration tests go in `crates/<c>/tests/it/` and are listed in its `main.rs`. Never add a new top-level `tests/*.rs`.
  - Run one with `cargo test -p <c> --test it <module>::`.
- **Prose rules (doc comments, notices, error strings):** no "simply", "just", "easy"; no exclamation marks; sentence case.
- **Every `Stop`** carries the provider's own wire value in `wire.value` (or `None` when the provider sent none). Code never invents a wire value, except for a synthetic stop built with `Stop::synthetic`.
- **Request builders** keep dropping `ContentBlock::Unknown` exactly as they drop it today. They also drop `ContentBlock::Fallback` for every provider except Anthropic.

## File structure

| File | Responsibility |
|---|---|
| `crates/rupu-providers/src/stop.rs` (new) | `Stop`, `StopReason`, `WireStop`, `RefusalDetail`, `RefusalSource`, `ServedBy`, `FallbackHop`; the `map` tables (Anthropic, chat finish, Responses incomplete, Gemini finish) |
| `crates/rupu-providers/src/reply_error.rs` (new) | `ApiErrorBody`, `ErrorOrigin`, `ErrorClass`, `parse_error_body`, `classify` |
| `crates/rupu-providers/src/overflow.rs` (new) | `Overflow`, `OutputCapOverflow`, `parse_context_overflow`, `parse_output_cap_overflow`, `numbers`, moved verbatim from `rupu-agent/src/runner.rs` |
| `crates/rupu-providers/src/types.rs` | `LlmResponse.stop`; `ContentBlock::Unknown { provider, raw }` and `ContentBlock::Fallback`; custom `Deserialize` for `ContentBlock`; re-export of `stop::*` |
| `crates/rupu-providers/src/error.rs` | `ProviderError::Reply`, `IncompleteStream`; `Api`, `RateLimited`, `Unauthorized`, `QuotaExceeded`, `ModelUnavailable`, `BadRequest` removed; `api_error_from_response` gets a new signature; helper constructors |
| `crates/rupu-providers/src/{anthropic,openai_codex,openai_wire,github_copilot,openai_compatible,google_gemini,google_gemini/code_assist,local,broker_client,provider_id,tuned,router,smart_router,provider,task_classifier}.rs` | Switch to the new types and mappings |
| `crates/rupu-agent/src/runner.rs` | Adapts only: `TurnEnd.stop_reason` comes from `stop.wire`; the retry classifier reads `ErrorClass`; overflow parsing is imported from `rupu_providers::overflow`; `ScriptedTurn::Reply` / `ScriptedTurn::ReplyError` added; `MockProvider::with_provider_id` |
| `crates/rupu-cli/src/cp_inventory.rs`, `crates/rupu-runtime/src/provider_factory.rs`, and the test files listed in Task 1 | Compile against the new types |

---

### Task 1: The typed `Stop` and the `LlmResponse.stop` field

**Files:**
- Create: `crates/rupu-providers/src/stop.rs`
- Modify: `crates/rupu-providers/src/lib.rs` (add `pub mod stop;` after `pub mod sse;`)
- Modify: `crates/rupu-providers/src/types.rs`:
  - remove `enum StopReason`
  - add `pub use crate::stop::{FallbackHop, RefusalDetail, RefusalSource, ServedBy, Stop, StopReason, WireStop};`
  - change `LlmResponse`
- Modify every `LlmResponse { … stop_reason: … }` constructor and every `.stop_reason` read. Each site listed here gets a mechanical change that keeps today's behavior. Later tasks swap in the real mappings.
  - `crates/rupu-providers/src/{anthropic,openai_codex,openai_wire,openai_compatible,google_gemini,local,broker_client,provider,task_classifier,router,smart_router,tuned,github_copilot}.rs`
  - `crates/rupu-providers/tests/it/integration.rs`
  - `crates/rupu-agent/src/runner.rs`
  - `crates/rupu-agent/tests/it/runner_model_limits.rs`
  - `crates/rupu-orchestrator/tests/it/cancel_vs_completion.rs`
  - `crates/rupu-runtime/src/provider_factory.rs`
- Test: inline `#[cfg(test)] mod tests` in `stop.rs`

**Interfaces:**
- Produces, used by every later task and by Plan 2:
  ```rust
  pub enum StopReason { EndTurn, ToolUse, StopSequence, MaxTokens, ContextWindowExceeded, PauseTurn,
      Refusal, Safety, MalformedToolCall, Incomplete, Unreported, Unrecognized }
  impl StopReason { pub fn as_str(&self) -> &'static str }
  pub struct WireStop { pub provider: String, pub value: Option<String>, pub details: Option<serde_json::Value> }
  pub enum RefusalSource { Classifier, Model, Policy, Unknown }
  pub struct RefusalDetail { pub category: Option<String>, pub explanation: Option<String>,
      pub recommended_model: Option<String>, pub source: RefusalSource }
  pub struct FallbackHop { pub from_model: String, pub to_model: String }
  pub struct ServedBy { pub model: String, pub hops: Vec<FallbackHop> }
  pub struct Stop { pub reason: StopReason, pub wire: WireStop,
      pub refusal: Option<RefusalDetail>, pub served_by: Option<ServedBy> }
  impl Stop {
      pub fn from_wire(reason: StopReason, provider: &str, value: Option<&str>) -> Stop;
      pub fn synthetic(reason: StopReason, provider: &str) -> Stop;   // wire.value = reason.as_str()
      pub fn set_detail(&mut self, key: &str, value: serde_json::Value);
      pub fn wire_value_or_reason(&self) -> String;                   // what TurnEnd.stop_reason gets
  }
  pub mod map {
      pub fn anthropic(value: &str) -> StopReason;
      pub fn chat_finish(value: &str) -> StopReason;
      pub fn responses_incomplete(reason: Option<&str>) -> StopReason;
      pub fn gemini_finish(value: &str) -> StopReason;
      pub fn gemini_block_reason_is_known(value: &str) -> bool;
  }
  // types.rs
  pub struct LlmResponse { pub id: String, pub model: String, pub content: Vec<ContentBlock>,
      pub stop: Stop, pub usage: Usage }
  ```

- [ ] **Step 1: Write the failing tests** at the bottom of the new `crates/rupu-providers/src/stop.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reason_round_trips_snake_case_and_unknown_is_unrecognized() {
        for r in [
            StopReason::EndTurn, StopReason::ToolUse, StopReason::StopSequence,
            StopReason::MaxTokens, StopReason::ContextWindowExceeded, StopReason::PauseTurn,
            StopReason::Refusal, StopReason::Safety, StopReason::MalformedToolCall,
            StopReason::Incomplete, StopReason::Unreported, StopReason::Unrecognized,
        ] {
            let v = serde_json::to_value(&r).unwrap();
            assert_eq!(v, serde_json::json!(r.as_str()));
            assert_eq!(serde_json::from_value::<StopReason>(v).unwrap(), r);
        }
        let r: StopReason = serde_json::from_value(serde_json::json!("brand_new_reason")).unwrap();
        assert_eq!(r, StopReason::Unrecognized);
    }

    #[test]
    fn anthropic_table_covers_every_documented_value() {
        use map::anthropic as a;
        assert_eq!(a("end_turn"), StopReason::EndTurn);
        assert_eq!(a("tool_use"), StopReason::ToolUse);
        assert_eq!(a("stop_sequence"), StopReason::StopSequence);
        assert_eq!(a("max_tokens"), StopReason::MaxTokens);
        assert_eq!(a("model_context_window_exceeded"), StopReason::ContextWindowExceeded);
        assert_eq!(a("pause_turn"), StopReason::PauseTurn);
        assert_eq!(a("refusal"), StopReason::Refusal);
        assert_eq!(a("brand_new_reason"), StopReason::Unrecognized);
    }

    #[test]
    fn chat_finish_table() {
        use map::chat_finish as c;
        assert_eq!(c("stop"), StopReason::EndTurn);
        assert_eq!(c("length"), StopReason::MaxTokens);
        assert_eq!(c("tool_calls"), StopReason::ToolUse);
        assert_eq!(c("function_call"), StopReason::ToolUse);
        assert_eq!(c("content_filter"), StopReason::Safety);
        assert_eq!(c("abort"), StopReason::Incomplete);
        assert_eq!(c("eos_but_new"), StopReason::Unrecognized);
    }

    #[test]
    fn responses_incomplete_table() {
        use map::responses_incomplete as r;
        assert_eq!(r(Some("max_output_tokens")), StopReason::MaxTokens);
        assert_eq!(r(Some("content_filter")), StopReason::Safety);
        assert_eq!(r(Some("max_messages")), StopReason::Incomplete);
        assert_eq!(r(Some("steered")), StopReason::Incomplete);
        assert_eq!(r(Some("something_new")), StopReason::Incomplete);
        assert_eq!(r(None), StopReason::Incomplete);
    }

    #[test]
    fn gemini_finish_table() {
        use map::gemini_finish as g;
        assert_eq!(g("STOP"), StopReason::EndTurn);
        assert_eq!(g("MAX_TOKENS"), StopReason::MaxTokens);
        for s in ["SAFETY", "RECITATION", "LANGUAGE", "BLOCKLIST", "PROHIBITED_CONTENT", "SPII",
                  "IMAGE_SAFETY", "IMAGE_PROHIBITED_CONTENT", "IMAGE_RECITATION", "IMAGE_OTHER",
                  "NO_IMAGE"] {
            assert_eq!(g(s), StopReason::Safety, "{s}");
        }
        for s in ["MALFORMED_FUNCTION_CALL", "UNEXPECTED_TOOL_CALL", "TOO_MANY_TOOL_CALLS"] {
            assert_eq!(g(s), StopReason::MalformedToolCall, "{s}");
        }
        assert_eq!(g("OTHER"), StopReason::Incomplete);
        assert_eq!(g("FINISH_REASON_UNSPECIFIED"), StopReason::Unreported);
        // Not Gemini values (the old table invented them): unrecognized now.
        assert_eq!(g("STOP_SEQUENCE"), StopReason::Unrecognized);
        assert_eq!(g("FUNCTION_CALLING"), StopReason::Unrecognized);
    }

    #[test]
    fn synthetic_and_from_wire() {
        let s = Stop::synthetic(StopReason::EndTurn, "mock");
        assert_eq!(s.wire.value.as_deref(), Some("end_turn"));
        assert_eq!(s.wire.provider, "mock");
        let w = Stop::from_wire(StopReason::Unreported, "openai-compatible", None);
        assert_eq!(w.wire.value, None);
        assert_eq!(w.wire_value_or_reason(), "unreported");
        let mut d = Stop::from_wire(StopReason::Safety, "google-gemini-cli", Some("SAFETY"));
        d.set_detail("finishMessage", serde_json::json!("blocked"));
        d.set_detail("prompt_blocked", serde_json::json!(false));
        assert_eq!(d.wire.details.as_ref().unwrap()["finishMessage"], "blocked");
        assert_eq!(d.wire_value_or_reason(), "SAFETY");
    }

    #[test]
    fn stop_serializes_compactly() {
        let s = Stop::from_wire(StopReason::EndTurn, "anthropic", Some("end_turn"));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v, serde_json::json!({
            "reason": "end_turn",
            "wire": {"provider": "anthropic", "value": "end_turn"}
        }));
        let back: Stop = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
```

- [ ] **Step 2: Add `pub mod stop;` to `lib.rs` and run the tests to verify they fail.**

Run: `cargo test -p rupu-providers --lib stop::`
Expected: compile errors (`Stop`, `map` not found).

- [ ] **Step 3: Implement `stop.rs`** above the test module:

```rust
//! How an LLM reply ended, typed (spec 2026-10-01 response-outcomes §4.1).
//!
//! Every provider parses its own stop/finish signal into a [`Stop`]. The
//! provider's exact words always survive in [`Stop::wire`], so a reason rupu
//! doesn't know is still rendered — it becomes [`StopReason::Unrecognized`],
//! never an error and never a silent default.

use serde::{Deserialize, Serialize};

/// Normalized reason a reply ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// A value rupu doesn't know; [`Stop::wire`] carries it.
    #[serde(other)]
    Unrecognized,
}

impl StopReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::ToolUse => "tool_use",
            StopReason::StopSequence => "stop_sequence",
            StopReason::MaxTokens => "max_tokens",
            StopReason::ContextWindowExceeded => "context_window_exceeded",
            StopReason::PauseTurn => "pause_turn",
            StopReason::Refusal => "refusal",
            StopReason::Safety => "safety",
            StopReason::MalformedToolCall => "malformed_tool_call",
            StopReason::Incomplete => "incomplete",
            StopReason::Unreported => "unreported",
            StopReason::Unrecognized => "unrecognized",
        }
    }
}

/// The provider's own words for how the reply ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireStop {
    /// `ProviderId`'s display string (`anthropic`, `openai-codex`, …), or
    /// `mock` / `local` / `broker`.
    pub provider: String,
    /// e.g. `refusal`, `content_filter`, `SAFETY`, `max_output_tokens`.
    /// `None` when the provider sent nothing.
    pub value: Option<String>,
    /// Provider extras: `stop_sequence`, `finishMessage`, `safetyRatings`,
    /// `incomplete_details`, `blockReason`, `truncated_tool`, `malformed_tool`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalSource {
    /// A provider safety classifier declined (Anthropic `stop_details`).
    Classifier,
    /// The model itself declined (an OpenAI `refusal` field/part).
    Model,
    /// A provider policy error code (OpenAI `bio_policy`, …).
    Policy,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefusalDetail {
    /// `cyber`, `bio`, … — or `None`, a valid permanent value.
    pub category: Option<String>,
    /// Display text only; never parsed.
    pub explanation: Option<String>,
    pub recommended_model: Option<String>,
    pub source: RefusalSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackHop {
    pub from_model: String,
    pub to_model: String,
}

/// Anthropic server-side fallback: which model actually served the reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServedBy {
    pub model: String,
    pub hops: Vec<FallbackHop>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub reason: StopReason,
    pub wire: WireStop,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<RefusalDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_by: Option<ServedBy>,
}

impl Stop {
    /// A stop parsed off a provider's wire. `value` is exactly what it sent.
    pub fn from_wire(reason: StopReason, provider: &str, value: Option<&str>) -> Stop {
        Stop {
            reason,
            wire: WireStop {
                provider: provider.to_string(),
                value: value.map(str::to_string),
                details: None,
            },
            refusal: None,
            served_by: None,
        }
    }

    /// A stop no provider sent — mock scripts and providers whose wire has
    /// no stop signal. Its wire value is the reason's own name.
    pub fn synthetic(reason: StopReason, provider: &str) -> Stop {
        let value = reason.as_str();
        Stop::from_wire(reason, provider, Some(value))
    }

    /// Insert `key` into `wire.details` (created as an object when absent).
    pub fn set_detail(&mut self, key: &str, value: serde_json::Value) {
        let details = self
            .wire
            .details
            .get_or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let Some(obj) = details.as_object_mut() {
            obj.insert(key.to_string(), value);
        }
    }

    /// The provider's value, or the reason's name when it sent none —
    /// what `TurnEnd.stop_reason` records.
    pub fn wire_value_or_reason(&self) -> String {
        self.wire
            .value
            .clone()
            .unwrap_or_else(|| self.reason.as_str().to_string())
    }
}

/// Per-provider wire value → [`StopReason`] tables (spec §4.2). Pure, so
/// each row is pinned by a test. Tool-use precedence (a function call in
/// the output) is applied by the providers, not here.
pub mod map {
    use super::StopReason;

    pub fn anthropic(value: &str) -> StopReason {
        match value {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "stop_sequence" => StopReason::StopSequence,
            "max_tokens" => StopReason::MaxTokens,
            "model_context_window_exceeded" => StopReason::ContextWindowExceeded,
            "pause_turn" => StopReason::PauseTurn,
            "refusal" => StopReason::Refusal,
            _ => StopReason::Unrecognized,
        }
    }

    /// Chat-completions `finish_reason` (OpenAI, Copilot, vLLM, local).
    pub fn chat_finish(value: &str) -> StopReason {
        match value {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "tool_calls" | "function_call" => StopReason::ToolUse,
            "content_filter" => StopReason::Safety,
            "abort" => StopReason::Incomplete,
            _ => StopReason::Unrecognized,
        }
    }

    /// OpenAI Responses `incomplete_details.reason`.
    pub fn responses_incomplete(reason: Option<&str>) -> StopReason {
        match reason {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Safety,
            _ => StopReason::Incomplete,
        }
    }

    /// Gemini `candidate.finishReason`.
    pub fn gemini_finish(value: &str) -> StopReason {
        match value {
            "STOP" => StopReason::EndTurn,
            "MAX_TOKENS" => StopReason::MaxTokens,
            "SAFETY" | "RECITATION" | "LANGUAGE" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
            | "IMAGE_SAFETY" | "IMAGE_PROHIBITED_CONTENT" | "IMAGE_RECITATION" | "IMAGE_OTHER"
            | "NO_IMAGE" => StopReason::Safety,
            "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" | "TOO_MANY_TOOL_CALLS" => {
                StopReason::MalformedToolCall
            }
            "OTHER" => StopReason::Incomplete,
            "FINISH_REASON_UNSPECIFIED" => StopReason::Unreported,
            _ => StopReason::Unrecognized,
        }
    }

    /// Gemini `promptFeedback.blockReason` values the API documents.
    pub fn gemini_block_reason_is_known(value: &str) -> bool {
        matches!(
            value,
            "BLOCK_REASON_UNSPECIFIED" | "SAFETY" | "OTHER" | "BLOCKLIST" | "PROHIBITED_CONTENT"
                | "IMAGE_SAFETY"
        )
    }
}
```

- [ ] **Step 4: Run the stop tests.**

Run: `cargo test -p rupu-providers --lib stop::`
Expected: PASS (6 tests).

- [ ] **Step 5: Switch `LlmResponse` to `stop: Stop`.**
  - In `types.rs`, delete the old `StopReason` enum and its doc comment.
  - Add `pub use crate::stop::{FallbackHop, RefusalDetail, RefusalSource, ServedBy, Stop, StopReason, WireStop};` near the top.
  - Change `pub stop_reason: Option<StopReason>,` to:
    ```rust
    /// How the reply ended — always present (spec 2026-10-01 §4.1).
    pub stop: Stop,
    ```
  - Update the two `types.rs` tests that serde-roundtrip `StopReason` (`test_stop_reason_serde`, `test_stop_reason_all_variants_serde`) to the new variants, or delete them, because `stop::tests` now covers this.

  Then fix every construction and read site. The `provider` string is the client's `ProviderId` display string. The `Some(x)` / `None` below refer to the old `stop_reason` values:

  | Site | Today | Change to |
  |---|---|---|
  | `anthropic.rs` `AnthropicResponse` | `stop_reason: Option<StopReason>` | `stop_reason: Option<String>`. In `into_llm_response`, use `Stop::from_wire(map::anthropic(v), "anthropic", Some(v))`, or `Stop::from_wire(StopReason::Unreported, "anthropic", None)` when `None`. This fixes the decode failure on an unknown value right away. |
  | `anthropic.rs` `StreamAccumulator.stop_reason` | `Option<StopReason>` | `Option<String>`, holding the raw value. In `message_delta`, store `reason.to_string()`. `into_response` builds the `Stop` the same way as the row above. |
  | `openai_codex.rs`, `openai_wire.rs`, `google_gemini.rs`, `broker_client.rs` accumulators and parse functions | `Some(StopReason::X)` | `Stop::synthetic(StopReason::X, "<provider>")` for now. Tasks 4–7 replace these with real wire mappings. |
  | `None` | | `Stop::from_wire(StopReason::Unreported, "<provider>", None)` |
  | `local.rs` | `stop_reason: Some(StopReason::EndTurn)` | `Stop::synthetic(StopReason::EndTurn, "local")`. Task 6 replaces this. |
  | `provider.rs`, `task_classifier.rs`, `router.rs`, `smart_router.rs`, `tuned.rs` test helpers; `tests/it/integration.rs`; `rupu-runtime/src/provider_factory.rs`; `rupu-orchestrator/tests/it/cancel_vs_completion.rs`; `rupu-agent/tests/it/runner_model_limits.rs` | `stop_reason: Some(StopReason::X)` | `stop: Stop::synthetic(StopReason::X, "mock")` |
  | Assertions | `assert_eq!(r.stop_reason, Some(StopReason::X))` | `assert_eq!(r.stop.reason, StopReason::X)` |
  | `rupu-agent/src/runner.rs` `MockProvider::send` | `stop_reason: Some(stop)` | `stop: Stop::synthetic(stop, self.provider_label())`. Task 8 adds `provider_label`; until then use `"mock"`. |
  | `rupu-agent/src/runner.rs` `TurnEnd`, around the `stop_reason: resp.stop_reason.as_ref().map(…)` block | the 4-arm match | `stop_reason: Some(resp.stop.wire_value_or_reason()),` |

  Find every site with:
  ```bash
  grep -rn "stop_reason" crates --include='*.rs' | grep -v "TurnEnd\|rupu-transcript\|jsonl_reader\|live_view"
  ```
  The transcript `TurnEnd.stop_reason: Option<String>` field itself is unchanged.

- [ ] **Step 6: Compile the workspace and run the touched crates' tests.**

  Run: `cargo check --workspace --all-targets`
  Expected: no errors.

  Run: `cargo test -p rupu-providers --lib && cargo test -p rupu-agent --lib`
  Expected: PASS. The Gemini test `test_map_finish_reason` fails, because `SAFETY` now maps to `Safety`. Update it to the new table (assert `SAFETY` → `Safety` and `UNKNOWN` → `Unrecognized`), since `map::gemini_finish` is now its source of truth. Repoint `map_finish_reason` in `google_gemini.rs` at `crate::stop::map::gemini_finish`; Task 7 finishes Gemini.

- [ ] **Step 7: Format the touched files** with `rustfmt --edition 2021` (check first; never `lib.rs`).

- [ ] **Step 8: Commit.**

```bash
git add -A crates/
git commit -m "feat(providers): typed Stop on every LlmResponse (response outcomes plan 1)

LlmResponse.stop_reason: Option<StopReason> becomes stop: Stop, which keeps
the provider's own wire value. StopReason grows to every documented ending
plus Unrecognized/Unreported; Anthropic's send no longer fails to decode an
unknown stop_reason.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Structured provider errors and the moved overflow parser

**Files:**
- Create: `crates/rupu-providers/src/reply_error.rs`, `crates/rupu-providers/src/overflow.rs`
- Modify: `crates/rupu-providers/src/lib.rs` (`pub mod overflow;` and `pub mod reply_error;`, alphabetical)
- Modify: `crates/rupu-providers/src/error.rs`
- Modify (migration): `anthropic.rs`, `openai_codex.rs`, `github_copilot.rs`, `openai_compatible.rs`, `google_gemini.rs`, `google_gemini/code_assist.rs`, `local.rs`, `broker_client.rs`, `tuned.rs`, `router.rs`, `smart_router.rs`, `provider.rs`, `types.rs` (tests)
- Modify: `crates/rupu-agent/src/runner.rs` (retry classifier; overflow imports; the `context_trim_tests` overflow cases move out), `crates/rupu-cli/src/cp_inventory.rs`, `crates/rupu-runtime/src/provider_factory.rs`, `crates/rupu-agent/tests/it/runner_model_limits.rs`
- Test: inline tests in `reply_error.rs` and `overflow.rs`

**Interfaces:**
- Produces:
  ```rust
  // reply_error.rs
  pub enum ErrorOrigin { Http { status: u16 }, Stream }
  pub enum ErrorClass { RateLimited, Overloaded, Server, Timeout, InvalidRequest, ContextOverflow,
      Auth, Permission, Quota, NotFound, TooLarge, Policy, Unrecognized }
  pub struct ApiErrorBody { pub provider: String, pub origin: ErrorOrigin, pub kind: Option<String>,
      pub message: String, pub request_id: Option<String>, pub details: Option<serde_json::Value>,
      pub raw: serde_json::Value, pub retry_after_secs: Option<u64>, pub class: ErrorClass }
  impl ApiErrorBody {
      pub fn status(&self) -> Option<u16>;
      pub fn is_retryable(&self) -> bool;
      pub fn retry_after(&self) -> Option<std::time::Duration>;
  }
  pub fn parse_error_body(provider: &str, origin: ErrorOrigin, text: &str,
      request_id_header: Option<String>, retry_after: Option<std::time::Duration>) -> ApiErrorBody;
  pub fn parse_error_value(provider: &str, origin: ErrorOrigin, value: &serde_json::Value) -> ApiErrorBody;
  pub fn classify(origin: ErrorOrigin, kind: Option<&str>, message: &str) -> ErrorClass;
  pub const RAW_CAP_BYTES: usize = 64 * 1024;
  // overflow.rs (moved from rupu-agent, now pub)
  pub struct Overflow { pub tokens: Option<u32>, pub max: Option<u32> }
  pub struct OutputCapOverflow { pub input: u32, pub max_tokens: u32, pub window: u32 }
  impl OutputCapOverflow { pub fn lowered_max_tokens(&self) -> Option<u32> }
  pub fn parse_context_overflow(err: &str) -> Option<Overflow>;
  pub fn parse_output_cap_overflow(err: &str) -> Option<OutputCapOverflow>;
  pub fn context_overflow_of(e: &ProviderError) -> Option<Overflow>;
  pub fn output_cap_overflow_of(e: &ProviderError) -> Option<OutputCapOverflow>;
  // error.rs
  ProviderError::Reply(Box<ApiErrorBody>)
  ProviderError::IncompleteStream(String)
  impl ProviderError {
      pub fn api(provider: &str, status: u16, message: &str) -> ProviderError; // tests + synthetic
      pub fn class(&self) -> ErrorClass;
      pub fn status(&self) -> Option<u16>;
      pub fn reply(&self) -> Option<&ApiErrorBody>;
  }
  pub fn api_error_from_response(provider: &str, status: u16, headers: &HeaderMap, body: &str) -> ProviderError;
  ```
- Removed: `ProviderError::{Api, RateLimited, Unauthorized, QuotaExceeded, ModelUnavailable, BadRequest}`.

- [ ] **Step 1: Move the overflow parser.**
  - Cut `Overflow`, `parse_context_overflow`, `OutputCapOverflow`, `OUTPUT_CAP_MARGIN`, `MIN_LOWERED_OUTPUT_CAP`, `impl OutputCapOverflow`, `parse_output_cap_overflow` and `numbers` from `crates/rupu-agent/src/runner.rs` into a new `crates/rupu-providers/src/overflow.rs`, verbatim.
  - Change `pub(crate)` to `pub`, and add a module doc: `//! Context-overflow error formats (spec 2026-09-30 §7), shared by the error classifier and the agent runner.`
  - Move the overflow cases from runner's `context_trim_tests` into `overflow.rs`'s own `#[cfg(test)] mod tests`: every test calling `parse_context_overflow` / `parse_output_cap_overflow`, with their `const` fixtures. Leave `trim_oldest_exchange` tests in the runner.
  - In `runner.rs`, add `use rupu_providers::overflow::{context_overflow_of, output_cap_overflow_of, Overflow, OutputCapOverflow};` (only the names it uses).

  Run: `cargo test -p rupu-providers --lib overflow::`
  Expected: PASS. These are the moved tests.

- [ ] **Step 2: Write the failing `reply_error` tests** in `crates/rupu-providers/src/reply_error.rs`. Every fixture below is invented.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn http(s: u16) -> ErrorOrigin { ErrorOrigin::Http { status: s } }

    #[test]
    fn anthropic_body_is_parsed() {
        let text = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_0001"}"#;
        let b = parse_error_body("anthropic", http(529), text, None, None);
        assert_eq!(b.kind.as_deref(), Some("overloaded_error"));
        assert_eq!(b.message, "Overloaded");
        assert_eq!(b.request_id.as_deref(), Some("req_0001"));
        assert_eq!(b.class, ErrorClass::Overloaded);
        assert!(b.is_retryable());
        assert_eq!(b.raw["error"]["type"], "overloaded_error");
    }

    #[test]
    fn openai_code_wins_over_generic_type() {
        let text = r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130001 tokens.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
        let b = parse_error_body("openai-codex", http(400), text, Some("req_hdr".into()), None);
        assert_eq!(b.kind.as_deref(), Some("context_length_exceeded"));
        assert_eq!(b.class, ErrorClass::ContextOverflow);
        assert_eq!(b.request_id.as_deref(), Some("req_hdr"));
        assert!(!b.is_retryable());
    }

    #[test]
    fn gemini_status_is_the_kind_and_array_wrapping_is_unwrapped() {
        let text = r#"[{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"7s"}]}}]"#;
        let b = parse_error_body("google-gemini-cli", http(429), text, None, None);
        assert_eq!(b.kind.as_deref(), Some("RESOURCE_EXHAUSTED"));
        assert_eq!(b.class, ErrorClass::RateLimited);
        assert!(b.details.is_some());
    }

    #[test]
    fn invalid_request_that_is_an_overflow_classifies_as_overflow() {
        let text = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#;
        let b = parse_error_body("anthropic", http(400), text, None, None);
        assert_eq!(b.class, ErrorClass::ContextOverflow);
    }

    #[test]
    fn non_json_body_is_kept_raw_and_classified_by_status() {
        let b = parse_error_body("openai-compatible", http(502), "<html>bad gateway</html>", None, None);
        assert_eq!(b.kind, None);
        assert_eq!(b.message, "<html>bad gateway</html>");
        assert_eq!(b.raw, serde_json::json!("<html>bad gateway</html>"));
        assert_eq!(b.class, ErrorClass::Server);
    }

    #[test]
    fn huge_bodies_are_capped_char_safely() {
        let text = "é".repeat(RAW_CAP_BYTES); // 2 bytes each → over the cap
        let b = parse_error_body("anthropic", http(500), &text, None, None);
        let raw = b.raw.as_str().unwrap();
        assert!(raw.len() <= RAW_CAP_BYTES);
        assert!(raw.chars().all(|c| c == 'é'));
    }

    #[test]
    fn status_table_when_kind_is_unknown() {
        let c = |s| classify(http(s), None, "x");
        assert_eq!(c(429), ErrorClass::RateLimited);
        assert_eq!(c(529), ErrorClass::Overloaded);
        assert_eq!(c(503), ErrorClass::Overloaded);
        assert_eq!(c(500), ErrorClass::Server);
        assert_eq!(c(502), ErrorClass::Server);
        assert_eq!(c(504), ErrorClass::Timeout);
        assert_eq!(c(408), ErrorClass::Timeout);
        assert_eq!(c(401), ErrorClass::Auth);
        assert_eq!(c(403), ErrorClass::Permission);
        assert_eq!(c(402), ErrorClass::Quota);
        assert_eq!(c(404), ErrorClass::NotFound);
        assert_eq!(c(413), ErrorClass::TooLarge);
        assert_eq!(c(400), ErrorClass::InvalidRequest);
        assert_eq!(c(418), ErrorClass::Unrecognized);
    }

    #[test]
    fn kind_table() {
        let k = |kind: &str| classify(http(400), Some(kind), "x");
        assert_eq!(k("rate_limit_error"), ErrorClass::RateLimited);
        assert_eq!(k("rate_limit_exceeded"), ErrorClass::RateLimited);
        assert_eq!(k("RESOURCE_EXHAUSTED"), ErrorClass::RateLimited);
        assert_eq!(k("overloaded_error"), ErrorClass::Overloaded);
        assert_eq!(k("UNAVAILABLE"), ErrorClass::Overloaded);
        assert_eq!(k("api_error"), ErrorClass::Server);
        assert_eq!(k("server_error"), ErrorClass::Server);
        assert_eq!(k("INTERNAL"), ErrorClass::Server);
        assert_eq!(k("timeout_error"), ErrorClass::Timeout);
        assert_eq!(k("DEADLINE_EXCEEDED"), ErrorClass::Timeout);
        assert_eq!(k("context_length_exceeded"), ErrorClass::ContextOverflow);
        assert_eq!(k("model_max_prompt_tokens_exceeded"), ErrorClass::ContextOverflow);
        assert_eq!(k("authentication_error"), ErrorClass::Auth);
        assert_eq!(k("UNAUTHENTICATED"), ErrorClass::Auth);
        assert_eq!(k("invalid_api_key"), ErrorClass::Auth);
        assert_eq!(k("permission_error"), ErrorClass::Permission);
        assert_eq!(k("PERMISSION_DENIED"), ErrorClass::Permission);
        assert_eq!(k("billing_error"), ErrorClass::Quota);
        assert_eq!(k("insufficient_quota"), ErrorClass::Quota);
        assert_eq!(k("not_found_error"), ErrorClass::NotFound);
        assert_eq!(k("NOT_FOUND"), ErrorClass::NotFound);
        assert_eq!(k("model_not_found"), ErrorClass::NotFound);
        assert_eq!(k("request_too_large"), ErrorClass::TooLarge);
        assert_eq!(k("bio_policy"), ErrorClass::Policy);
        assert_eq!(k("misalignment_policy_violation"), ErrorClass::Policy);
        assert_eq!(k("invalid_request_error"), ErrorClass::InvalidRequest);
        assert_eq!(k("INVALID_ARGUMENT"), ErrorClass::InvalidRequest);
        assert_eq!(k("invalid_prompt"), ErrorClass::InvalidRequest);
        // Unknown kind falls back to the HTTP status (400 here).
        assert_eq!(k("brand_new_kind"), ErrorClass::InvalidRequest);
    }

    #[test]
    fn stream_errors_with_unknown_kind_are_unrecognized_but_retryable() {
        let v = serde_json::json!({"type":"error","error":{"type":"brand_new_kind","message":"hmm"}});
        let b = parse_error_value("anthropic", ErrorOrigin::Stream, &v);
        assert_eq!(b.class, ErrorClass::Unrecognized);
        assert!(b.is_retryable());
    }

    #[test]
    fn display_keeps_the_familiar_shape() {
        let b = parse_error_body("anthropic", http(400), r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#, None, None);
        assert_eq!(b.to_string(), "API error 400: bad");
        let s = parse_error_value("anthropic", ErrorOrigin::Stream,
            &serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}));
        assert_eq!(s.to_string(), "stream error (overloaded_error): Overloaded");
    }
}
```

  Run: `cargo test -p rupu-providers --lib reply_error::`
  Expected: compile errors (module items missing).

- [ ] **Step 3: Implement `reply_error.rs`.**

```rust
//! A provider's error reply, parsed (spec 2026-10-01 response-outcomes §4.4).
//!
//! Every HTTP error body and every mid-stream error event becomes an
//! [`ApiErrorBody`]: the provider's own kind/code, message, request id and
//! the raw body, plus a normalized [`ErrorClass`] that retry logic and the
//! agent runner read instead of guessing from strings.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Raw bodies are kept up to this many bytes (cut on a char boundary).
pub const RAW_CAP_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErrorOrigin {
    Http { status: u16 },
    /// An error event inside a 200 stream.
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    RateLimited,
    Overloaded,
    Server,
    Timeout,
    InvalidRequest,
    ContextOverflow,
    Auth,
    Permission,
    Quota,
    NotFound,
    TooLarge,
    Policy,
    Unrecognized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub provider: String,
    pub origin: ErrorOrigin,
    /// Anthropic `error.type` · OpenAI `error.code` (else `error.type`) ·
    /// Gemini `error.status`.
    pub kind: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
    pub details: Option<serde_json::Value>,
    /// The body as received (JSON when it parsed, else a string), capped.
    pub raw: serde_json::Value,
    pub retry_after_secs: Option<u64>,
    pub class: ErrorClass,
}

impl ApiErrorBody {
    pub fn status(&self) -> Option<u16> {
        match self.origin {
            ErrorOrigin::Http { status } => Some(status),
            ErrorOrigin::Stream => None,
        }
    }

    /// Re-issuing the same request could plausibly succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.class,
            ErrorClass::RateLimited | ErrorClass::Overloaded | ErrorClass::Server | ErrorClass::Timeout
        ) || (self.origin == ErrorOrigin::Stream && self.class == ErrorClass::Unrecognized)
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after_secs.map(Duration::from_secs)
    }
}

impl std::fmt::Display for ApiErrorBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.origin, self.kind.as_deref()) {
            (ErrorOrigin::Http { status }, _) => write!(f, "API error {status}: {}", self.message),
            (ErrorOrigin::Stream, Some(kind)) => write!(f, "stream error ({kind}): {}", self.message),
            (ErrorOrigin::Stream, None) => write!(f, "stream error: {}", self.message),
        }
    }
}

fn cap_chars(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Parse an HTTP error body. `request_id_header` is the `request-id` /
/// `x-request-id` header; the body's own `request_id` wins when present.
pub fn parse_error_body(
    provider: &str,
    origin: ErrorOrigin,
    text: &str,
    request_id_header: Option<String>,
    retry_after: Option<Duration>,
) -> ApiErrorBody {
    let mut body = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => parse_error_value(provider, origin, &v),
        Err(_) => {
            let capped = cap_chars(text.trim(), RAW_CAP_BYTES).to_string();
            ApiErrorBody {
                provider: provider.to_string(),
                origin,
                kind: None,
                message: capped.clone(),
                request_id: None,
                details: None,
                raw: serde_json::Value::String(capped.clone()),
                retry_after_secs: None,
                class: classify(origin, None, &capped),
            }
        }
    };
    if text.len() > RAW_CAP_BYTES {
        body.raw = serde_json::Value::String(cap_chars(text, RAW_CAP_BYTES).to_string());
    }
    if body.request_id.is_none() {
        body.request_id = request_id_header;
    }
    body.retry_after_secs = retry_after.map(|d| d.as_secs());
    body
}

/// Parse an already-decoded error value (an SSE error event's data, a
/// `response.failed` error object, a chat-completions error chunk, …).
pub fn parse_error_value(provider: &str, origin: ErrorOrigin, value: &serde_json::Value) -> ApiErrorBody {
    // Gemini sometimes wraps the error in a one-element array.
    let v = match value {
        serde_json::Value::Array(items) if !items.is_empty() => &items[0],
        other => other,
    };
    let err = v.get("error").unwrap_or(v);
    let str_at = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let (kind, message, details) = match err {
        serde_json::Value::String(s) => (None, s.clone(), None),
        serde_json::Value::Object(_) => {
            let kind = str_at(err, "code")
                .or_else(|| str_at(err, "status"))
                .or_else(|| str_at(err, "type"));
            let message = str_at(err, "message")
                .or_else(|| str_at(err, "detail"))
                .unwrap_or_else(|| err.to_string());
            let details = err.get("details").or_else(|| err.get("param")).cloned();
            (kind, message, details)
        }
        other => (None, other.to_string(), None),
    };
    let class = classify(origin, kind.as_deref(), &message);
    ApiErrorBody {
        provider: provider.to_string(),
        origin,
        kind,
        message,
        request_id: str_at(v, "request_id"),
        details,
        raw: value.clone(),
        retry_after_secs: None,
        class,
    }
}

/// Provider kind/code first, then HTTP status. An `invalid_request`-class
/// error whose message is a context-overflow format is `ContextOverflow`.
pub fn classify(origin: ErrorOrigin, kind: Option<&str>, message: &str) -> ErrorClass {
    let by_kind = kind.and_then(|k| match k.to_ascii_lowercase().as_str() {
        "rate_limit_error" | "rate_limit_exceeded" | "resource_exhausted" => Some(ErrorClass::RateLimited),
        "overloaded_error" | "unavailable" => Some(ErrorClass::Overloaded),
        "api_error" | "server_error" | "internal" => Some(ErrorClass::Server),
        "timeout_error" | "deadline_exceeded" => Some(ErrorClass::Timeout),
        "context_length_exceeded" | "model_max_prompt_tokens_exceeded" => Some(ErrorClass::ContextOverflow),
        "authentication_error" | "unauthenticated" | "invalid_api_key" => Some(ErrorClass::Auth),
        "permission_error" | "permission_denied" => Some(ErrorClass::Permission),
        "billing_error" | "insufficient_quota" => Some(ErrorClass::Quota),
        "not_found_error" | "not_found" | "model_not_found" => Some(ErrorClass::NotFound),
        "request_too_large" => Some(ErrorClass::TooLarge),
        "bio_policy" | "misalignment_policy_violation" | "content_policy_violation"
        | "image_content_policy_violation" => Some(ErrorClass::Policy),
        "invalid_request_error" | "invalid_argument" | "invalid_prompt" => Some(ErrorClass::InvalidRequest),
        _ => None,
    });
    let class = by_kind.unwrap_or(match origin {
        ErrorOrigin::Http { status } => match status {
            429 => ErrorClass::RateLimited,
            529 | 503 => ErrorClass::Overloaded,
            504 | 408 => ErrorClass::Timeout,
            401 => ErrorClass::Auth,
            403 => ErrorClass::Permission,
            402 => ErrorClass::Quota,
            404 => ErrorClass::NotFound,
            413 => ErrorClass::TooLarge,
            400 | 422 => ErrorClass::InvalidRequest,
            s if s >= 500 => ErrorClass::Server,
            _ => ErrorClass::Unrecognized,
        },
        ErrorOrigin::Stream => ErrorClass::Unrecognized,
    });
    if class == ErrorClass::InvalidRequest && crate::overflow::parse_context_overflow(message).is_some() {
        return ErrorClass::ContextOverflow;
    }
    class
}
```

  Run: `cargo test -p rupu-providers --lib reply_error::`
  Expected: PASS.

- [ ] **Step 4: Rework `ProviderError`** in `error.rs`:
  - Delete the variants `Api`, `RateLimited`, `Unauthorized`, `QuotaExceeded`, `ModelUnavailable` and `BadRequest`.
  - Add these two variants:

```rust
    /// A provider's error reply — an HTTP error body or a mid-stream error
    /// event — parsed (spec 2026-10-01 §4.4).
    #[error("{0}")]
    Reply(Box<crate::reply_error::ApiErrorBody>),

    /// The stream ended before the provider finished the reply (e.g. an
    /// Anthropic stream with `message_start` but no `message_stop`).
    #[error("stream ended before the reply finished: {0}")]
    IncompleteStream(String),
```

and the helpers:

```rust
impl ProviderError {
    /// A synthetic HTTP error with a plain-text message — for tests and for
    /// errors rupu itself produces in the shape of a reply (router exhaustion).
    pub fn api(provider: &str, status: u16, message: &str) -> ProviderError {
        ProviderError::Reply(Box::new(crate::reply_error::parse_error_body(
            provider,
            crate::reply_error::ErrorOrigin::Http { status },
            message,
            None,
            None,
        )))
    }

    pub fn reply(&self) -> Option<&crate::reply_error::ApiErrorBody> {
        match self {
            ProviderError::Reply(b) => Some(b),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        self.reply().and_then(|b| b.status())
    }

    /// Normalized class for every variant (spec §4.4's fixed table for the
    /// errors that aren't a reply body).
    pub fn class(&self) -> crate::reply_error::ErrorClass {
        use crate::reply_error::ErrorClass as C;
        match self {
            ProviderError::Reply(b) => b.class,
            ProviderError::Http(_) | ProviderError::SseParse(_) | ProviderError::UnexpectedEndOfStream
            | ProviderError::IncompleteStream(_) | ProviderError::Transient(_) => C::Server,
            ProviderError::TokenRefreshFailed(_) | ProviderError::MissingAuth { .. }
            | ProviderError::AuthConfig(_) => C::Auth,
            ProviderError::LongContextUnavailable { .. } => C::ContextOverflow,
            ProviderError::Json(_) | ProviderError::Preflight(_) | ProviderError::NotImplemented { .. }
            | ProviderError::Other(_) | ProviderError::Terminating => C::Unrecognized,
        }
    }
}
```

  Then make these three changes in `error.rs`:
  - `is_long_context_refusal` matches `ProviderError::Reply(b) if b.status() == Some(429) && is_long_context_refusal(&b.message)`, alongside `LongContextUnavailable`.
  - Replace `api_error_from_response` with:

```rust
/// Build the error for a non-2xx HTTP response. `body` is the drained body
/// text; `headers` must come from the same response (grab them before
/// `.text()`). 429s carry the server's `Retry-After` (I-83).
pub fn api_error_from_response(provider: &str, status: u16, headers: &HeaderMap, body: &str) -> ProviderError {
    let request_id = ["request-id", "x-request-id"]
        .iter()
        .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()).map(str::to_string));
    let retry_after = if status == 429 { parse_retry_after(headers) } else { None };
    ProviderError::Reply(Box::new(crate::reply_error::parse_error_body(
        provider,
        crate::reply_error::ErrorOrigin::Http { status },
        body,
        request_id,
        retry_after,
    )))
}
```

  - Add tests to `error.rs`'s test module:
    - `api_error_from_response` on a 429 with `Retry-After: 7` gives `class() == RateLimited` and `reply().unwrap().retry_after() == Some(7s)`.
    - The long-context 429 body (`{"type":"error","error":{"type":"rate_limit_error","message":"Extra usage is required for long context requests"}}`) gives `is_long_context_refusal() == true`.

- [ ] **Step 5: Migrate every construction and match site.** Find them with:
  ```bash
  grep -rn "ProviderError::Api\|ProviderError::RateLimited\|Unauthorized {\|QuotaExceeded\|ModelUnavailable\|BadRequest {" crates --include='*.rs'
  ```

  | Site | New code |
  |---|---|
  | `anthropic.rs` `send` and `stream`: non-success bodies, and the 429 `last_err` | `crate::error::api_error_from_response("anthropic", status, &headers, &text)`. Capture `let headers = response.headers().clone();` before `.text()`. Delete both byte-slice truncations (`&text[..4096]`). This fixes the UTF-8 panic. Keep the long-context check (it reads `text`) before building the error. The "rate-limited after max retries" fallback becomes `ProviderError::api("anthropic", 429, "rate-limited after max retries")`. |
  | `anthropic.rs` model-list and probe error sites | `api_error_from_response("anthropic", …)` with the full text |
  | `openai_codex.rs` stream non-success | `api_error_from_response("openai-codex", status, &headers, &text)`; drop `truncate_error` there |
  | `openai_codex.rs` `response.failed` | handled in Task 5; for now `ProviderError::Reply(Box::new(parse_error_value("openai-codex", ErrorOrigin::Stream, &data["response"]["error"])))` |
  | `github_copilot.rs`, `openai_compatible.rs`, `google_gemini.rs`, `local.rs`, `broker_client.rs` HTTP error sites | `api_error_from_response("<provider>", status, &headers, &text)`. Gemini drops `extract_google_error` (delete the function), because the body parser reads `error.message` / `error.status` / `error.details` itself. |
  | Model-listing helper sites building `Api { status, message }` | `ProviderError::api("<provider>", status, &text)` |
  | `google_gemini/code_assist.rs` `duplicate_error` | `ProviderError::Reply(b) => ProviderError::Reply(b.clone())` in place of the `Api` and `RateLimited` arms |
  | `tuned.rs` `is_retryable` | `ProviderError::Reply(b) => b.is_retryable()`, plus `Transient` / `Http` / `IncompleteStream` → `true` |
  | `tuned.rs` retry-after extraction | `e.reply().and_then(|b| b.retry_after())` |
  | `tuned.rs` tests | `RateLimited { retry_after: None }` → `ProviderError::api("mock", 429, "slow down")`. Other `Api` fixtures → `ProviderError::api(...)` with the same status. |
  | `router.rs` failover | `Err(e) if matches!(e.class(), ErrorClass::RateLimited)` → next provider; `Err(e) if matches!(e.class(), ErrorClass::Auth \| ErrorClass::Permission)` → next provider (logging `status = e.status()`, `message = %e`). Keep the existing `TokenRefreshFailed` and `Http` arms ahead of these. |
  | `smart_router.rs` `is_retriable` and retry-after | Same treatment. The synthetic 503 errors become `ProviderError::api("router", 503, "…")`. The test assertions `matches!(err, ProviderError::Api { status: 503, .. })` become `assert_eq!(err.status(), Some(503))`. |
  | `provider.rs` test helpers | `ProviderError::api("mock", 500, "…")` |
  | `types.rs` `test_provider_error_display_messages` | `ProviderError::api("anthropic", 401, "Unauthorized")`, still asserting `401` and `Unauthorized` in the display string |
  | `rupu-cli/src/cp_inventory.rs` | `ProviderError::Api { status, .. } if *status == 401 \|\| *status == 403` → `e if matches!(e.class(), ErrorClass::Auth \| ErrorClass::Permission)`. `Err(ProviderError::RateLimited { .. }) => ProbeState::Ok` → `Err(e) if e.class() == ErrorClass::RateLimited => ProbeState::Ok`. Test fixtures → `ProviderError::api("anthropic", s, "…")`. |
  | `rupu-runtime/src/provider_factory.rs` test | `ProviderError::RateLimited { retry_after: None }` → `ProviderError::api("mock", 429, "slow down")` |

  Rewrite the runner retry classifier to:

```rust
fn is_retryable_provider_error(e: &rupu_providers::ProviderError) -> bool {
    use rupu_providers::ProviderError as E;
    if e.is_long_context_refusal() {
        return false;
    }
    match e {
        E::Reply(b) => b.is_retryable(),
        E::Http(_)
        | E::SseParse(_)
        | E::Json(_)
        | E::UnexpectedEndOfStream
        | E::IncompleteStream(_)
        | E::TokenRefreshFailed(_)
        | E::Transient(_) => true,
        E::MissingAuth { .. }
        | E::AuthConfig(_)
        | E::NotImplemented { .. }
        | E::Preflight(_)
        | E::LongContextUnavailable { .. }
        | E::Other(_) => false,
        // The process is exiting: nothing started now would finish.
        E::Terminating => false,
    }
}
```

- [ ] **Step 6: Read overflow from the error, not its display string.**
  - Add to `overflow.rs`:

```rust
use crate::error::ProviderError;
use crate::reply_error::ErrorClass;

/// Overflow read off an error (spec 2026-10-01 §4.4): a reply body's
/// message is parsed for numbers; a `ContextOverflow`-class reply is an
/// overflow even when no numbers parse. Errors without a body fall back to
/// their display text (the three generic phrases still match there).
pub fn context_overflow_of(e: &ProviderError) -> Option<Overflow> {
    match e {
        ProviderError::Reply(b) => parse_context_overflow(&b.message).or(
            (b.class == ErrorClass::ContextOverflow).then_some(Overflow { tokens: None, max: None }),
        ),
        other => parse_context_overflow(&other.to_string()),
    }
}

pub fn output_cap_overflow_of(e: &ProviderError) -> Option<OutputCapOverflow> {
    match e {
        ProviderError::Reply(b) => parse_output_cap_overflow(&b.message),
        other => parse_output_cap_overflow(&other.to_string()),
    }
}
```

  - Add tests:
    - `context_overflow_of(&ProviderError::api("openai-codex", 400, r#"{"error":{"message":"too big","code":"context_length_exceeded"}}"#))` is `Some(Overflow { tokens: None, max: None })`.
    - The Anthropic `prompt is too long: 215000 tokens > 200000 maximum` body gives `Some(Overflow { tokens: Some(215000), max: Some(200000) })`.
  - In `runner.rs`, replace `parse_output_cap_overflow(&e_str)` with `output_cap_overflow_of(&e)`, and `parse_context_overflow(&e_str)` with `context_overflow_of(&e)`. Keep `e_str` for the notices and `RunComplete.error`.

- [ ] **Step 7: Compile, test and format.**

  Run:
  ```bash
  cargo check --workspace --all-targets
  cargo test -p rupu-providers --lib
  cargo test -p rupu-providers --test it
  cargo test -p rupu-agent --lib
  cargo test -p rupu-agent --test it runner_model_limits::
  cargo test -p rupu-cli --lib cp_inventory
  ```
  Expected: PASS.

  The model-limits runner tests feed Anthropic-shaped overflow errors through `ScriptedTurn::ProviderError(String)`, which produces `ProviderError::Other`. They still match through the display-text fallback in `context_overflow_of`.

  Then format each touched file.

- [ ] **Step 8: Commit.**

```bash
git add -A crates/
git commit -m "feat(providers): parse error bodies into ApiErrorBody with an ErrorClass

ProviderError::Api / RateLimited become Reply(Box<ApiErrorBody>): the
provider's kind/code, message, request id, raw body (64 KB, char-safe) and a
normalized ErrorClass that every retry classifier now reads. The overflow
format table moves to rupu_providers::overflow and reads the parsed message.
Fixes the byte-slice truncation panic on non-ASCII Anthropic error bodies.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `ContentBlock::Unknown { provider, raw }` and `ContentBlock::Fallback`

**Files:**
- Modify: `crates/rupu-providers/src/types.rs` (enum, custom `Deserialize`, tests)
- Modify: the request builders in `anthropic.rs` (`build_request_body` / message conversion), `openai_codex.rs`, `openai_wire.rs`, `google_gemini.rs` and `local.rs`. They change only where they match `ContentBlock::Unknown`: the variant now has fields, so patterns become `ContentBlock::Unknown { .. }`. Anthropic additionally emits `Fallback`.
- Modify: every other `ContentBlock::Unknown` pattern in the workspace (`grep -rn "ContentBlock::Unknown" crates --include='*.rs'`)

**Interfaces:**
- Produces:
  ```rust
  ContentBlock::Unknown { provider: Option<String>, raw: serde_json::Value }   // serde tag "unknown"
  ContentBlock::Fallback { from_model: String, to_model: String }             // serde tag "fallback"
  ```
  Deserializing accepts the legacy `{"type":"Unknown"}`, and any unrecognized `type`, as `Unknown { provider: None, raw: <the whole object> }`.

- [ ] **Step 1: Replace the two `Unknown` tests in `types.rs` with failing ones.**

```rust
    #[test]
    fn unknown_block_type_keeps_its_payload() {
        let json = serde_json::json!({"type": "some_future_block", "payload": 1});
        let block: ContentBlock = serde_json::from_value(json.clone()).expect("must not error");
        assert_eq!(block, ContentBlock::Unknown { provider: None, raw: json });
    }

    #[test]
    fn legacy_unknown_literal_still_deserializes() {
        let block: ContentBlock = serde_json::from_value(serde_json::json!({"type": "Unknown"})).unwrap();
        assert!(matches!(block, ContentBlock::Unknown { provider: None, .. }));
    }

    #[test]
    fn unknown_and_fallback_round_trip() {
        for b in [
            ContentBlock::Unknown {
                provider: Some("anthropic".into()),
                raw: serde_json::json!({"type": "server_tool_use", "id": "srv_1"}),
            },
            ContentBlock::Fallback { from_model: "claude-opus-5-5".into(), to_model: "claude-opus-4-8".into() },
        ] {
            let v = serde_json::to_value(&b).unwrap();
            assert_eq!(serde_json::from_value::<ContentBlock>(v).unwrap(), b);
        }
        let v = serde_json::to_value(ContentBlock::Fallback { from_model: "a".into(), to_model: "b".into() }).unwrap();
        assert_eq!(v, serde_json::json!({"type": "fallback", "from_model": "a", "to_model": "b"}));
    }

    #[test]
    fn known_blocks_still_round_trip() {
        for b in [
            ContentBlock::Text { text: "hi".into() },
            ContentBlock::ToolUse { id: "t1".into(), name: "read_file".into(), input: serde_json::json!({"p": 1}) },
            ContentBlock::ToolResult { tool_use_id: "t1".into(), content: "ok".into(), is_error: false },
        ] {
            let v = serde_json::to_value(&b).unwrap();
            assert_eq!(serde_json::from_value::<ContentBlock>(v).unwrap(), b);
        }
    }
```

  Run: `cargo test -p rupu-providers --lib types::`
  Expected: compile errors.

- [ ] **Step 2: Change the enum.**
  - Keep `#[derive(Debug, Clone, PartialEq, Serialize)]` with `#[serde(tag = "type")]`, and drop `Deserialize` from the derive list.
  - Replace the `Unknown` variant with:

```rust
    /// An Anthropic server-side fallback boundary (`{"type":"fallback",
    /// "from":{"model":…},"to":{"model":…}}` on the wire). Echoed back to
    /// Anthropic in place; every other provider's request builder drops it.
    #[serde(rename = "fallback")]
    Fallback { from_model: String, to_model: String },

    /// A block rupu doesn't model, kept verbatim with its producing provider
    /// (spec 2026-10-01 §4.5). Rendered, never interpreted, and never sent
    /// back to a provider — every request builder drops it.
    #[serde(rename = "unknown")]
    Unknown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        raw: serde_json::Value,
    },
```

  - Add the custom `Deserialize` below the enum:

```rust
impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum Known {
            #[serde(rename = "text")]
            Text { text: String },
            #[serde(rename = "tool_use")]
            ToolUse { id: String, name: String, input: serde_json::Value },
            #[serde(rename = "tool_result")]
            ToolResult { tool_use_id: String, content: String, #[serde(default)] is_error: bool },
            #[serde(rename = "reasoning")]
            Reasoning {
                #[serde(default)]
                text: Option<String>,
                provider: String,
                model: String,
                raw: serde_json::Value,
            },
            #[serde(rename = "fallback")]
            Fallback { from_model: String, to_model: String },
            #[serde(rename = "unknown")]
            Unknown {
                #[serde(default)]
                provider: Option<String>,
                raw: serde_json::Value,
            },
        }
        let value = serde_json::Value::deserialize(d)?;
        let tag = value.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        let known = matches!(
            tag,
            "text" | "tool_use" | "tool_result" | "reasoning" | "fallback" | "unknown"
        );
        if !known {
            return Ok(ContentBlock::Unknown { provider: None, raw: value });
        }
        let k: Known = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(match k {
            Known::Text { text } => ContentBlock::Text { text },
            Known::ToolUse { id, name, input } => ContentBlock::ToolUse { id, name, input },
            Known::ToolResult { tool_use_id, content, is_error } => {
                ContentBlock::ToolResult { tool_use_id, content, is_error }
            }
            Known::Reasoning { text, provider, model, raw } => {
                ContentBlock::Reasoning { text, provider, model, raw }
            }
            Known::Fallback { from_model, to_model } => ContentBlock::Fallback { from_model, to_model },
            Known::Unknown { provider, raw } => ContentBlock::Unknown { provider, raw },
        })
    }
}
```

- [ ] **Step 3: Update the request builders and every other match on `ContentBlock`.**
  - Each provider's message conversion that today skips `ContentBlock::Unknown` now skips `ContentBlock::Unknown { .. } | ContentBlock::Fallback { .. }`.
  - **Anthropic is the exception.** In its assistant-message block conversion, add:

```rust
ContentBlock::Fallback { from_model, to_model } => Some(serde_json::json!({
    "type": "fallback",
    "from": { "model": from_model },
    "to": { "model": to_model },
})),
```

  - The existing tests `unknown_block_is_dropped_from_request` (anthropic.rs, and the same-named tests in the other providers, if any) construct `ContentBlock::Unknown`. Change those to `ContentBlock::Unknown { provider: None, raw: serde_json::json!({"type": "x"}) }`.
  - Add an Anthropic test, `fallback_block_is_echoed_in_wire_shape`, using `request_with_assistant_blocks(vec![ContentBlock::Fallback{..}, ContentBlock::Text{..}])`. It asserts the first assistant block in the body is `{"type":"fallback","from":{"model":"a"},"to":{"model":"b"}}`.
  - Add the same check for Codex, chat completions and Gemini: a `Fallback` block is absent from the body.
  - Fix every remaining `ContentBlock::Unknown` pattern the compiler reports, in `rupu-agent` (replay, runner `emit_turn_content`), `rupu-cli` and `rupu-cp`. Existing behavior stays unchanged: those sites skip the block. Plan 3 renders it.

- [ ] **Step 4: Run the tests.**

  Run:
  ```bash
  cargo check --workspace --all-targets
  cargo test -p rupu-providers --lib
  cargo test -p rupu-agent --lib
  ```
  Expected: PASS.

- [ ] **Step 5: Format and commit.**

```bash
git add -A crates/
git commit -m "feat(providers): keep unknown content blocks with their payload; typed fallback block

ContentBlock::Unknown now carries { provider, raw } (legacy {\"type\":\"Unknown\"}
still parses) and ContentBlock::Fallback models Anthropic's server-side
fallback boundary, echoed only to Anthropic.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Anthropic — every reply shape

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs` (`process_sse_event`, `StreamAccumulator`, `into_response`, `parse_content_blocks`, `AnthropicResponse::into_llm_response`, `stream`'s end-of-stream handling)
- Test: the inline `#[cfg(test)]` module in `anthropic.rs`, using the existing `process_sse_events` test harness that feeds `SseEvent`s (see `test_process_sse_events_full_text_stream`)

**Interfaces:**
- Consumes: `Stop`, `map::anthropic`, `RefusalDetail`, `RefusalSource`, `ServedBy`, `FallbackHop` (Task 1); `parse_error_value`, `ErrorOrigin` (Task 2); `ContentBlock::{Unknown, Fallback}` (Task 3).
- Produces: `StreamAccumulator::into_response(self) -> Result<LlmResponse, ProviderError>` (was `Option`).

**Behaviors:**

| Situation | Result |
|---|---|
| `message_delta.delta.stop_reason` | `Stop::from_wire(map::anthropic(v), "anthropic", Some(v))` |
| `message_delta.delta.stop_sequence` (string) | `stop.set_detail("stop_sequence", …)` |
| `delta.stop_details` (stream) or top-level `stop_details` (send), when not null | `RefusalDetail { category, explanation, recommended_model, source: Classifier }`. The source is `Classifier` when `category` is non-null, else `Model`. |
| `content_block_start` with `type: "fallback"` | Push `ContentBlock::Fallback { from_model: block.from.model, to_model: block.to.model }` and record a `FallbackHop` |
| `usage.iterations` (from `message_delta.usage`, or the `send` body's `usage`) containing a `fallback_message` entry, or any recorded hop | `served_by = ServedBy { model: <last hop's to_model, else the iteration entry's model, else response model>, hops }` |
| `content_block_start` of any type other than `text` / `tool_use` / `thinking` / `redacted_thinking` / `fallback` | `ContentBlock::Unknown { provider: Some("anthropic"), raw: block }`. Its deltas are ignored (debug-logged as today). |
| `content_block_start` with `type: "text"` carrying non-empty initial `text` | Seed `acc.text` with it. Today it's dropped by `_ => {}`. |
| `event: error` (SSE event type `error`) | `return Err(ProviderError::Reply(Box::new(parse_error_value("anthropic", ErrorOrigin::Stream, &data))))` |
| Stream ends after `message_start` without `message_stop` | `Err(ProviderError::IncompleteStream("anthropic: no message_stop".into()))` |
| Stream ends with no `message_start` | `Err(ProviderError::UnexpectedEndOfStream)`, as today |
| `content_block_stop` for a tool whose JSON fails to parse | Don't return `Json`. Record `acc.bad_tool = Some(json!({"name": name, "id": id, "error": e.to_string()}))` and drop the block. In `into_response`: if `stop.reason == MaxTokens`, call `stop.set_detail("truncated_tool", bad_tool)`; otherwise set `stop.reason = MalformedToolCall` and `stop.set_detail("malformed_tool", bad_tool)`. |
| Non-streaming `parse_content_blocks` | Unknown blocks become `ContentBlock::Unknown { provider: Some("anthropic"), raw }` instead of being dropped; `fallback` blocks become `ContentBlock::Fallback`. `AnthropicResponse` gains `#[serde(default)] stop_sequence: Option<String>` and `#[serde(default)] stop_details: Option<serde_json::Value>`; `usage` becomes `serde_json::Value`, re-deserialized into `AnthropicWireUsage` with `iterations` read alongside. |

Put the shared stop-building in one helper so the stream and `send` agree:

```rust
/// One place for both paths (stream + send): stop_reason, stop_sequence,
/// stop_details and fallback hops/iterations → a `Stop`.
fn anthropic_stop(
    stop_reason: Option<&str>,
    stop_sequence: Option<&str>,
    stop_details: Option<&serde_json::Value>,
    hops: Vec<FallbackHop>,
    iterations: Option<&serde_json::Value>,
    response_model: &str,
) -> Stop {
    let mut stop = match stop_reason {
        Some(v) => Stop::from_wire(map::anthropic(v), "anthropic", Some(v)),
        None => Stop::from_wire(StopReason::Unreported, "anthropic", None),
    };
    if let Some(seq) = stop_sequence {
        stop.set_detail("stop_sequence", serde_json::json!(seq));
    }
    if let Some(d) = stop_details.filter(|d| !d.is_null()) {
        let s = |k: &str| d.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let category = s("category");
        stop.refusal = Some(RefusalDetail {
            source: if category.is_some() { RefusalSource::Classifier } else { RefusalSource::Model },
            category,
            explanation: s("explanation"),
            recommended_model: s("recommended_model"),
        });
        stop.set_detail("stop_details", d.clone());
    }
    let fallback_entry = iterations
        .and_then(|it| it.as_array())
        .and_then(|it| it.iter().rev().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("fallback_message")))
        .and_then(|e| e.get("model").and_then(|m| m.as_str()).map(str::to_string));
    if !hops.is_empty() || fallback_entry.is_some() {
        let model = hops
            .last()
            .map(|h| h.to_model.clone())
            .or(fallback_entry)
            .unwrap_or_else(|| response_model.to_string());
        stop.served_by = Some(ServedBy { model, hops });
    }
    stop
}
```

- [ ] **Step 1: Write the failing tests.** Each test builds the SSE event sequence inline, as the existing `test_process_sse_events_*` tests do, and asserts on the resulting `LlmResponse` or error:
  1. `refusal_stream_carries_stop_details`:
     - `message_start`; text block "Partial"; `message_delta` with `{"delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":"Declined for this example."}},"usage":{"output_tokens":3}}`; `message_stop`.
     - Assert `reason == Refusal`, `wire.value == Some("refusal")`, `refusal.category == Some("cyber")`, `source == Classifier`, and the text is kept as "Partial". Discarding the partial output is Plan 2's job.
  2. `refusal_with_null_category_is_model_sourced`: `stop_details` with `"category":null,"explanation":null` gives `source == Model`.
  3. `pause_turn_and_context_window_exceeded_are_typed`: two runs, with `stop_reason` `pause_turn` and `model_context_window_exceeded`.
  4. `unknown_stop_reason_is_unrecognized_with_wire_value`: `"stop_reason":"brand_new_reason"` gives `Unrecognized` and `wire.value == Some("brand_new_reason")`.
  5. `stop_sequence_is_recorded`: `"stop_reason":"stop_sequence","stop_sequence":"</done>"` gives `details.stop_sequence == "</done>"`.
  6. `sse_error_event_is_a_stream_reply_error`:
     - Event type `error` with data `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`.
     - Feeding it to `process_sse_event` returns `Err(ProviderError::Reply(b))` with `b.origin == ErrorOrigin::Stream`, `b.class == ErrorClass::Overloaded` and `b.is_retryable()`.
  7. `missing_message_stop_is_incomplete_stream`: `message_start` plus a text delta, no `message_stop`. `into_response()` returns `Err(ProviderError::IncompleteStream(_))`.
  8. `truncated_tool_json_under_max_tokens_is_dropped_and_reported`: a `tool_use` block start, `input_json_delta` `{"path": "a`, `content_block_stop`, then `message_delta` `stop_reason: max_tokens`, `message_stop`. Expect: no `ToolUse` in content, `reason == MaxTokens`, `details.truncated_tool.name == "read_file"`.
  9. `bad_tool_json_without_max_tokens_is_malformed_tool_call`: as test 8, but with `stop_reason: tool_use`. Expect `reason == MalformedToolCall` and `details.malformed_tool` present.
  10. `server_side_fallback_blocks_and_iterations_set_served_by`:
      - `message_start` with model `claude-opus-5-5`; `content_block_start` `{"type":"fallback","from":{"model":"claude-opus-5-5"},"to":{"model":"claude-opus-4-8"}}`; `content_block_stop`; text; `message_delta` with `stop_reason: end_turn` and `usage.iterations: [{"type":"message","model":"claude-opus-5-5"},{"type":"fallback_message","model":"claude-opus-4-8"}]`; `message_stop`.
      - Expect: content starts with `ContentBlock::Fallback { from_model: "claude-opus-5-5", to_model: "claude-opus-4-8" }`, `served_by.model == "claude-opus-4-8"`, and one hop.
  11. `unknown_content_block_is_kept`: `content_block_start` `{"type":"server_tool_use","id":"srv_1","name":"web_search","input":{}}` gives a `ContentBlock::Unknown { provider: Some("anthropic"), raw }` whose `raw["id"] == "srv_1"`.
  12. `send_decodes_refusal_and_unknown_stop_reason`: `serde_json::from_str::<AnthropicResponse>` on a body with `"stop_reason":"refusal","stop_details":{…}` and on one with `"stop_reason":"brand_new_reason"`. Both decode, and `into_llm_response()` gives `Refusal` / `Unrecognized`. Today the second fails.
  13. `send_keeps_unknown_blocks`: replaces `non_streaming_unknown_block_type_is_dropped_not_fatal`, asserting the block is kept as `Unknown` instead.

  Run: `cargo test -p rupu-providers --lib anthropic::tests::`
  Expected: the new tests fail.

- [ ] **Step 2: Implement.**
  - **Accumulator fields:** add `stop_sequence: Option<String>`, `stop_details: Option<serde_json::Value>`, `hops: Vec<FallbackHop>`, `iterations: Option<serde_json::Value>`, `bad_tool: Option<serde_json::Value>` and `saw_message_stop: bool` to `StreamAccumulator`.
  - **`process_sse_event`:**
    - Set `acc.saw_message_stop = true` on `message_stop`.
    - Add the arm `"error" => { let data: serde_json::Value = serde_json::from_str(&event.data)?; return Err(ProviderError::Reply(Box::new(crate::reply_error::parse_error_value("anthropic", crate::reply_error::ErrorOrigin::Stream, &data)))); }`.
    - Extend `content_block_start` and `content_block_stop` per the behaviors table.
    - In `message_delta`, read `delta.stop_sequence`, `delta.stop_details` and `usage.iterations`.
  - **`into_response`** returns `Result<LlmResponse, ProviderError>`:
    - `id` empty → `Err(UnexpectedEndOfStream)`.
    - `!saw_message_stop` → `Err(IncompleteStream(…))`.
    - Otherwise build the `Stop` with `anthropic_stop(..)`, then apply the `bad_tool` rule.
  - **`stream()`:** the existing `accumulator.into_response().ok_or(ProviderError::UnexpectedEndOfStream)` becomes `accumulator.into_response()`.
  - **Tests:** update the existing tests that call `into_response()` and expect `Some`/`None`:
    - `test_stream_accumulator_text_only`, `test_stream_accumulator_with_tool_use`: set `saw_message_stop = true` in the test, and `.unwrap()` the `Result`.
    - `test_stream_accumulator_empty_returns_none` → assert `Err(UnexpectedEndOfStream)`.

- [ ] **Step 3: Run the tests.**

  Run: `cargo test -p rupu-providers --lib anthropic::`
  Expected: PASS.

  Run: `cargo test -p rupu-runtime --test it anthropic_prompt_cache`
  Expected: PASS. That test streams through a local fake server: if its fake SSE omits `message_stop`, add one there.

- [ ] **Step 4: Format and commit.**

```bash
git add -A crates/
git commit -m "feat(providers): Anthropic parses every reply shape

All seven stop_reasons typed (refusal with stop_details, pause_turn,
model_context_window_exceeded), stop_sequence recorded, server-side fallback
blocks + usage.iterations → served_by, SSE error events become stream
errors, a stream without message_stop is IncompleteStream, unknown blocks are
kept, and truncated tool JSON is reported instead of retried as a Json error.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Codex / OpenAI Responses — every reply shape

**Files:**
- Modify: `crates/rupu-providers/src/openai_codex.rs` (`process_sse_event`, `ResponseAccumulator`, `into_response`, `parse_response`)
- Test: the inline module in `openai_codex.rs`, following the `test_sse_text_streaming` pattern

**Interfaces:**
- Consumes: Task 1 (`map::responses_incomplete`, `Stop`, `RefusalDetail`), Task 2 (`parse_error_value`, `ErrorOrigin`).
- Produces: `ResponseAccumulator::into_response(self) -> Result<LlmResponse, ProviderError>`.

**Behaviors:**

| Event / shape | Result |
|---|---|
| `response.completed` | `EndTurn`, or `ToolUse` when `output` has a `function_call`. Wire value `"completed"`. |
| `response.incomplete` | Usage is read exactly as in `completed` (factor that block into `fn absorb_terminal_response(&mut acc, resp, on_event)`). Reason is `map::responses_incomplete(resp.incomplete_details.reason)`, and the wire value is that reason (or `"incomplete"`). `details.incomplete_details` holds the object. A `function_call` does **not** override an incomplete stop. |
| `response.completed` with `status: "incomplete"` (legacy shape) | Same as `response.incomplete` |
| status `cancelled` | `Incomplete`, wire value `"cancelled"` |
| `response.refusal.delta` | Append to `acc.refusal` and emit nothing. A refusal is not answer text. |
| `response.refusal.done` | `acc.refusal = data.refusal` |
| An `output_item.done` message item whose `content` has a part with `type: "refusal"` | `acc.refusal = part.refusal` |
| `acc.refusal` non-empty at the end | `reason = Refusal`, wire value `"refusal"`, `refusal = RefusalDetail { category: None, explanation: Some(text), recommended_model: None, source: Model }` |
| `response.failed` | `Err(Reply(parse_error_value("openai-codex", Stream, &data["response"]["error"])))`. When `error` is null, use `json!({"message": "response failed (no details)"})`. Policy codes classify as `Policy` through `classify`. |
| Top-level `error` event (`{"type":"error","code":…,"message":…,"param":…}`) | `Err(Reply(parse_error_value("openai-codex", Stream, &data)))` |
| Function arguments that fail to parse (`output_item.done`) | Same rule as Anthropic: record `bad_tool`, drop the block; at the end, `MaxTokens` → `truncated_tool`, else `MalformedToolCall` with `malformed_tool` |
| Stream ends with no terminal event (`completed` / `incomplete` / `failed`) but an id was set | `Err(IncompleteStream("openai-codex: no terminal response event"))` |
| `parse_response` (non-streaming; tests only) | The same rules through the same helper, so the two paths can't drift again. Remove `#[allow(dead_code)]` only if it becomes used; leave it otherwise. |

- [ ] **Step 1: Write the failing tests.** Inline SSE fixtures, invented:
  1. `incomplete_event_max_output_tokens` → `MaxTokens`, wire value `max_output_tokens`, usage read.
  2. `incomplete_event_content_filter` → `Safety`.
  3. `incomplete_with_function_call_stays_incomplete` → the reason stays `MaxTokens` even with a `function_call` item.
  4. `refusal_events_become_refusal` → `response.refusal.delta` ×2 plus `refusal.done` gives `Refusal`, with the explanation text and `source == Model`, and no `TextDelta` emitted for it. Use a collecting callback to assert that.
  5. `refusal_content_part_becomes_refusal` → an `output_item.done` with `{"type":"message","content":[{"type":"refusal","refusal":"Not able to help with that example."}]}`.
  6. `failed_event_is_a_classified_stream_error` → `response.failed` with `{"error":{"code":"server_error","message":"boom"}}` gives `Reply`, `origin == Stream`, `class == Server`, retryable. Update `test_sse_failed_event` and `test_sse_failed_event_without_response_key`, which assert `Api{500}`.
  7. `failed_event_policy_code_is_policy` → `code: "bio_policy"` gives `class == Policy`, not retryable.
  8. `error_event_is_a_stream_error` → top-level `{"type":"error","code":"rate_limit_exceeded","message":"slow down","param":null}` gives `class == RateLimited`.
  9. `bad_arguments_under_max_tokens_are_truncated_tool`.
  10. `no_terminal_event_is_incomplete_stream`.

  Run: `cargo test -p rupu-providers --lib openai_codex::`
  Expected: the new tests fail.

- [ ] **Step 2: Implement** per the table.
  - Add `refusal: String`, `bad_tool: Option<serde_json::Value>`, `stop: Option<Stop>` and `terminal_seen: bool` to `ResponseAccumulator`.
  - `into_response` returns `Result`. It uses `stop.unwrap_or(Stop::from_wire(Unreported, "openai-codex", None))`, then applies the refusal override and then the `bad_tool` rule.
  - `stream()` returns `acc.into_response()`.

- [ ] **Step 3: Run the tests.**

  Run: `cargo test -p rupu-providers --lib openai_codex::`
  Expected: PASS.

- [ ] **Step 4: Format and commit.**

```bash
git add -A crates/
git commit -m "feat(providers): Codex/Responses parses every reply shape

response.incomplete (max_output_tokens/content_filter/other), refusal deltas
and refusal parts, response.failed and error events as classified stream
errors (no more invented Api 500), cancelled/missing terminal events, and
truncated tool arguments.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Chat completions (Copilot, OpenAI-compatible, vLLM, local)

**Files:**
- Modify: `crates/rupu-providers/src/openai_wire.rs` (`parse_chat_completion`, `CompletionAccumulator`, `into_response`, `process_completion_sse`)
- Modify: `crates/rupu-providers/src/github_copilot.rs`, `crates/rupu-providers/src/openai_compatible.rs`. Their stop assertions change to `stop.reason`, and the streaming `into_response()` callers change for the `Result` return.
- Modify: `crates/rupu-providers/src/local.rs` (`send`)
- Test: the inline modules in `openai_wire.rs` and `local.rs`

**Interfaces:**
- Consumes: `map::chat_finish`, `Stop`, `RefusalDetail`, `parse_error_value`.
- Produces:
  - `parse_chat_completion(json: &serde_json::Value, provider: &str) -> Result<LlmResponse, ProviderError>`. It gains `provider`, so the `Stop` and error bodies are attributed to `github-copilot` / `openai-compatible` / `local`.
  - `CompletionAccumulator::new(provider: &str)`.
  - `into_response(self) -> Result<Option<LlmResponse>, ProviderError>`. `Ok(None)` keeps today's "nothing arrived" meaning; callers keep their `UnexpectedEndOfStream`.

**Behaviors:**

| Shape | Result |
|---|---|
| `finish_reason` string | `Stop::from_wire(map::chat_finish(v), provider, Some(v))`; tool calls present plus `stop` → `ToolUse` (keep wire value `stop`) |
| `finish_reason` null or absent at the end | `Unreported` (`ToolUse` if tool calls are present) |
| `message.refusal` non-null string (send), or accumulated `delta.refusal` (stream) | `reason = Refusal`, `source = Model`, `explanation = Some(text)`. The refusal text is not added as a `Text` block. |
| A chunk with an `error` object (stream) | `Err(Reply(parse_error_value(provider, Stream, &data)))` |
| A non-streaming body with `error` and no `choices` | `Err(Reply(parse_error_value(provider, Http{200}, json)))` |
| Streamed tool arguments that don't parse | No `{}` dispatch. Record `bad_tool` and drop the call; `MaxTokens` → `truncated_tool`, else `MalformedToolCall`. |
| Non-streaming tool arguments that don't parse | Same rule. This replaces the `Json` error. |
| `local.rs` `send` | Read `choices[0].finish_reason` through `map::chat_finish` with provider `"local"`; `null` → `Unreported`; read `message.refusal` the same way. |

- [ ] **Step 1: Write the failing tests** in `openai_wire.rs`'s test module:
  1. `content_filter_is_safety` (send and stream).
  2. `message_refusal_is_refusal_not_text` (send).
  3. `delta_refusal_accumulates_into_refusal` (stream).
  4. `vllm_abort_is_incomplete`.
  5. `unknown_finish_reason_is_unrecognized_with_wire_value`.
  6. `null_finish_reason_is_unreported`.
  7. `error_chunk_is_a_stream_error` (`data: {"error":{"message":"upstream overloaded","type":"server_error","code":null}}`) gives `class == Server`, retryable.
  8. `streamed_bad_arguments_never_dispatch_empty_object`: after `length` they give `truncated_tool`; after `tool_calls` they give `MalformedToolCall`, and no `ToolUse` block either way.
  9. `local_reads_finish_reason`, in `local.rs`. Feed `local`'s response parser directly. If parsing is inline in `send`, extract `fn parse_local_response(json) -> Result<LlmResponse, ProviderError>` first and test that.

  Run: `cargo test -p rupu-providers --lib openai_wire:: local::`
  Expected: the new tests fail.

- [ ] **Step 2: Implement** per the table.
  - Thread `provider` from `github_copilot.rs` (`"github-copilot"`), `openai_compatible.rs` (`"openai-compatible"`) and `local.rs` (`"local"`).

- [ ] **Step 3: Run the tests.**

  Run: `cargo test -p rupu-providers --lib`
  Expected: PASS.

  Run: `cargo test -p rupu-providers --test it`
  Expected: PASS.

- [ ] **Step 4: Format and commit.**

```bash
git add -A crates/
git commit -m "feat(providers): chat-completions parses every finish reason, refusals and error chunks

content_filter → Safety, message/delta refusal → Refusal, vLLM abort,
unknown/null finish reasons typed, mid-stream error chunks surfaced, and
unparseable tool arguments reported instead of dispatched as {}.
local.rs reads finish_reason instead of hardcoding EndTurn.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Gemini — every finish reason, prompt blocks and error chunks

**Files:**
- Modify: `crates/rupu-providers/src/google_gemini.rs` (`parse_generate_content_response`, `map_finish_reason` deleted in favor of `crate::stop::map::gemini_finish`, `GeminiAccumulator`, `into_response`, `process_gemini_sse`)
- Test: the inline module in `google_gemini.rs`

**Interfaces:**
- Consumes: `map::gemini_finish`, `map::gemini_block_reason_is_known`, `Stop`, `parse_error_value`.
- Produces: `GeminiAccumulator::new(model: &str, provider: &str)`; `into_response(self) -> Option<LlmResponse>`. It returns `Some` when a stop was recorded, even with no parts.

**Behaviors:**

| Shape | Result |
|---|---|
| `candidate.finishReason` | `Stop::from_wire(map::gemini_finish(v), provider, Some(v))`. `finishMessage` → `details.finishMessage`; `safetyRatings` → `details.safetyRatings`. |
| `STOP` with a `functionCall` part in this turn | `ToolUse`, wire value `STOP`. This fixes `STOP` overriding `ToolUse`. |
| No candidates and `promptFeedback.blockReason` present | `Stop::from_wire(Safety, provider, Some(blockReason))`, with `details.prompt_blocked = true` and `details.safetyRatings` from `promptFeedback`. An unknown `blockReason` (`!gemini_block_reason_is_known`) is still `Safety`, with its wire value kept. |
| `data.error` in a stream chunk | `Err(Reply(parse_error_value(provider, Stream, &data)))` |
| Response with candidates but no `finishReason` (end of stream) | `Unreported`, or `ToolUse` if there were function calls |

`provider` is `self.variant.provider_id().to_string()` (`google-gemini-cli` / `google-antigravity`). The Code Assist `{response: …}` envelope is already unwrapped by `generate_content_payload` (#721), so no change is needed there. Use it for `promptFeedback` as well.

- [ ] **Step 1: Write the failing tests:**
  1. `safety_finish_reason_is_safety_with_details`: `finishReason: SAFETY`, `finishMessage`, `safetyRatings`.
  2. `recitation_and_prohibited_content_are_safety`.
  3. `malformed_function_call_is_malformed_tool_call`.
  4. `stop_with_function_call_is_tool_use`, in both stream and send. Today `test_parse_response_function_call` uses `FUNCTION_CALLING`: switch those fixtures to the real `STOP` and assert `ToolUse`.
  5. `prompt_blocked_send`: `{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT","safetyRatings":[]}}` with no candidates gives `Safety` and `prompt_blocked == true`.
  6. `prompt_blocked_stream`: the same body as one SSE chunk. `into_response()` is `Some(..)` with `Safety`, instead of today's `None` → `UnexpectedEndOfStream`.
  7. `stream_error_chunk_is_a_stream_error`: `{"error":{"code":503,"message":"The model is overloaded.","status":"UNAVAILABLE"}}` gives `class == Overloaded`.
  8. `other_is_incomplete_and_unknown_is_unrecognized`.
  9. Update the existing `FUNCTION_CALLING` / `STOP_SEQUENCE` fixtures. `FUNCTION_CALLING` is not a Gemini value: replace it with `STOP` where the test means "tool call", and keep one test asserting `FUNCTION_CALLING` → `Unrecognized` (with a `functionCall` part present, the stop is still `ToolUse`, because the function call wins over an unrecognized value).

  Run: `cargo test -p rupu-providers --lib google_gemini::`
  Expected: the new tests fail.

- [ ] **Step 2: Implement.**
  - Tool-use precedence goes in one helper used by both paths:
    ```rust
    fn gemini_stop(finish: Option<&str>, had_function_call: bool, provider: &str) -> Stop
    ```
    - If `finish` is `STOP`, `None` or unrecognized, and `had_function_call`, the result is `ToolUse` (wire value kept).
    - `MaxTokens` / `Safety` / `MalformedToolCall` / `Incomplete` always win over the function call.

- [ ] **Step 3: Run the tests.**

  Run: `cargo test -p rupu-providers --lib google_gemini::`
  Expected: PASS.

- [ ] **Step 4: Format and commit.**

```bash
git add -A crates/
git commit -m "feat(providers): Gemini parses every finish reason, prompt blocks and error chunks

SAFETY/RECITATION/PROHIBITED_CONTENT/… → Safety with finishMessage and
safetyRatings, MALFORMED_FUNCTION_CALL family → MalformedToolCall, OTHER →
Incomplete, promptFeedback.blockReason → a prompt-blocked Safety stop on both
paths, STOP no longer overrides a function call, and stream error chunks are
surfaced.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Broker, provider identity, and mock scripting

**Files:**
- Modify: `crates/rupu-providers/src/provider_id.rs`, `crates/rupu-providers/src/local.rs`, `crates/rupu-providers/src/broker_client.rs`
- Modify: `crates/rupu-agent/src/runner.rs` (`ScriptedTurn`, `MockProvider`)
- Test: the inline modules in `provider_id.rs` and `broker_client.rs`; a new `crates/rupu-agent/tests/it/mock_provider_outcomes.rs`, registered in `crates/rupu-agent/tests/it/main.rs`

**Interfaces:**
- Produces:
  ```rust
  ProviderId::Local, ProviderId::Broker        // not in ProviderId::ALL; Display "local" / "broker"
  impl ProviderId { pub fn env_var_name(&self) -> Option<&'static str> }   // None for Local/Broker
  ScriptedTurn::Reply { content: Vec<ContentBlock>, stop: Stop, #[serde(default)] usage: Usage }
  ScriptedTurn::ReplyError { body: ApiErrorBody }
  impl MockProvider {
      pub fn with_provider_id(self, id: ProviderId) -> Self;   // default ProviderId::Anthropic
      fn provider_label(&self) -> &'static str;                // "mock"
  }
  ```

- [ ] **Step 1: Write the failing tests.**
  - `provider_id.rs`:
    - `ProviderId::Local.to_string() == "local"` and `ProviderId::Broker.to_string() == "broker"`.
    - Neither is in `ProviderId::ALL`.
    - `env_var_name()` is `None` for both, and `Some("ANTHROPIC_API_KEY")` for `Anthropic`.
    - Update `test_env_var_name` to the `Option` return.
  - `local.rs`: `LocalModelProvider::provider_id() == ProviderId::Local`.
  - `broker_client.rs`:
    - `provider_id() == ProviderId::Broker`.
    - A `send` response body with `"stop_reason":"refusal"` parses to `StopReason::Refusal` with wire value `refusal`. Extract `fn parse_broker_response(resp: &serde_json::Value) -> LlmResponse` from `send` to test it.
    - With `"stop_reason":"brand_new"` → `Unrecognized`; missing → `Unreported` (or `ToolUse` when tool blocks are present).
    - Streaming stop synthesis becomes `Stop::synthetic(ToolUse|EndTurn, "broker")`. The broker's stream protocol carries no stop reason, so it's synthetic.
  - `crates/rupu-agent/tests/it/mock_provider_outcomes.rs`:

```rust
use rupu_agent::runner::{MockProvider, ScriptedTurn};
use rupu_providers::provider::LlmProvider;
use rupu_providers::reply_error::{parse_error_body, ErrorClass, ErrorOrigin};
use rupu_providers::types::{ContentBlock, LlmRequest, RefusalDetail, RefusalSource, Stop, StopReason, Usage};
use rupu_providers::{ProviderError, ProviderId};

#[tokio::test]
async fn scripted_reply_carries_a_full_stop() {
    let mut stop = Stop::from_wire(StopReason::Refusal, "anthropic", Some("refusal"));
    stop.refusal = Some(RefusalDetail {
        category: Some("cyber".into()),
        explanation: Some("Declined for this example.".into()),
        recommended_model: None,
        source: RefusalSource::Classifier,
    });
    let mut p = MockProvider::new(vec![ScriptedTurn::Reply {
        content: vec![ContentBlock::Text { text: "partial".into() }],
        stop: stop.clone(),
        usage: Usage::default(),
    }]);
    let r = p.send(&LlmRequest::default()).await.unwrap();
    assert_eq!(r.stop, stop);
}

#[tokio::test]
async fn scripted_reply_error_is_a_structured_error() {
    let body = parse_error_body("anthropic", ErrorOrigin::Http { status: 529 },
        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#, None, None);
    let mut p = MockProvider::new(vec![ScriptedTurn::ReplyError { body }]);
    let e = p.send(&LlmRequest::default()).await.unwrap_err();
    assert_eq!(e.class(), ErrorClass::Overloaded);
    assert!(matches!(e, ProviderError::Reply(_)));
}

#[tokio::test]
async fn provider_id_is_configurable_and_defaults_to_anthropic() {
    let p = MockProvider::new(vec![]);
    assert_eq!(p.provider_id(), ProviderId::Anthropic);
    let p = MockProvider::new(vec![]).with_provider_id(ProviderId::OpenaiCodex);
    assert_eq!(p.provider_id(), ProviderId::OpenaiCodex);
}

#[test]
fn reply_scripts_deserialize_from_json() {
    // RUPU_MOCK_PROVIDER_SCRIPT scripts are JSON: the new variants must parse there too.
    let json = r#"[{"Reply":{"content":[{"type":"text","text":"hi"}],
        "stop":{"reason":"pause_turn","wire":{"provider":"anthropic","value":"pause_turn"}}}}]"#;
    let turns: Vec<ScriptedTurn> = serde_json::from_str(json).unwrap();
    assert!(matches!(&turns[0], ScriptedTurn::Reply { stop, .. } if stop.reason == StopReason::PauseTurn));
}
```

  Run: `cargo test -p rupu-providers --lib provider_id:: local:: broker_client::`
  Then: `cargo test -p rupu-agent --test it mock_provider_outcomes::`
  Expected: compile failures.

- [ ] **Step 2: Implement.**
  - **`ProviderId`:**
    - Add the variants `Local` and `Broker` after `OpenaiCompatible`. They are absent from `ALL`, and `FromStr` is unchanged, because neither is a configurable provider.
    - Add `auth_key` arms returning `"local"` / `"broker"`.
    - `env_var_name` returns `Option<&'static str>` (`None` for both new variants). Update its callers (tests only, per `grep -rn "env_var_name()" crates`).
  - **`local.rs` / `broker_client.rs`:** `provider_id()` returns the new variants. Update the router's `ProviderId::Anthropic => request.model.starts_with("claude")` match in `router.rs` so it handles the two new variants: `ProviderId::Local | ProviderId::Broker => true`. A local or broker provider serves whatever model it's given.
  - **`MockProvider`:**
    - Add a field `provider_id: ProviderId` (default `Anthropic`), plus `with_provider_id`.
    - `provider_id()` returns the field. `provider_label()` returns `"mock"`, and every synthetic stop it builds uses it.
    - Handle the new variants:
      - `Reply { content, stop, usage }` → `Ok(LlmResponse { id: "mock".into(), model: "mock-1".into(), content, stop, usage })`
      - `ReplyError { body }` → `Err(ProviderError::Reply(Box::new(body)))`
  - **`ScriptedTurn`** derives `Serialize, Deserialize`. `Stop`, `Usage` and `ApiErrorBody` all implement both, so the derive works. For `usage`, use `#[serde(default)]`.

- [ ] **Step 3: Run the tests.**

  Run: `cargo test -p rupu-providers --lib`
  Expected: PASS.

  Run: `cargo test -p rupu-agent --test it mock_provider_outcomes::`
  Expected: PASS.

- [ ] **Step 4: Format and commit.**

```bash
git add -A crates/
git commit -m "feat(providers,agent): honest provider ids for local/broker; scriptable stops and errors

ProviderId gains Local and Broker (not credentialed, so not in ALL), and the
local and broker clients stop claiming Anthropic. The broker parses its
stop_reason tolerantly. MockProvider can script any Stop and any structured
error, and its provider id is configurable for cross-provider tests.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Whole-workspace verification and docs

**Files:**
- Modify: `CLAUDE.md`. In the `rupu-cli`-adjacent crates list, add a `rupu-providers` entry if there is none. Otherwise extend the existing mention with: typed `Stop` (`stop.rs`: `StopReason` + `WireStop` + refusal/served-by, per-provider `map` tables); `ApiErrorBody`/`ErrorClass` (`reply_error.rs`) behind `ProviderError::Reply`; `overflow.rs` owns the context-overflow formats; `ContentBlock::Unknown { provider, raw }` and `Fallback`.
- Modify: `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md` §9. Add one line under the §9 heading:

  > *Superseded in part (2026-10-01): the response-outcomes spec (`2026-10-01-rupu-response-outcomes-design.md` §2) replaces "anything rupu doesn't recognise becomes a surfaced error" — unrecognized replies are parsed, rendered with their raw payload, and handled by content.*

- [ ] **Step 1: Full local verification.** CI is a release gate, so this is the merge check.

  Run:
  ```bash
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test -p rupu-providers
  cargo test -p rupu-agent
  cargo test -p rupu-runtime
  cargo test -p rupu-orchestrator
  cargo test -p rupu-cli --test it
  cargo test -p rupu-cli --test serial < /dev/null
  cargo test -p rupu-cp
  ```
  Expected: all green.

  Fix anything red in place: any failure here is a bug in this plan's changes. Look in particular for CI-1.95 clippy lints (`!x.is_some_and(..)`, match arms that contain only an `if`).

- [ ] **Step 2: Grep for leftovers.** All four must print nothing:

  ```bash
  grep -rn "ProviderError::Api\b\|ProviderError::RateLimited\|stop_reason: Some(StopReason\|ContentBlock::Unknown\b[^ {]" crates --include='*.rs'
  grep -rn "unwrap_or(serde_json::json!({}))" crates/rupu-providers/src/openai_wire.rs
  grep -rn "_ => StopReason::EndTurn" crates/rupu-providers/src
  grep -rn "&text\[\.\.4096\]" crates/rupu-providers/src
  ```

- [ ] **Step 3: Commit the docs.**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md
git commit -m "docs: response-outcomes plan 1 — provider outcome types in CLAUDE.md; model-limits §9 superseded note

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Out of scope for this plan

These are covered by later plans, so don't do them here:

- Runner behavior changes: the success rule, outcome classification, the ladder, continuation and fallback hops (Plan 2). After this plan, a `Refusal` / `Safety` / `MaxTokens` reply still ends a run `Ok` when it has no tool calls. That is the pre-existing interim behavior, and Plan 2 fixes it.
- Sending `fallbacks: "default"` and the beta header, plus the echo rule for blocks before a mid-output `fallback` block (Plan 2, spec §4.6).
- Transcript `Outcome` / `Recovery` events, `TurnEnd.stop`, and raw `Event::Unknown` (Plan 2).
- Rendering anything new (Plan 3).
