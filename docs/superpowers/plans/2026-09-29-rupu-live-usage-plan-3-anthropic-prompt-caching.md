# Live usage — Plan 3: Anthropic prompt caching + cache-token accounting — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Anthropic prompt caching on by default (per-provider and
per-agent opt-out), and account for cache reads and cache writes correctly in
tokens and cost everywhere.

**Architecture:**
- **One cross-provider token meaning:** `input_tokens` is the whole prompt;
  `cached_tokens` (cache reads) and a new `cache_write_tokens` are subsets of
  it. The Anthropic wire boundary folds its separate counts into that meaning.
- **Pricing** gains a cache-write rate.
- **Request body:** the Anthropic builder adds two explicit `cache_control`
  breakpoints — the last system block (which caches tools + system) and the last
  block of the final message (the rolling history).

**Tech Stack:** Rust (`rupu-providers`, `rupu-config`, `rupu-transcript`,
`rupu-agent`, `rupu-orchestrator`, `rupu-cp`, `rupu-cli`, `rupu-runtime`).

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-live-usage-ledger-design.md` §9.
**Depends on Plan 1** (ledger row, fold, `TurnPoint`); stack on it.

## Global Constraints

- **Semantics:** `Usage.input_tokens` = total prompt tokens, for every provider.
  `cached_tokens` ⊆ input (cache reads). `cache_write_tokens` ⊆ input (cache
  writes). OpenAI-family providers already report `cached ⊆ input` and have no
  cache writes (`0`).
- **Anthropic wire → normalized:**
  - `input = input_tokens + cache_read_input_tokens + cache_creation_input_tokens`;
  - `cached = cache_read_input_tokens`;
  - `cache_write = cache_creation_input_tokens`.
- **Cost:**
  `(input − cached − cache_write)·in + cached·read + cache_write·write + output·out`, per 1e6.
  `read` defaults to the input rate when unset; `write` defaults to the input
  rate when unset.
- **Built-in Anthropic prices:** `write = 1.25 × input` (5-minute TTL).
  - Add `claude-opus-5-5`: in 4.00 / out 20.00 / read 0.20 / write 5.00.
  - Add `claude-sonnet-5-5`: in 2.00 / out 10.00 / read 0.20 / write 2.50.
- **Caching default: ON.** Opt out with `[providers.<name>] prompt_cache = false`
  or agent frontmatter `anthropicPromptCache: false`. The agent setting wins
  over the provider setting.
- **Breakpoints:**
  - Explicit `{"type":"ephemeral"}` only — no top-level automatic caching, and
    no 1h TTL.
  - At most 2 markers per request.
  - Never on a `thinking` / `redacted_thinking` block, never on an empty text
    block, never inside a restored raw reasoning block.
- **New fields are additive:** serde default `0`, and
  `skip_serializing_if = "is_zero"` where the struct already uses skip patterns.
  Regenerate the macOS fixtures if any fixture type changes.
- Do not run package-wide `cargo fmt`. Do not use bare `git stash`. Clippy must
  be clean with `-D warnings`.
- **Live API calls spend money.** The live verification test is `#[ignore]` and
  is run only after matt approves it in chat.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-providers/src/types.rs` | `Usage.cache_write_tokens` |
| `crates/rupu-providers/src/anthropic.rs` | `AnthropicWireUsage` + normalization (stream + non-stream); `with_prompt_cache`; `apply_cache_breakpoints` |
| `crates/rupu-providers/src/{broker_types,broker_client}.rs` | pass-through of the new field (serde) |
| `crates/rupu-providers/src/tuning.rs` | `ProviderTuning.prompt_cache: Option<bool>` |
| `crates/rupu-config/src/provider_config.rs` | `ProviderConfig.prompt_cache: Option<bool>` |
| `crates/rupu-config/src/pricing_config.rs` | `cache_write_per_mtok`; 4-arg `cost_usd` |
| `crates/rupu-config/src/pricing.rs` | built-in write rates; Opus 5.5 / Sonnet 5.5; comment fix |
| `crates/rupu-runtime/src/provider_factory.rs` | resolve + thread the caching flag |
| `crates/rupu-agent/src/spec.rs` | `anthropicPromptCache` frontmatter |
| `crates/rupu-transcript/src/{event,aggregate}.rs` | `Usage.cache_write_tokens`; `UsageRow.cache_write_tokens` |
| `crates/rupu-agent/src/runner.rs` | writes / hooks the new field |
| `crates/rupu-orchestrator/src/usage_ledger.rs` | `LedgerRow.cache_write_tokens` |
| `crates/rupu-cp/src/{usage,usage_index}.rs` + web `lib/usage.ts` | fold + summary + `TurnPoint` carry cache writes |

---

### Task 1: Normalized `Usage` with cache writes; Anthropic wire parsing

**Files:**
- Modify: `crates/rupu-providers/src/types.rs:119-138`
- Modify: `crates/rupu-providers/src/anthropic.rs`:
  - `message_start` usage (~1938-1958);
  - `message_delta` usage (~2120-2135);
  - `StreamAccumulator` (~2155-2210);
  - `AnthropicResponse` (~2276-2295).
- Test: `anthropic.rs` inline tests (next to `decode_response_populates_cached_tokens` ~3695)

**Interfaces:**
- Produces:
  - `Usage.cache_write_tokens: u32` (`#[serde(default)]`).
  - `struct AnthropicWireUsage { input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens }`
    (all `u32`, `#[serde(default)]`) with
    `fn normalize(&self) -> Usage`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn decode_response_normalizes_cache_reads_and_writes_into_input() {
        let body = r#"{
            "id": "msg_x", "model": "claude-opus-5-5",
            "content": [{"type":"text","text":"hi"}], "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "cache_read_input_tokens": 200, "cache_creation_input_tokens": 30}
        }"#;
        let parsed: AnthropicResponse = serde_json::from_str(body).unwrap();
        let resp = parsed.into_llm_response();
        assert_eq!(resp.usage.input_tokens, 240);
        assert_eq!(resp.usage.cached_tokens, 200);
        assert_eq!(resp.usage.cache_write_tokens, 30);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn stream_usage_normalizes_and_message_delta_cache_fields_win_when_present() {
        // message_start carries input 10 / read 200 / write 30; message_delta carries
        // output 7 and (newer API) repeats cache fields with updated values read 200 / write 30.
        // Drive the SSE parser the same way the existing streaming tests do (~3442-3483).
        // Expect final usage: input 240, cached 200, cache_write 30, output 7.
    }
```

Also update `decode_response_populates_cached_tokens` (~3695). Its input of 10
plus a read of 200 now normalizes to `input_tokens == 210`. Change that one
assertion and add a comment that explains the normalization.

Write the stream test body by copying the event-construction style of the
existing streaming usage test (~3442-3483).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-providers anthropic::tests::decode_response anthropic::tests::stream_usage`
Expected: FAIL (no field, no normalization).

- [ ] **Step 3: Implement**
  1. `types.rs` — add, after `cached_tokens`:

```rust
    /// Prompt tokens written to the provider's prompt cache on this call — a
    /// SUBSET of `input_tokens`, like `cached_tokens` (cache reads). Only
    /// Anthropic reports it (`cache_creation_input_tokens`); billed at the
    /// cache-write rate (1.25x input for the 5-minute TTL).
    #[serde(default)]
    pub cache_write_tokens: u32,
```

     Fix every `Usage { … }` literal the compiler flags: add
     `cache_write_tokens: 0`, or use `..Default::default()` where the literal
     style allows it.
  2. `anthropic.rs`:

```rust
/// Anthropic's wire usage. Its `input_tokens` EXCLUDES cache reads and cache
/// writes; `normalize` folds both back in so every provider's
/// `Usage.input_tokens` means "the whole prompt".
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct AnthropicWireUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

impl AnthropicWireUsage {
    fn normalize(&self) -> Usage {
        Usage {
            input_tokens: self
                .input_tokens
                .saturating_add(self.cache_read_input_tokens)
                .saturating_add(self.cache_creation_input_tokens),
            output_tokens: self.output_tokens,
            cached_tokens: self.cache_read_input_tokens,
            cache_write_tokens: self.cache_creation_input_tokens,
            reasoning_tokens: 0,
        }
    }
}
```

     - `AnthropicResponse.usage` becomes `AnthropicWireUsage`, and
       `into_llm_response` uses `self.usage.normalize()`.
     - `StreamAccumulator` stores `wire: AnthropicWireUsage` instead of separate
       input/cached counters.
     - `message_start`: deserialize `msg["usage"]` into `AnthropicWireUsage`
       (`serde_json::from_value(...).unwrap_or_default()`), store it, and emit
       `UsageSnapshot(acc.wire.normalize())`.
     - `message_delta`: set `acc.wire.output_tokens` from `usage.output_tokens`.
       For each of `input_tokens`, `cache_read_input_tokens` and
       `cache_creation_input_tokens`, when present in the delta
       (`usage.get(k).and_then(as_u64)`), overwrite the stored value. Then emit
       `UsageSnapshot(acc.wire.normalize())`.
     - `into_response` builds `usage: self.wire.normalize()`.
     - Leave the `alias = "cache_read_input_tokens"` on `Usage.cached_tokens`
       in place (harmless). Add a comment that Anthropic parsing now goes
       through `AnthropicWireUsage`.
  3. Check `broker_types.rs` / `broker_client.rs`. If they build `Usage` from
     their own JSON, map `cache_write_tokens` through. If they serde `Usage`
     directly, nothing is needed beyond the default.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-providers && cargo build --workspace --all-targets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "fix(providers): normalize Anthropic usage — input includes cache reads+writes; record cache_write_tokens"
```

---

### Task 2: Carry `cache_write_tokens` through transcript, ledger, fold, summary, web

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs` — `Event::Usage`
  `cache_write_tokens: u32` (`#[serde(default, skip_serializing_if = "is_zero_u32")]`;
  add a private `fn is_zero_u32(v: &u32) -> bool { *v == 0 }`).
- Modify: `crates/rupu-transcript/src/aggregate.rs` —
  `UsageRow.cache_write_tokens: u64`, summed.
- Modify: `crates/rupu-agent/src/runner.rs` — the per-turn and compaction
  `Usage` writes and `UsageTurn` carry it (`UsageTurn.cache_write_tokens: u64`).
- Modify: `crates/rupu-orchestrator/src/usage_ledger.rs` —
  `LedgerRow.cache_write_tokens: u64` (`#[serde(default)]`); `hook` copies it.
- Modify: `crates/rupu-cp/src/usage_index.rs` (`Tokens.cache_write`, both
  folds) and `crates/rupu-cp/src/usage.rs`:
  - `UsageSummary.cache_write_tokens` (`#[serde(default)]`);
  - `TurnPoint.tokens_cache_write` (`#[serde(default)]`);
  - `summarize` sums it and prices via the 4-arg `cost_usd` (Task 3 — do Task 3
    first if you prefer; otherwise pass `0` and let Task 3 switch the call).
- Modify: web `lib/usage.ts` (`cache_write_tokens?: number`), `lib/api.ts`
  (`UsageTimelinePoint.tokens_cache_write?: number`), and the RunDetail header
  (show `cache write N` next to `cached` when > 0).
- Test: `crates/rupu-transcript/tests/usage_event.rs`, `usage_index.rs`,
  `usage_ledger.rs`.

- [ ] **Step 1: Write the failing tests**
  - Transcript: `Usage` with `cache_write_tokens: 30` round-trips. An old line
    without it parses as `0` and does not serialize it when `0`.
  - Ledger: `hook` with `UsageTurn { cache_write_tokens: 30, .. }` writes a row
    with `cache_write_tokens == 30`.
  - Fold: two ledger rows with `cache_write_tokens` 30 and 5 →
    `RunUsage.rows[0].cache_write_tokens == 35`. A fallback transcript
    `Usage { cache_write_tokens: 7 }` → 7.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-transcript -p rupu-orchestrator -p rupu-cp cache_write`
Expected: compile errors.

- [ ] **Step 3: Implement.** Add the field in each place listed and let the
  compiler find the literals (`cache_write_tokens: 0`, or the real value).

  In the agent runner, write
  `cache_write_tokens: resp.usage.cache_write_tokens` into the transcript
  `Usage` and set `UsageTurn.cache_write_tokens` to
  `resp.usage.cache_write_tokens as u64`. Do the same for the compaction
  usage.

  Web: display only; no logic change.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-cp -p rupu-cli && (cd crates/rupu-cp/web && npx vitest run)`
Expected: PASS. If fixture types changed, run `make macos-fixtures` and commit
the result.

- [ ] **Step 5: Commit**

```bash
git add -A crates apps/rupu-macos/Fixtures
git commit -m "feat(usage): cache_write_tokens through transcript, ledger, fold, summary, UI"
```

---

### Task 3: Pricing — cache-write rate, 4-arg `cost_usd`, built-ins

**Files:**
- Modify: `crates/rupu-config/src/pricing_config.rs:60-110` (+ tests ~113-190)
- Modify: `crates/rupu-config/src/pricing.rs` (Anthropic section ~36-215)
- Modify: every `.cost_usd(` caller (20 hits, listed by
  `grep -rn '\.cost_usd(' --include='*.rs' crates`; ignore the `CostAcc::cost_usd()`
  zero-arg accessors). Pass the row's or fact's `cache_write_tokens` where one
  is available, otherwise `0`:
  - `rupu-cp/src/usage.rs:56, :455`;
  - `rupu-cp/src/api/usage.rs:915`;
  - `rupu-cp/src/api/sessions.rs:153` (now a fold per Plan 1; pass the fold's
    value);
  - `rupu-cli/src/cmd/workflow.rs:2472, :2629`;
  - `rupu-cli/src/cmd/usage.rs:1032`;
  - `rupu-cli/src/cmd/autoflow.rs:7287`;
  - `rupu-cli/src/cmd/session.rs:6416`;
  - `rupu-cli/src/output/live_run.rs` (if still present after Plan 1);
  - `rupu-agent/src/runner.rs:2290`.
- Test: `pricing_config.rs` and `pricing.rs` inline tests

**Interfaces:**
- Produces:
  - `ModelPricing.cache_write_per_mtok: Option<f64>`
    (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
  - `pub fn cost_usd(&self, input_tokens: u64, output_tokens: u64, cached_tokens: u64, cache_write_tokens: u64) -> f64`.

- [ ] **Step 1: Write the failing tests** (`pricing_config.rs`):

```rust
    #[test]
    fn cost_bills_reads_writes_and_uncached_separately() {
        let p = ModelPricing {
            input_per_mtok: 4.0, output_per_mtok: 20.0,
            cached_input_per_mtok: Some(0.20), cache_write_per_mtok: Some(5.0),
        };
        // 1M prompt = 700k read + 200k write + 100k uncached; 10k output.
        let c = p.cost_usd(1_000_000, 10_000, 700_000, 200_000);
        let want = 0.1 * 4.0 + 0.7 * 0.20 + 0.2 * 5.0 + 0.01 * 20.0;
        assert!((c - want).abs() < 1e-9, "{c} vs {want}");
    }

    #[test]
    fn write_rate_defaults_to_input_rate() {
        let p = ModelPricing { input_per_mtok: 3.0, output_per_mtok: 15.0,
            cached_input_per_mtok: None, cache_write_per_mtok: None };
        assert!((p.cost_usd(1_000_000, 0, 0, 1_000_000) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn reads_plus_writes_exceeding_input_are_clamped() {
        let p = ModelPricing { input_per_mtok: 1.0, output_per_mtok: 0.0,
            cached_input_per_mtok: Some(0.1), cache_write_per_mtok: Some(1.25) };
        // Malformed report: read+write > input. Never negative uncached.
        let c = p.cost_usd(100, 0, 80, 80);
        assert!(c >= 0.0);
    }
```

In `pricing.rs` tests:
- `lookup(&cfg, "anthropic", "claude-opus-5-5", "any")` returns input 4.0 /
  output 20.0 / read 0.20 / write 5.0.
- Every built-in `anthropic` entry has
  `cache_write_per_mtok == Some(1.25 * input_per_mtok)`, within 1e-9.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-config pricing`
Expected: compile errors.

- [ ] **Step 3: Implement**

```rust
    pub fn cost_usd(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        cache_write_tokens: u64,
    ) -> f64 {
        let cached = cached_tokens.min(input_tokens);
        let write = cache_write_tokens.min(input_tokens - cached);
        let uncached = input_tokens - cached - write;
        let read_rate = self.cached_input_per_mtok.unwrap_or(self.input_per_mtok);
        let write_rate = self.cache_write_per_mtok.unwrap_or(self.input_per_mtok);
        (uncached as f64 * self.input_per_mtok
            + cached as f64 * read_rate
            + write as f64 * write_rate
            + output_tokens as f64 * self.output_per_mtok)
            / 1_000_000.0
    }
```

- Rewrite the doc comment to state the three input subsets and that Anthropic
  inputs are normalized upstream (Plan 3 Task 1). Keep the I-48 reasoning
  paragraph.
- `pricing.rs`:
  - Add `cache_write_per_mtok: Some(<1.25 × input>)` to every Anthropic entry.
  - Add the two new model entries (values in Global Constraints) at the top of
    the Anthropic section.
  - Add `cache_write_per_mtok: None` to every non-Anthropic entry.
  - Replace the misleading comment ("transcripts fold cache-creation tokens
    into `input_tokens`, so writes are billed here at 1x…") with:
    "Cache writes bill at 1.25x input (5-minute TTL — rupu's only TTL);
    `input_tokens` is normalized to include reads and writes (see
    `rupu_providers::anthropic::AnthropicWireUsage`)."
  - Bump "Last reviewed" to 2026-09-29.
- Update all callers (see the list under **Files**).

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-config && cargo build --workspace --all-targets && cargo test -p rupu-cp -p rupu-cli`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates
git commit -m "feat(pricing): cache-write rate; 4-arg cost_usd; Opus 5.5 / Sonnet 5.5 built-ins"
```

---

### Task 4: Enable caching — breakpoints + config/frontmatter opt-out

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs`:
  - the client struct (~760-800, next to `oauth_system_prefix_enabled`) and
    its constructors (~800-975);
  - a `with_prompt_cache` builder next to `with_oauth_system_prefix` (~862);
  - `build_request_body` (~1677-…) — call `apply_cache_breakpoints` at the end.
- Modify: `crates/rupu-providers/src/tuning.rs:39-70` — `prompt_cache: Option<bool>`
  (default `None`); fix the `ProviderTuning { … }` literals (21 hits).
- Modify: `crates/rupu-config/src/provider_config.rs:7-32` — `prompt_cache: Option<bool>`.
- Modify: `crates/rupu-runtime/src/provider_factory.rs`:
  - `ProviderConfig` (~24-40): `anthropic_prompt_cache: Option<bool>`;
  - `provider_tuning` (~108-131): `prompt_cache: p.and_then(|p| p.prompt_cache)`;
  - `build_anthropic` (~540-552).
- Modify: `crates/rupu-agent/src/spec.rs` (~49-51, ~151, ~210): frontmatter
  `anthropicPromptCache` → `AgentSpec.anthropic_prompt_cache`.
- Modify: every site that sets `anthropic_oauth_system_prefix:` (grep; e.g.
  `rupu-cli/src/cmd/dispatch.rs:274`, `cp_definition_generator.rs:67`, and the
  step factory / run / session builders) — set `anthropic_prompt_cache` beside
  it.
- Test: `anthropic.rs` inline request-shape tests (next to the
  `build_body_*` tests ~3714); `provider_factory.rs` test for resolution;
  `spec.rs` frontmatter test.

**Interfaces:**
- Produces:
  - `AnthropicClient::with_prompt_cache(self, enabled: bool) -> Self`; the
    field defaults to `true` in every constructor.
  - `fn apply_cache_breakpoints(body: &mut serde_json::Value)` (private).

- [ ] **Step 1: Write the failing tests**

```rust
    fn body_for(client: &AnthropicClient, req: &LlmRequest) -> serde_json::Value {
        client.build_request_body(req, true)
    }

    fn cache_markers(v: &serde_json::Value) -> usize {
        let mut n = 0;
        fn walk(v: &serde_json::Value, n: &mut usize) {
            match v {
                serde_json::Value::Object(m) => {
                    if m.contains_key("cache_control") { *n += 1; }
                    m.values().for_each(|x| walk(x, n));
                }
                serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, n)),
                _ => {}
            }
        }
        walk(v, &mut n);
        n
    }

    #[test]
    fn caching_marks_last_system_block_and_last_message_block() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink));
        let req = LlmRequest {
            model: "claude-opus-5-5".into(),
            system: Some("You are a reviewer.".into()),
            messages: vec![Message::user("first"), /* assistant + user turns as the other tests build them */],
            tools: vec![/* one tool, built like the existing tools tests */],
            ..Default::default()
        };
        let b = body_for(&client, &req);
        let sys = b["system"].as_array().unwrap();
        assert_eq!(sys.last().unwrap()["cache_control"]["type"], "ephemeral");
        let last_msg = b["messages"].as_array().unwrap().last().unwrap();
        let blocks = last_msg["content"].as_array().expect("string content converted to blocks");
        assert_eq!(blocks.last().unwrap()["cache_control"]["type"], "ephemeral");
        assert_eq!(cache_markers(&b), 2);
    }

    #[test]
    fn caching_disabled_emits_no_markers() {
        let client = AnthropicClient::new("k".into(), Arc::new(rupu_netflow::NullSink)).with_prompt_cache(false);
        let req = LlmRequest { model: "m".into(), system: Some("s".into()),
            messages: vec![Message::user("hi")], ..Default::default() };
        assert_eq!(cache_markers(&body_for(&client, &req)), 0);
    }

    #[test]
    fn no_system_marks_last_tool_instead() { /* system None, 2 tools → tools[1] marked; total 2 */ }

    #[test]
    fn never_marks_thinking_or_empty_text_blocks() {
        /* last message content = [text "x", thinking {...}] (assistant) or [text ""] →
           marker lands on the last NON-thinking, NON-empty block; if none qualifies, no
           message marker (total 1) */
    }

    #[test]
    fn oauth_billing_blocks_stay_first_and_unmarked_except_last_system_block() {
        /* oauth_client() (existing helper ~2820): system = [billing, self-desc, agent];
           only system[2] has cache_control; system[0] text is byte-identical to
           ANTHROPIC_BILLING_HEADER_BLOCK */
    }

    #[test]
    fn restored_reasoning_raw_block_is_byte_identical() {
        /* message history containing a restored reasoning block (see the existing
           restore_reasoning_blocks tests) → after build_request_body its JSON equals the
           no-cache build's JSON for that block */
    }
```

In `provider_factory.rs` tests:
- `[providers.anthropic] prompt_cache = false` → `provider_tuning(...).prompt_cache == Some(false)`.
- `spec.rs`: frontmatter `anthropicPromptCache: false` parses to `Some(false)`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-providers caching && cargo test -p rupu-runtime prompt_cache && cargo test -p rupu-agent anthropic_prompt_cache`
Expected: FAIL / compile errors.

- [ ] **Step 3: Implement**

```rust
/// Explicit prompt-cache breakpoints (spec 2026-09-29 §9.3). Two markers:
/// (a) the last `system` block — caches tools + system (tools render first);
///     with no system, the last tool definition instead;
/// (b) the last cacheable block of the final message — the rolling
///     conversation breakpoint, so each turn re-reads all prior history.
/// Never marks thinking / redacted_thinking / empty-text blocks. Runs LAST in
/// `build_request_body`, after tool-name sanitizing and reasoning restoration,
/// and only touches the marked blocks' `cache_control` key.
fn apply_cache_breakpoints(body: &mut serde_json::Value) {
    let eph = || serde_json::json!({ "type": "ephemeral" });
    let mut marked_prefix = false;
    if let Some(sys) = body.get_mut("system").and_then(|s| s.as_array_mut()) {
        if let Some(last) = sys.last_mut() {
            last["cache_control"] = eph();
            marked_prefix = true;
        }
    }
    if !marked_prefix {
        if let Some(last) = body.get_mut("tools").and_then(|t| t.as_array_mut()).and_then(|t| t.last_mut()) {
            last["cache_control"] = eph();
        }
    }
    let Some(last_msg) = body
        .get_mut("messages")
        .and_then(|m| m.as_array_mut())
        .and_then(|m| m.last_mut())
    else {
        return;
    };
    if let Some(text) = last_msg.get("content").and_then(|c| c.as_str()).map(str::to_string) {
        if text.is_empty() {
            return;
        }
        last_msg["content"] = serde_json::json!([{ "type": "text", "text": text }]);
    }
    let Some(blocks) = last_msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
        return;
    };
    let target = blocks.iter_mut().rev().find(|b| {
        let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let empty_text = ty == "text" && b.get("text").and_then(|t| t.as_str()).map_or(true, str::is_empty);
        !matches!(ty, "thinking" | "redacted_thinking") && !empty_text
    });
    if let Some(b) = target {
        b["cache_control"] = eph();
    }
}
```

- At the end of `build_request_body`, just before returning `body`:
  `if self.prompt_cache_enabled { apply_cache_breakpoints(&mut body); }`.
- Add `prompt_cache_enabled: bool` to the client struct. Set it to `true` in
  every constructor that sets `oauth_system_prefix_enabled: true`.
- Add the builder:

```rust
    /// Explicit prompt-cache breakpoints (default ON). `false` from
    /// `[providers.<name>] prompt_cache = false` or agent frontmatter
    /// `anthropicPromptCache: false` — e.g. for an Anthropic-compatible
    /// gateway that rejects `cache_control`.
    pub fn with_prompt_cache(mut self, enabled: bool) -> Self {
        self.prompt_cache_enabled = enabled;
        self
    }
```

- `provider_factory.rs::build_anthropic`: before `.with_tuning(tuning)`
  consumes `tuning`, read
  `let cache = config.anthropic_prompt_cache.or(tuning.prompt_cache).unwrap_or(true);`.
  After building the client, apply `client = client.with_prompt_cache(cache);`.
  (`tuning` here is the resolved `ProviderTuning`. Check how `build_anthropic`
  obtains it — `config.tuning` or `ProviderTuning::for_provider` — and read
  `prompt_cache` from that same value.)
- `spec.rs`: mirror `anthropic_oauth_prefix` exactly
  (`#[serde(default, rename = "anthropicPromptCache")] anthropic_prompt_cache: Option<bool>`,
  the `AgentSpec` field, and the mapping at ~210). Add docs: "Anthropic prompt
  caching opt-out (default on). `false` disables the explicit cache_control
  breakpoints for this agent."
- `docs/providers.md` (or the provider config doc that lists
  `[providers.<name>]` keys — find it with `grep -rn 'max_concurrency' docs`):
  document `prompt_cache` and `anthropicPromptCache`.

- [ ] **Step 4: Run to verify**

Run: `cargo test -p rupu-providers -p rupu-runtime -p rupu-agent -p rupu-config && cargo build --workspace --all-targets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A crates docs
git commit -m "feat(anthropic): prompt caching on by default (system + rolling message breakpoints); prompt_cache / anthropicPromptCache opt-out"
```

---

### Task 5: Live verification (with approval) + PR

- [ ] **Step 1: Add an ignored live test** to
  `crates/rupu-providers/tests/live_smoke.rs`, following its existing gating and
  credential pattern:
  - send the same ~3k-token system prompt plus a short user message twice,
    streaming;
  - assert that the second response's `usage.cached_tokens > 0` and that
    `usage.input_tokens >= usage.cached_tokens`.

  Mark it `#[ignore = "live API: spends money; run with --ignored after approval"]`.
- [ ] **Step 2: Ask matt in chat** before running it. It is ~2 small requests.
  Only after an explicit yes, run:
  `cargo test -p rupu-providers --test live_smoke -- --ignored prompt_cache`.
  Report the observed `cached_tokens` / `cache_write_tokens` for both calls.
- [ ] **Step 3: Full suites + clippy**

```bash
cargo test -p rupu-providers -p rupu-config -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-runtime -p rupu-cp -p rupu-cli 2>&1 | grep -E '^test result|FAILED|panicked'
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 4: Open the PR** (stacked on the Plan 1/2 branch if unmerged). Push
  with an explicit refspec. Body:
  - the normalization table;
  - the pricing formula;
  - the breakpoint placement;
  - the default ON with the opt-outs;
  - the live-test result (or "not run — awaiting approval");
  - `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

---

## Self-review notes (plan author)

- **Spec coverage:**
  - §9.1 → Tasks 1 and 2.
  - §9.2 → Task 3.
  - §9.3 → Task 4.
  - §9.4 → Task 4.
  - §9.5 → Tasks 4 and 5.
- **Ordering:** Task 2 refers to the 4-arg `cost_usd` from Task 3. Either order
  works: Task 2 passes `0`, and Task 3 switches the callers.
