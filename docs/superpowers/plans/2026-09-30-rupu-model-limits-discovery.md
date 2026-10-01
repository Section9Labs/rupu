# rupu model-limits discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every run discovers its model's real input limit and output cap from the provider's model-list endpoint (1h cache). It uses them for `max_tokens` and the compaction threshold unless the agent pins values, learns the real limit from overflow errors, and exposes the catalog plus a manual refetch in `rupu models` and the CP Settings → Models tab.

**Architecture:**
- **Providers** (`rupu-providers`) gain `LlmProvider::fetch_models(&mut self)`, which returns limits or a real error, and `output_shares_context()`. Each provider maps its native fields onto "usable input tokens" and "max output tokens".
- **Registry.** `ModelRegistry`'s cache moves to v2 so limits survive across processes.
- **Resolver.** A new `rupu_runtime::model_limits` module owns `resolve` (agent → config → live → unknown, with provenance), `refresh` and `catalog`. The refresh logic that lives in the CLI today moves there.
- **Runner** (`rupu-agent`). Its three loose limit fields are replaced by one `limits: ModelLimits`, used for the request `max_tokens`, the compaction threshold (with an output-headroom rule), the run-start notice, and learning the limit from overflow errors.
- **CP** (`rupu-cp`) exposes a `ModelCatalog` port. `rupu cp serve` implements it, and the web gets a Models tab.

**Tech Stack:** Rust 2021 (tokio, serde, chrono, reqwest-middleware, httpmock 0.7.0, thiserror, async-trait); React + TypeScript + vitest for the CP web.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md`. Read it first; section numbers below (§N) refer to it.

## Global Constraints

- Hexagonal: `rupu-cli` is arg parsing plus delegation, with no business logic (CLAUDE.md rule 2). Model-list refresh and resolution logic lives in `rupu-runtime`.
- Workspace deps only: versions are pinned in the root `Cargo.toml`, never in a crate's `Cargo.toml` (`futures-util.workspace = true`, etc.).
- `#![deny(clippy::all)]`; `cargo clippy --workspace --all-targets -- -D warnings` must be clean. `unsafe_code` is forbidden.
- **Never run package-wide `cargo fmt`** (main is fmt-dirty under the pinned toolchain). Format only files you touched: `rustfmt --edition 2021 <file>`.
- **Never use `git stash`** (the stash is shared across worktrees and sessions). Use a WIP commit if you need to set work aside.
- Public repo: invent all fixture data from scratch. Never adapt real assessment output.
- No silent no-ops: an unknown limit is always stated in the run-start notice.
- Exact values from the spec:
  - model-list fetch timeout **10s**
  - cache TTL **1h** (existing `CACHE_TTL_SECS`)
  - `ANTHROPIC_FALLBACK_MAX_TOKENS = 8192`
  - `DEFAULT_COMPACT_AT_PERCENT = 80`, clamped to `[10, 95]`
  - Codex `effective_context_window_percent` defaults to **95** when absent
  - Anthropic listing `?limit=1000`
  - Copilot header `X-GitHub-Api-Version: 2025-10-01`
  - Codex `client_version=0.50.0` (unchanged)
  - cache schema **2**
- Before Task 1, record the baseline: `cargo test --workspace 2>&1 | tail -40`. The Homebrew toolchain in worktrees can differ from the pinned one, so note any failures that already exist and don't "fix" them here.
- Every commit message ends with:
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-providers/src/model_limits.rs` (new) | `ModelLimits` / `Limit` / `LimitSource`, threshold math, notice text, number formatting |
| `crates/rupu-providers/src/provider.rs` | trait methods `fetch_models`, `output_shares_context` |
| `crates/rupu-providers/src/tuned.rs` | decorators forward the two new methods |
| `crates/rupu-providers/src/types.rs` | `LlmRequest.max_tokens: Option<u32>` |
| `crates/rupu-providers/src/{anthropic,openai_codex,github_copilot,google_gemini,openai_compatible,openai_wire,local,broker_types}.rs` | wire handling of `Option` max_tokens, plus per-provider `fetch_models` |
| `crates/rupu-providers/src/model_registry.rs` | v2 cache (limits, schema, atomic write), `fetched_at`, `find_live`, `match_model_id`, `strip_1m` |
| `crates/rupu-runtime/src/model_limits.rs` (new) | `LimitsContext`, `LimitOverrides`, `resolve`, `refresh`, `catalog`, provider-name targeting moved from the CLI |
| `crates/rupu-runtime/src/provider_factory.rs` | `provider_config_for`; drop the made-up OpenAI-compatible defaults |
| `crates/rupu-agent/src/runner.rs` | `AgentRunOpts.limits`, `RunResult.final_limits`, notice, threshold, `parse_context_overflow`, clamp + compaction |
| `crates/rupu-cli/src/cmd/{run,dispatch,session,workflow}.rs`, `crates/rupu-cli/src/resume.rs`, `crates/rupu-orchestrator/src/step_factory.rs` | resolve limits at every launch site |
| `crates/rupu-cli/src/cmd/models.rs` | thin wrapper over `rupu_runtime::model_limits` |
| `crates/rupu-cp/src/model_catalog.rs` (new), `crates/rupu-cp/src/api/models.rs` (new) | port + `GET /api/models` + `POST /api/models/refresh` |
| `crates/rupu-cli/src/cp_model_catalog.rs` (new) | `cp serve` adapter for the port |
| `crates/rupu-cp/web/src/components/settings/ModelsTab.tsx` (new) | Settings → Models tab |
| `docs/agent-format.md`, `docs/providers.md`, `CLAUDE.md` | docs |

---

### Task 1: Limit types and the two new provider trait methods

**Files:**
- Create: `crates/rupu-providers/src/model_limits.rs`
- Modify: `crates/rupu-providers/src/lib.rs` (add `pub mod model_limits;` next to the other `pub mod` lines 12-42)
- Modify: `crates/rupu-providers/src/provider.rs:32-76` (the `LlmProvider` trait)
- Modify: `crates/rupu-providers/src/tuned.rs` (the `impl LlmProvider for ThrottledProvider` around line 88 and `impl LlmProvider for RetryingProvider` around line 243)
- Test: in-file `#[cfg(test)] mod tests` in `model_limits.rs` and `tuned.rs`

**Interfaces:**
- Produces:
  - `rupu_providers::model_limits::{ModelLimits, Limit, LimitSource, ANTHROPIC_FALLBACK_MAX_TOKENS, DEFAULT_COMPACT_AT_PERCENT, group_thousands, fmt_age}`
  - `ModelLimits::{unknown(), fixed(u32,u32), from_pins(Option<u32>,Option<u32>,Option<u8>), with_input(u32), with_output(u32), with_percent(u8), compact_threshold() -> Option<u64>, clamp_input(u32) -> bool, describe(&str, DateTime<Utc>) -> String}`
  - `LlmProvider::fetch_models(&mut self) -> Result<Vec<ModelInfo>, ProviderError>`
  - `LlmProvider::output_shares_context(&self) -> bool`

- [ ] **Step 1: Write the failing tests** at the bottom of the new `crates/rupu-providers/src/model_limits.rs` (the module body comes in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, h, m, 0).unwrap()
    }

    #[test]
    fn threshold_is_off_when_input_unknown() {
        assert_eq!(ModelLimits::unknown().compact_threshold(), None);
    }

    #[test]
    fn threshold_uses_percent_when_output_unknown() {
        let l = ModelLimits::unknown().with_input(1_000_000);
        assert_eq!(l.compact_threshold(), Some(800_000));
    }

    #[test]
    fn headroom_rule_wins_for_haiku_sized_models() {
        // 200K input / 64K output: 80% = 160K, headroom = 136K (spec §6.4)
        let l = ModelLimits::fixed(200_000, 64_000);
        assert_eq!(l.compact_threshold(), Some(136_000));
    }

    #[test]
    fn percent_wins_for_1m_models() {
        let l = ModelLimits::fixed(1_000_000, 128_000);
        assert_eq!(l.compact_threshold(), Some(800_000));
    }

    #[test]
    fn headroom_rule_is_skipped_when_output_does_not_share_the_window() {
        let mut l = ModelLimits::fixed(128_000, 128_000);
        l.output_shares_context = false;
        assert_eq!(l.compact_threshold(), Some(102_400));
    }

    #[test]
    fn headroom_is_ignored_when_output_is_not_smaller_than_input() {
        let l = ModelLimits::fixed(8_000, 8_000);
        assert_eq!(l.compact_threshold(), Some(6_400));
    }

    #[test]
    fn percent_is_clamped() {
        let l = ModelLimits::unknown().with_input(1000).with_percent(99);
        assert_eq!(l.compact_at_percent, 95);
        assert_eq!(l.compact_threshold(), Some(950));
    }

    #[test]
    fn clamp_input_lowers_only() {
        let mut l = ModelLimits::unknown().with_input(1_000_000);
        assert!(l.clamp_input(200_000));
        assert_eq!(l.input, Limit::new(200_000, LimitSource::Observed));
        assert!(!l.clamp_input(500_000), "never raises");
        let mut u = ModelLimits::unknown();
        assert!(u.clamp_input(128_000), "unknown input accepts the observed value");
    }

    #[test]
    fn from_pins_marks_agent_source() {
        let l = ModelLimits::from_pins(Some(1000), None, Some(50));
        assert_eq!(l.input, Limit::new(1000, LimitSource::Agent));
        assert_eq!(l.output, Limit::unknown());
        assert_eq!(l.compact_at_percent, 50);
    }

    #[test]
    fn describe_known_live_limits() {
        let mut l = ModelLimits::unknown();
        let fetched = at(12, 0);
        l.input = Limit::new(1_000_000, LimitSource::Live { fetched_at: fetched, stale: false });
        l.output = Limit::new(128_000, LimitSource::Live { fetched_at: fetched, stale: false });
        assert_eq!(
            l.describe("anthropic", at(12, 12)),
            "input 1,000,000 · output 128,000 · compact at 800,000 (80%) — anthropic model list, cached 12m ago"
        );
    }

    #[test]
    fn describe_unknown_with_fallback_and_note() {
        let mut l = ModelLimits::unknown();
        l.output_fallback = Some(ANTHROPIC_FALLBACK_MAX_TOKENS);
        l.note = Some("gemini exposes no model limits".into());
        assert_eq!(
            l.describe("gemini", at(12, 0)),
            "input unknown · output unknown (8192 fallback) · compaction off — no limit source; gemini exposes no model limits"
        );
    }

    #[test]
    fn describe_marks_headroom_and_stale() {
        let fetched = at(9, 0);
        let mut l = ModelLimits::unknown();
        l.input = Limit::new(200_000, LimitSource::Live { fetched_at: fetched, stale: true });
        l.output = Limit::new(64_000, LimitSource::Agent);
        assert_eq!(
            l.describe("anthropic", at(12, 0)),
            "input 200,000 · output 64,000 · compact at 136,000 (output headroom) — anthropic model list, cached 3h ago (stale) + agent frontmatter"
        );
    }

    #[test]
    fn group_thousands_formats() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_048_576), "1,048,576");
    }

    #[test]
    fn serde_round_trips() {
        let l = ModelLimits::fixed(10, 5).with_percent(60);
        let s = serde_json::to_string(&l).unwrap();
        let back: ModelLimits = serde_json::from_str(&s).unwrap();
        assert_eq!(back, l);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers model_limits`
Expected: compile error (`ModelLimits` not defined).

- [ ] **Step 3: Write the module** at the top of `crates/rupu-providers/src/model_limits.rs`:

```rust
//! Resolved per-run model limits (spec
//! `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md` §5).
//! Types and pure math only; resolution lives in `rupu_runtime::model_limits`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Anthropic requires `max_tokens`. When the output cap is neither pinned nor
/// discovered, this is what goes on the wire (spec §6.3).
pub const ANTHROPIC_FALLBACK_MAX_TOKENS: u32 = 8192;

/// Compaction percentage when the agent doesn't set `compactAtPercent`.
pub const DEFAULT_COMPACT_AT_PERCENT: u8 = 80;

/// Where a resolved limit came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LimitSource {
    /// Agent frontmatter (`contextWindowTokens` / `maxTokens`).
    Agent,
    /// `[[providers.<name>.models]]` in config.
    Config,
    /// The provider's model-list endpoint, via the 1h cache.
    Live { fetched_at: DateTime<Utc>, stale: bool },
    /// Learned from a provider overflow error during this run (spec §7).
    Observed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    pub tokens: Option<u32>,
    pub source: LimitSource,
}

impl Limit {
    pub fn unknown() -> Self {
        Self { tokens: None, source: LimitSource::Unknown }
    }
    pub fn new(tokens: u32, source: LimitSource) -> Self {
        Self { tokens: Some(tokens), source }
    }
}

/// The limits one run uses. `input` is the number of input tokens the model
/// accepts (spec §3 semantics); `output` is the max output tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLimits {
    pub input: Limit,
    pub output: Limit,
    /// Agent value, else [`DEFAULT_COMPACT_AT_PERCENT`]; clamped to `[10, 95]`.
    pub compact_at_percent: u8,
    /// Whether output counts against the same window as input
    /// (`LlmProvider::output_shares_context`). Drives the headroom rule.
    pub output_shares_context: bool,
    /// What the provider sends when `output` is unknown (Anthropic:
    /// `Some(8192)`; providers where the cap is optional: `None`, meaning
    /// omitted). Display only; the provider applies it.
    #[serde(default)]
    pub output_fallback: Option<u32>,
    /// Why a limit is unknown or stale, appended to the notice.
    #[serde(default)]
    pub note: Option<String>,
}

impl ModelLimits {
    pub fn unknown() -> Self {
        Self {
            input: Limit::unknown(),
            output: Limit::unknown(),
            compact_at_percent: DEFAULT_COMPACT_AT_PERCENT,
            output_shares_context: true,
            output_fallback: None,
            note: None,
        }
    }

    /// Both limits pinned (source `Agent`). Tests and fixed-budget callers.
    pub fn fixed(input: u32, output: u32) -> Self {
        Self::unknown().with_input(input).with_output(output)
    }

    /// Agent-frontmatter pins only, with no discovery. Every unset pin stays unknown.
    pub fn from_pins(input: Option<u32>, output: Option<u32>, pct: Option<u8>) -> Self {
        let mut l = Self::unknown();
        if let Some(n) = input {
            l = l.with_input(n);
        }
        if let Some(n) = output {
            l = l.with_output(n);
        }
        if let Some(p) = pct {
            l = l.with_percent(p);
        }
        l
    }

    pub fn with_input(mut self, n: u32) -> Self {
        self.input = Limit::new(n, LimitSource::Agent);
        self
    }
    pub fn with_output(mut self, n: u32) -> Self {
        self.output = Limit::new(n, LimitSource::Agent);
        self
    }
    pub fn with_percent(mut self, pct: u8) -> Self {
        self.compact_at_percent = pct.clamp(10, 95);
        self
    }

    fn by_percent(&self) -> Option<u64> {
        let input = self.input.tokens? as u64;
        Some(input * self.compact_at_percent.clamp(10, 95) as u64 / 100)
    }

    /// Input-token count above which proactive compaction runs (spec §6.4):
    /// `min(input × pct, input − output)` when output shares the window,
    /// else `input × pct`; `None` when the input limit is unknown.
    pub fn compact_threshold(&self) -> Option<u64> {
        let by_pct = self.by_percent()?;
        let input = self.input.tokens? as u64;
        let headroom = match (self.output.tokens, self.output_shares_context) {
            (Some(out), true) => input.checked_sub(out as u64).filter(|h| *h > 0),
            _ => None,
        };
        Some(headroom.map_or(by_pct, |h| by_pct.min(h)))
    }

    /// Lower the input limit to `max` (source `Observed`) when `max` is below
    /// the current value or the current value is unknown. Never raises it.
    /// Returns `true` when the limit changed.
    pub fn clamp_input(&mut self, max: u32) -> bool {
        match self.input.tokens {
            Some(cur) if cur <= max => false,
            _ => {
                self.input = Limit::new(max, LimitSource::Observed);
                true
            }
        }
    }

    /// The run-start `Notice { kind: "model_limits" }` text (spec §6.6).
    pub fn describe(&self, provider_name: &str, now: DateTime<Utc>) -> String {
        let input = match self.input.tokens {
            Some(n) => format!("input {}", group_thousands(n as u64)),
            None => "input unknown".to_string(),
        };
        let output = match (self.output.tokens, self.output_fallback) {
            (Some(n), _) => format!("output {}", group_thousands(n as u64)),
            (None, Some(f)) => format!("output unknown ({f} fallback)"),
            (None, None) => "output unknown (provider max)".to_string(),
        };
        let compact = match (self.compact_threshold(), self.by_percent()) {
            (Some(t), Some(p)) if t < p => format!("compact at {} (output headroom)", group_thousands(t)),
            (Some(t), _) => format!("compact at {} ({}%)", group_thousands(t), self.compact_at_percent),
            (None, _) => "compaction off".to_string(),
        };
        let mut origins: Vec<String> = Vec::new();
        for src in [&self.input.source, &self.output.source] {
            let text = match src {
                LimitSource::Agent => "agent frontmatter".to_string(),
                LimitSource::Config => format!("[providers.{provider_name}.models]"),
                LimitSource::Live { fetched_at, stale } => format!(
                    "{provider_name} model list, cached {}{}",
                    fmt_age(now - *fetched_at),
                    if *stale { " (stale)" } else { "" }
                ),
                LimitSource::Observed => "provider error".to_string(),
                LimitSource::Unknown => continue,
            };
            if !origins.contains(&text) {
                origins.push(text);
            }
        }
        let origin = if origins.is_empty() { "no limit source".to_string() } else { origins.join(" + ") };
        let mut s = format!("{input} · {output} · {compact} — {origin}");
        if let Some(note) = &self.note {
            s.push_str("; ");
            s.push_str(note);
        }
        s
    }
}

/// `1048576` → `"1,048,576"`.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `just now` / `12m ago` / `3h ago` / `2d ago`.
pub fn fmt_age(age: chrono::Duration) -> String {
    let mins = age.num_minutes();
    if mins < 1 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 60 * 24 {
        format!("{}h ago", mins / 60)
    } else {
        format!("{}d ago", mins / (60 * 24))
    }
}
```

Add `pub mod model_limits;` to `crates/rupu-providers/src/lib.rs`, keeping the existing alphabetical order of the `pub mod` lines.

- [ ] **Step 4: Add the trait methods.** In `crates/rupu-providers/src/provider.rs`, inside `pub trait LlmProvider` after `list_models`:

```rust
    /// Fetch the live model catalog with limits (spec 2026-09-30 §3).
    /// `&mut self` so OAuth providers can refresh their token first. Unlike
    /// `list_models`, a failure is an `Err`, never an empty list.
    /// `Err(ProviderError::NotImplemented)` means the provider exposes no
    /// model listing.
    async fn fetch_models(
        &mut self,
    ) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        Err(ProviderError::NotImplemented {
            provider: self.provider_id().to_string(),
        })
    }

    /// Whether generated output counts against the same window as the
    /// input (spec §3). Copilot and Gemini have independent budgets.
    fn output_shares_context(&self) -> bool {
        true
    }
```

`ProviderError` is already imported in `provider.rs` (the `send` signature uses it).

- [ ] **Step 5: Forward both methods from the decorators.** In `crates/rupu-providers/src/tuned.rs`, add to **both** `impl LlmProvider for ThrottledProvider` and `impl LlmProvider for RetryingProvider` (each wraps `inner: Box<dyn LlmProvider>`):

```rust
    async fn fetch_models(
        &mut self,
    ) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        self.inner.fetch_models().await
    }

    fn output_shares_context(&self) -> bool {
        self.inner.output_shares_context()
    }
```

Then add this test to `tuned.rs`'s test module. It reuses the file's existing test provider if one fits; otherwise define this local one:

```rust
    #[tokio::test]
    async fn decorators_forward_fetch_models_and_output_shares_context() {
        struct Independent;
        #[async_trait::async_trait]
        impl LlmProvider for Independent {
            async fn send(&mut self, _: &crate::types::LlmRequest) -> Result<crate::types::LlmResponse, ProviderError> {
                unreachable!()
            }
            async fn stream(
                &mut self,
                _: &crate::types::LlmRequest,
                _: &mut (dyn FnMut(crate::types::StreamEvent) + Send),
            ) -> Result<crate::types::LlmResponse, ProviderError> {
                unreachable!()
            }
            fn default_model(&self) -> &str { "m" }
            fn provider_id(&self) -> crate::provider_id::ProviderId { crate::provider_id::ProviderId::GithubCopilot }
            async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
                Ok(vec![])
            }
            fn output_shares_context(&self) -> bool { false }
        }
        let mut t = RetryingProvider::new(Box::new(Independent), 0, |_| std::time::Duration::ZERO);
        assert!(!t.output_shares_context());
        assert!(t.fetch_models().await.unwrap().is_empty());
    }
```

Check `RetryingProvider::new`'s real signature in `tuned.rs` (the struct has `inner`, `max_retries: u32` and `backoff: fn(u32) -> Duration`) and adjust the constructor call to match. Add the same assertion for `ThrottledProvider::new(inner, Arc::new(Semaphore::new(1)))` (signature at `tuned.rs:47`).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p rupu-providers model_limits tuned`
Expected: all new tests PASS.

- [ ] **Step 7: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/model_limits.rs crates/rupu-providers/src/provider.rs crates/rupu-providers/src/tuned.rs crates/rupu-providers/src/lib.rs
git add crates/rupu-providers/src/model_limits.rs crates/rupu-providers/src/provider.rs crates/rupu-providers/src/tuned.rs crates/rupu-providers/src/lib.rs
git commit -m "feat(providers): ModelLimits types + fetch_models/output_shares_context trait methods"
```

---

### Task 2: `LlmRequest.max_tokens` becomes `Option<u32>`

**Files:**
- Modify: `crates/rupu-providers/src/types.rs:164-224` (`LlmRequest`)
- Modify: `crates/rupu-providers/src/anthropic.rs:1835-1857`, `:1967`, `:1971` (`build_request_body` + thinking budget)
- Modify: `crates/rupu-providers/src/openai_codex.rs:522-525`
- Modify: `crates/rupu-providers/src/openai_wire.rs:142-147`
- Modify: `crates/rupu-providers/src/google_gemini.rs:381-383`
- Modify: `crates/rupu-providers/src/local.rs:97`
- Modify: `crates/rupu-providers/src/broker_types.rs:46`, `:61`
- Modify (production literals): `crates/rupu-agent/src/runner.rs:382`, `:1229`; `crates/rupu-providers/src/task_classifier.rs:203`; `crates/rupu-orchestrator/src/generate.rs:210`
- Modify (test literals): every `LlmRequest { … max_tokens: N … }` in the workspace (about 100 sites, listed by the compiler)

**Interfaces:**
- Produces: `LlmRequest.max_tokens: Option<u32>`. `None` means the value is neither pinned nor discovered. Providers where the cap is optional omit it; Anthropic sends `ANTHROPIC_FALLBACK_MAX_TOKENS`.

- [ ] **Step 1: Write the failing wire tests.**

In `anthropic.rs`'s test module, next to the existing `body["max_tokens"], 1024` test (~2818):

```rust
    #[test]
    fn unset_max_tokens_sends_the_anthropic_fallback() {
        let client = AnthropicClient::with_url("k".into(), "http://x/v1/messages".into(), Arc::new(rupu_netflow::NullSink));
        let mut req = make_request(None);
        req.max_tokens = None;
        let body = client.build_request_body(&req, false);
        assert_eq!(body["max_tokens"], crate::model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS);
        req.max_tokens = Some(64_000);
        let body = client.build_request_body(&req, false);
        assert_eq!(body["max_tokens"], 64_000);
    }
```

In `openai_wire.rs`'s test module (helper `req(messages)` at ~873):

```rust
    #[test]
    fn unset_max_tokens_is_omitted_from_chat_body() {
        let mut r = req(vec![Message::user("hi")]);
        r.max_tokens = None;
        let body = build_chat_request_body(&r, false);
        assert!(body.get("max_tokens").is_none());
        r.max_tokens = Some(512);
        let body = build_chat_request_body(&r, false);
        assert_eq!(body["max_tokens"], 512);
    }
```

In `openai_codex.rs`'s test module (helper `request_with(messages)` at ~2761, which builds a `gpt-5` request):

```rust
    #[test]
    fn unset_max_tokens_omits_max_output_tokens() {
        let client = OpenAiCodexClient::new(AuthCredentials::ApiKey { key: "k".into() }, None, Arc::new(rupu_netflow::NullSink)).unwrap();
        let mut r = request_with(vec![Message::user("hi")]);
        r.model = "o4-mini".into(); // not gated like gpt-5.x
        r.max_tokens = None;
        assert!(client.build_request_body(&r, false).get("max_output_tokens").is_none());
        r.max_tokens = Some(900);
        assert_eq!(client.build_request_body(&r, false)["max_output_tokens"], 900);
    }
```

In `google_gemini.rs`'s test module, build a request with the file's existing request helper (search `fn .*-> LlmRequest` in the test module) and assert that `body` with `max_tokens: None` has no `maxOutputTokens` under the generation config. With `Some(2048)` it must be `2048`. Look at how the existing `build_request_body` tests read the generation config (the key sits under `generationConfig`, possibly inside `request` for the Code Assist wrapper; see `google_gemini.rs:431`) and use the same path.

- [ ] **Step 2: Change the type.** In `types.rs`:

```rust
    /// Output-token cap. `None` means neither pinned nor discovered: providers
    /// where the cap is optional omit it (the model's own max applies);
    /// Anthropic sends `model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS`
    /// (spec 2026-09-30 §6.3).
    pub max_tokens: Option<u32>,
```

- [ ] **Step 3: Update the provider body builders.**

`anthropic.rs` `build_request_body` (1835). Compute the cap once and use it in the body and in both thinking-budget lines:

```rust
        let max_tokens = request
            .max_tokens
            .unwrap_or(crate::model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS);
        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": max_tokens,
```

Line 1967 becomes `ThinkingLevel::Max => max_tokens.saturating_sub(2000),`, and line 1971 becomes `let clamped = raw_budget.min(max_tokens);`.

`openai_codex.rs:522-525`:

```rust
        // max_output_tokens is not supported by all models (e.g., gpt-5.x);
        // unset means the model's own max (spec 2026-09-30 §6.3).
        if let Some(n) = request.max_tokens {
            if !request.model.starts_with("gpt-5") {
                body["max_output_tokens"] = serde_json::json!(n);
            }
        }
```

`openai_wire.rs:142-147`: remove the `"max_tokens"` key from the `json!` literal, then right after it:

```rust
    if let Some(n) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(n);
    }
```

`google_gemini.rs:381-383`:

```rust
        let mut gen_config = serde_json::json!({});
        if let Some(n) = request.max_tokens {
            gen_config["maxOutputTokens"] = serde_json::json!(n);
        }
```

`local.rs:97`: remove `"max_tokens"` from the literal and insert it conditionally, the same way as in `openai_wire`.

`broker_types.rs`: `LlmRequestWire.max_tokens` stays `u32`, because it's the broker wire contract. At line 46 use `max_tokens: r.max_tokens.unwrap_or(crate::model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS),`; at line 61 use `max_tokens: Some(w.max_tokens),`.

Production literals:
- `runner.rs:382`: `max_tokens: Some(summary_max_tokens),`
- `runner.rs:1229`: `max_tokens: Some(opts.max_tokens),` (Task 10 replaces this)
- `task_classifier.rs:203`: `max_tokens: Some(10),`
- `generate.rs:210`: `max_tokens: Some(MAX_TOKENS),`

- [ ] **Step 4: Sweep the remaining literals with the compiler.**

Run: `cargo check --workspace --all-targets 2>&1 | grep -E '^\s+--> ' | sort -u`

Fix every reported site with these rules:
- Inside an `LlmRequest { … }` literal, `max_tokens: <expr>,` becomes `max_tokens: Some(<expr>),`.
- A Rust-value assertion such as `assert_eq!(req.max_tokens, 100)` becomes `assert_eq!(req.max_tokens, Some(100))`.
- A JSON-body assertion such as `body["max_tokens"]` is unchanged.

Do not touch the unrelated `max_tokens` fields on `AgentSpec`, `SessionRecord`, `AgentRunOpts` or `LlmRequestWire`. Repeat until `cargo check --workspace --all-targets` is clean.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-providers && cargo test -p rupu-agent && cargo test -p rupu-orchestrator --lib`
Expected: PASS, including the four new wire tests.

- [ ] **Step 6: Commit**

```bash
git diff --name-only | grep '\.rs$' | xargs rustfmt --edition 2021
git add -u
git commit -m "feat(providers): LlmRequest.max_tokens is Option — unset omits the cap (Anthropic falls back to 8192)"
```

---

### Task 3: Anthropic `fetch_models` (limits, paging, token refresh)

**Files:**
- Modify: `crates/rupu-providers/src/anthropic.rs`: `models_request` (1407-1442), `list_models` (2527-2579) and `probe` (2584-2603)
- Test: `anthropic.rs` test module (next to `list_models_api_key_path_parses_v1_models`, ~5098)

**Interfaces:**
- Consumes: `LlmProvider::fetch_models` (Task 1).
- Produces: `impl LlmProvider for AnthropicClient { fetch_models }`. It returns every page with `context_window = max_input_tokens` and `max_output_tokens = max_tokens` (null → 0).

- [ ] **Step 1: Write the failing tests.**

```rust
    fn no_after_id(req: &httpmock::prelude::HttpMockRequest) -> bool {
        !req.query_params
            .as_ref()
            .is_some_and(|q| q.iter().any(|(k, _)| k == "after_id"))
    }

    #[tokio::test]
    async fn fetch_models_reads_limits_and_follows_pages() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let page1 = server.mock(|when, then| {
            when.method(GET).path("/v1/models").query_param("limit", "1000").matches(no_after_id);
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "claude-alpha-9", "type": "model", "max_input_tokens": 1000000, "max_tokens": 128000 }],
                "has_more": true, "first_id": "claude-alpha-9", "last_id": "claude-alpha-9"
            }));
        });
        let page2 = server.mock(|when, then| {
            when.method(GET).path("/v1/models").query_param("after_id", "claude-alpha-9");
            then.status(200).json_body(serde_json::json!({
                "data": [{ "id": "claude-beta-2-20260101", "type": "model", "max_input_tokens": null, "max_tokens": null }],
                "has_more": false, "first_id": "claude-beta-2-20260101", "last_id": "claude-beta-2-20260101"
            }));
        });
        let mut client = AnthropicClient::with_url(
            "sk-ant-test".into(),
            format!("{}/v1/messages?beta=true", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.unwrap();
        page1.assert();
        page2.assert();
        assert_eq!(models.len(), 2);
        assert_eq!((models[0].context_window, models[0].max_output_tokens), (1_000_000, 128_000));
        assert_eq!((models[1].context_window, models[1].max_output_tokens), (0, 0));
    }

    #[tokio::test]
    async fn fetch_models_oauth_sends_bearer_and_beta() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/v1/models")
                .header("authorization", "Bearer tok-live")
                .header("anthropic-beta", "oauth-2025-04-20");
            then.status(200).json_body(serde_json::json!({ "data": [], "has_more": false }));
        });
        let mut client = AnthropicClient::from_auth_with_url(
            AuthMethod::OAuth { access_token: "tok-live".into(), refresh_token: "r".into(), expires_ms: u64::MAX },
            format!("{}/v1/messages?beta=true", server.url("")),
            Arc::new(rupu_netflow::NullSink),
        );
        let models = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.unwrap();
        m.assert();
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn fetch_models_surfaces_non_2xx_as_error() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(401).body("{\"error\":\"nope\"}");
        });
        let mut client = AnthropicClient::with_url("k".into(), format!("{}/v1/messages", server.url("")), Arc::new(rupu_netflow::NullSink));
        let err = <AnthropicClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.unwrap_err();
        assert!(matches!(err, ProviderError::Api { status: 401, .. }), "{err:?}");
    }
```

Before you rely on `expires_ms: u64::MAX` avoiding a refresh, read `ensure_valid_token` (anthropic.rs:1142). If it refreshes on some other condition, set the fields it checks accordingly.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers fetch_models`
Expected: FAIL (the default `fetch_models` returns `NotImplemented`).

- [ ] **Step 3: Implement.**

Change `models_request` to take a cursor and always ask for the max page size:

```rust
    async fn models_request(&self, after_id: Option<&str>) -> Result<reqwest::Response, ProviderError> {
        // (existing base computation unchanged)
        let url = format!("{base}/v1/models");
        let mut query: Vec<(&str, &str)> = vec![("limit", "1000")];
        if let Some(a) = after_id {
            query.push(("after_id", a));
        }
        let mut req = self
            .client
            .get(&url)
            .query(&query)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Accept", "application/json");
        // (existing auth match unchanged)
```

Add an inherent method on `AnthropicClient`:

```rust
    /// Every page of `GET /v1/models`, with limits (spec 2026-09-30 §3).
    async fn fetch_all_models(&self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        #[derive(serde::Deserialize)]
        struct Page {
            data: Vec<Entry>,
            #[serde(default)]
            has_more: bool,
            #[serde(default)]
            last_id: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            id: String,
            #[serde(default)]
            max_input_tokens: Option<u32>,
            #[serde(default)]
            max_tokens: Option<u32>,
        }
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        // Bounded: a server that repeats `last_id` must not loop forever.
        for _ in 0..50 {
            let resp = self.models_request(after.as_deref()).await?;
            let status = resp.status();
            if !status.is_success() {
                let message: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
                return Err(ProviderError::Api { status: status.as_u16(), message });
            }
            let page: Page = resp.json().await.map_err(|e| ProviderError::Http(e.to_string()))?;
            out.extend(page.data.into_iter().map(|e| crate::model_pool::ModelInfo {
                id: e.id,
                provider: ProviderId::Anthropic,
                context_window: e.max_input_tokens.unwrap_or(0),
                max_output_tokens: e.max_tokens.unwrap_or(0),
                capabilities: Vec::new(),
                cost: crate::model_pool::ModelCost::default(),
                status: crate::model_pool::ModelStatus::default(),
            }));
            match (page.has_more, page.last_id) {
                (true, Some(last)) => after = Some(last),
                _ => break,
            }
        }
        Ok(out)
    }
```

Replace the body of `list_models`. It keeps its legacy "empty on error" contract, which `list_models_returns_empty_on_non_2xx` pins:

```rust
    async fn list_models(&self) -> Vec<crate::model_pool::ModelInfo> {
        match self.fetch_all_models().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "anthropic list_models failed");
                Vec::new()
            }
        }
    }

    async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        self.ensure_valid_token().await?;
        self.fetch_all_models().await
    }
```

In `probe`, update the call to `self.models_request(None)`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-providers anthropic`
Expected: PASS: the new tests, `list_models_api_key_path_parses_v1_models`, `list_models_returns_empty_on_non_2xx` and all four probe tests.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/anthropic.rs
git add crates/rupu-providers/src/anthropic.rs
git commit -m "feat(anthropic): fetch_models reads max_input_tokens/max_tokens across all /v1/models pages"
```

---

### Task 4: Codex / OpenAI `fetch_models`

**Files:**
- Modify: `crates/rupu-providers/src/openai_codex.rs`: struct (157-172), `new` (194), plus new helpers next to `extract_model_ids` (978)
- Test: `openai_codex.rs` test module

**Interfaces:**
- Consumes: Task 1 trait method.
- Produces:
  - `pub(crate) fn codex_input_limit(entry: &serde_json::Value) -> u32`
  - `pub(crate) fn models_from_listing(parsed: &serde_json::Value, api_key_mode: bool, provider: ProviderId) -> Vec<ModelInfo>`
  - a new struct field `chatgpt_models_url: String`
  - `impl LlmProvider for OpenAiCodexClient { fetch_models }`

- [ ] **Step 1: Write the failing tests.**

```rust
    #[test]
    fn codex_input_limit_applies_effective_percent() {
        let e = serde_json::json!({ "slug": "a", "context_window": 272000, "effective_context_window_percent": 95 });
        assert_eq!(codex_input_limit(&e), 258_400);
    }

    #[test]
    fn codex_input_limit_defaults_percent_to_95_and_falls_back_to_max_window() {
        assert_eq!(codex_input_limit(&serde_json::json!({ "context_window": 100000 })), 95_000);
        assert_eq!(codex_input_limit(&serde_json::json!({ "max_context_window": 200000 })), 190_000);
        assert_eq!(codex_input_limit(&serde_json::json!({ "slug": "x" })), 0);
    }

    #[test]
    fn listing_filters_supported_in_api_only_in_api_key_mode() {
        let v = serde_json::json!({ "models": [
            { "slug": "m-api", "context_window": 1000, "supported_in_api": true },
            { "slug": "m-app", "context_window": 1000, "supported_in_api": false }
        ]});
        let ids = |ms: Vec<crate::model_pool::ModelInfo>| ms.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(models_from_listing(&v, true, ProviderId::OpenaiCodex)), ["m-api"]);
        assert_eq!(ids(models_from_listing(&v, false, ProviderId::OpenaiCodex)), ["m-api", "m-app"]);
    }

    #[tokio::test]
    async fn fetch_models_oauth_backend_reads_limits() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/backend-api/codex/models").header("chatgpt-account-id", "acct-1");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "slug": "gpt-test-1", "context_window": 272000, "max_context_window": 872000, "effective_context_window_percent": 95 }
            ]}));
        });
        let mut client = OpenAiCodexClient::new(AuthCredentials::ApiKey { key: "t".into() }, None, Arc::new(rupu_netflow::NullSink)).unwrap();
        client.api_url = format!("{}/backend-api/codex/responses", server.url(""));
        client.account_id = "acct-1".into();
        let models = <OpenAiCodexClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.unwrap();
        m.assert();
        assert_eq!(models[0].id, "gpt-test-1");
        assert_eq!((models[0].context_window, models[0].max_output_tokens), (258_400, 0));
    }

    #[tokio::test]
    async fn fetch_models_api_key_tries_codex_backend_first() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let backend = server.mock(|when, then| {
            when.method(GET).path("/backend-api/codex/models").header("authorization", "Bearer sk-k");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "slug": "gpt-api", "context_window": 200000, "supported_in_api": true },
                { "slug": "gpt-app-only", "context_window": 200000, "supported_in_api": false }
            ]}));
        });
        let mut client = OpenAiCodexClient::new(AuthCredentials::ApiKey { key: "sk-k".into() }, None, Arc::new(rupu_netflow::NullSink)).unwrap();
        client.api_url = format!("{}/v1/responses", server.url(""));
        client.chatgpt_models_url = server.url("/backend-api/codex/models?client_version=0.50.0");
        let models = <OpenAiCodexClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.unwrap();
        backend.assert();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-api"]);
        assert_eq!(models[0].context_window, 190_000);
    }

    #[tokio::test]
    async fn fetch_models_api_key_falls_back_to_v1_models_ids() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/backend-api/codex/models");
            then.status(401);
        });
        let public = server.mock(|when, then| {
            when.method(GET).path("/v1/models");
            then.status(200).json_body(serde_json::json!({ "data": [{ "id": "gpt-plain" }] }));
        });
        let mut client = OpenAiCodexClient::new(AuthCredentials::ApiKey { key: "sk-k".into() }, None, Arc::new(rupu_netflow::NullSink)).unwrap();
        client.api_url = format!("{}/v1/responses", server.url(""));
        client.chatgpt_models_url = server.url("/backend-api/codex/models?client_version=0.50.0");
        let models = <OpenAiCodexClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.unwrap();
        public.assert();
        assert_eq!(models[0].id, "gpt-plain");
        assert_eq!(models[0].context_window, 0);
    }

    #[tokio::test]
    async fn fetch_models_errors_when_both_listings_fail() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| { when.method(GET); then.status(500); });
        let mut client = OpenAiCodexClient::new(AuthCredentials::ApiKey { key: "sk-k".into() }, None, Arc::new(rupu_netflow::NullSink)).unwrap();
        client.api_url = format!("{}/v1/responses", server.url(""));
        client.chatgpt_models_url = server.url("/backend-api/codex/models");
        assert!(<OpenAiCodexClient as crate::provider::LlmProvider>::fetch_models(&mut client).await.is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers openai_codex`
Expected: compile error (the helpers and field don't exist yet).

- [ ] **Step 3: Implement.**

Add a constant next to `CODEX_BACKEND_URL` (line 15):

```rust
/// The Codex model catalog. The only OpenAI endpoint that carries
/// per-model limits; the public `/v1/models` has ids only (spec 2026-09-30 §3).
const CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models?client_version=0.50.0";
```

Add the field `chatgpt_models_url: String` to `OpenAiCodexClient`, and set `chatgpt_models_url: CODEX_MODELS_URL.to_string(),` in every `OpenAiCodexClient { … }` / `Self { … }` construction in the file. Find them with `grep -n "Self {" crates/rupu-providers/src/openai_codex.rs`.

Add the helpers next to `extract_model_ids`:

```rust
/// A Codex `/codex/models` entry's usable input limit (spec §3):
/// `context_window × effective_context_window_percent / 100` (percent
/// defaults to 95, as in Codex), falling back to `max_context_window`
/// when `context_window` is absent. 0 means unknown.
pub(crate) fn codex_input_limit(entry: &serde_json::Value) -> u32 {
    let window = entry
        .get("context_window")
        .and_then(|v| v.as_u64())
        .or_else(|| entry.get("max_context_window").and_then(|v| v.as_u64()));
    let Some(window) = window else { return 0 };
    let pct = entry
        .get("effective_context_window_percent")
        .and_then(|v| v.as_u64())
        .unwrap_or(95)
        .clamp(1, 100);
    u32::try_from(window * pct / 100).unwrap_or(u32::MAX)
}

/// Model listing → `ModelInfo`s. The Codex `models` array carries limits;
/// the public `/v1/models` `data` array carries ids only (limits 0). In
/// API-key mode, entries with `supported_in_api: false` are dropped.
pub(crate) fn models_from_listing(
    parsed: &serde_json::Value,
    api_key_mode: bool,
    provider: crate::provider_id::ProviderId,
) -> Vec<crate::model_pool::ModelInfo> {
    if let Some(arr) = parsed.get("models").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter(|e| !(api_key_mode && e.get("supported_in_api").and_then(|v| v.as_bool()) == Some(false)))
            .filter_map(|e| {
                let id = ["slug", "id", "display_name", "name"]
                    .iter()
                    .find_map(|k| e.get(*k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()))?
                    .to_string();
                let mut mi = make_model_info(id, provider);
                mi.context_window = codex_input_limit(e);
                Some(mi)
            })
            .collect();
    }
    extract_model_ids(parsed)
        .into_iter()
        .map(|id| make_model_info(id, provider))
        .collect()
}
```

Add an inherent GET helper to `impl OpenAiCodexClient`:

```rust
    async fn get_models_json(&self, url: &str) -> Result<serde_json::Value, ProviderError> {
        let mut req = self
            .client
            .get(url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.access_token));
        if !self.account_id.is_empty() {
            req = req.header("chatgpt-account-id", &self.account_id);
        }
        let resp = req.send().await.map_err(|e| ProviderError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let message: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
            return Err(ProviderError::Api { status: status.as_u16(), message });
        }
        resp.json().await.map_err(|e| ProviderError::Http(e.to_string()))
    }
```

Add `fetch_models` to `impl LlmProvider for OpenAiCodexClient`. Leave `list_models` unchanged; the CLI moves to `fetch_models` in Task 9.

```rust
    async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        self.ensure_valid_token().await?;
        let pid = crate::provider_id::ProviderId::OpenaiCodex;
        if self.api_url.contains("/backend-api/codex/responses") {
            let url = format!("{}?client_version=0.50.0", self.api_url.replace("/responses", "/models"));
            return Ok(models_from_listing(&self.get_models_json(&url).await?, false, pid));
        }
        // API key: Codex metadata is only on the ChatGPT backend. Try it
        // first, then fall back to ids-only `/v1/models`.
        let backend = self.chatgpt_models_url.clone();
        match self.get_models_json(&backend).await {
            Ok(v) => {
                let models = models_from_listing(&v, true, pid);
                if !models.is_empty() {
                    return Ok(models);
                }
            }
            Err(e) => tracing::debug!(error = %e, "codex model catalog unavailable with an API key; falling back to /v1/models"),
        }
        let base = self.api_url.trim_end_matches("/v1/responses").trim_end_matches('/');
        let v = self.get_models_json(&format!("{base}/v1/models")).await?;
        Ok(models_from_listing(&v, false, pid))
    }
```

Read `ensure_valid_token` (openai_codex.rs:731) and confirm it returns `Ok(())` without network for `AuthCredentials::ApiKey` credentials (an empty `refresh_token`). If it doesn't, call it only when `!self.refresh_token.is_empty()`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-providers openai_codex`
Expected: PASS, new tests and existing `list_models_*` tests alike.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/openai_codex.rs
git add crates/rupu-providers/src/openai_codex.rs
git commit -m "feat(codex): fetch_models reads Codex catalog limits (effective window), API-key path tries the catalog first"
```

---

### Task 5: GitHub Copilot `fetch_models`

**Files:**
- Modify: `crates/rupu-providers/src/github_copilot.rs`: `ensure_valid_token` (235-322), `impl LlmProvider` (~340-370)
- Test: `github_copilot.rs` test module

**Interfaces:**
- Produces:
  - `pub(crate) fn copilot_models_from_listing(v: &serde_json::Value) -> Vec<ModelInfo>`
  - `fetch_models`
  - `output_shares_context() == false`

- [ ] **Step 1: Write the failing tests.**

```rust
    #[test]
    fn copilot_listing_prefers_max_prompt_tokens() {
        let v = serde_json::json!({ "data": [
            { "id": "chat-a", "capabilities": { "type": "chat", "limits": {
                "max_context_window_tokens": 400000, "max_prompt_tokens": 128000, "max_output_tokens": 64000 } } },
            { "id": "chat-b", "capabilities": { "type": "chat", "limits": { "max_context_window_tokens": 200000 } } },
            { "id": "embed-x", "capabilities": { "type": "embeddings", "limits": { "max_prompt_tokens": 8000 } } }
        ]});
        let ms = copilot_models_from_listing(&v);
        assert_eq!(ms.len(), 2, "embeddings are not chat models");
        assert_eq!((ms[0].context_window, ms[0].max_output_tokens), (128_000, 64_000));
        assert_eq!((ms[1].context_window, ms[1].max_output_tokens), (200_000, 0));
    }

    #[tokio::test]
    async fn fetch_models_calls_live_models_endpoint() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/models")
                .header("authorization", "Bearer cop-tok")
                .header("x-github-api-version", "2025-10-01");
            then.status(200).json_body(serde_json::json!({ "data": [
                { "id": "chat-a", "capabilities": { "type": "chat", "limits": { "max_prompt_tokens": 1000, "max_output_tokens": 500 } } }
            ]}));
        });
        let mut client = GithubCopilotClient::new(test_creds(), None, Arc::new(rupu_netflow::NullSink)).unwrap();
        client.copilot_token = "cop-tok".into();
        client.copilot_expires_ms = u64::MAX;
        client.api_url = server.url("");
        let ms = <GithubCopilotClient as LlmProvider>::fetch_models(&mut client).await.unwrap();
        m.assert();
        assert_eq!((ms[0].context_window, ms[0].max_output_tokens), (1000, 500));
        assert!(!<GithubCopilotClient as LlmProvider>::output_shares_context(&client));
    }
```

Put the HTTP test in the module that defines `test_creds()` (~454). Check that `is_copilot_expired(u64::MAX)` returns `false` (github_copilot.rs:374); if the check is written differently, pick an expiry far in the future that it treats as valid.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers github_copilot`
Expected: compile error.

- [ ] **Step 3: Implement.**

Add the parser, a free function near `make_model_info` (325):

```rust
/// Copilot `GET {api}/models` → `ModelInfo`s (spec 2026-09-30 §3). Input is
/// `max_prompt_tokens` (often far below the window), else
/// `max_context_window_tokens`; non-chat models are skipped.
pub(crate) fn copilot_models_from_listing(v: &serde_json::Value) -> Vec<crate::model_pool::ModelInfo> {
    let arr = v.get("data").and_then(|d| d.as_array()).or_else(|| v.as_array());
    arr.into_iter()
        .flatten()
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?;
            let caps = e.get("capabilities");
            if let Some(t) = caps.and_then(|c| c.get("type")).and_then(|t| t.as_str()) {
                if t != "chat" {
                    return None;
                }
            }
            let limits = caps.and_then(|c| c.get("limits"));
            let n = |k: &str| {
                limits
                    .and_then(|l| l.get(k))
                    .and_then(|x| x.as_u64())
                    .map(|x| x.min(u32::MAX as u64) as u32)
            };
            let mut mi = make_model_info(id);
            mi.context_window = n("max_prompt_tokens").or_else(|| n("max_context_window_tokens")).unwrap_or(0);
            mi.max_output_tokens = n("max_output_tokens").unwrap_or(0);
            Some(mi)
        })
        .collect()
}
```

Add to `impl LlmProvider for GithubCopilotClient`, and keep the built-in `list_models` as the offline fallback:

```rust
    async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        self.ensure_valid_token().await?;
        let mut headers = self.build_headers()?;
        headers.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
        headers.insert("X-GitHub-Api-Version", "2025-10-01".parse().unwrap());
        let resp = self
            .client
            .get(format!("{}/models", self.api_url))
            .headers(headers)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let message: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
            return Err(ProviderError::Api { status: status.as_u16(), message });
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| ProviderError::Http(e.to_string()))?;
        Ok(copilot_models_from_listing(&v))
    }

    fn output_shares_context(&self) -> bool {
        false
    }
```

`ensure_valid_token` already takes `&mut self` and returns early when the token is valid (check the head of its body at 235). If it doesn't return early when `copilot_token` is non-empty and unexpired, add that guard at the top:

```rust
        if !self.copilot_token.is_empty() && !is_copilot_expired(self.copilot_expires_ms) {
            return Ok(());
        }
```

Update the comment on the built-in `list_models` (351-360). It should now say this is the offline list and that live limits come from `fetch_models`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-providers github_copilot`
Expected: PASS, including `list_models_returns_baked_in_when_offline`.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/github_copilot.rs
git add crates/rupu-providers/src/github_copilot.rs
git commit -m "feat(copilot): fetch_models reads live /models limits (max_prompt_tokens), output budget independent"
```

---

### Task 6: Gemini AI Studio `fetch_models`

**Files:**
- Modify: `crates/rupu-providers/src/google_gemini.rs`: struct (131-142), constructor(s), `impl LlmProvider` (548-567)
- Test: `google_gemini.rs` `mod llm_provider_impl_tests` (~2234)

**Interfaces:**
- Produces:
  - `pub(crate) fn gemini_models_from_listing(v: &serde_json::Value, provider: ProviderId) -> (Vec<ModelInfo>, Option<String>)`, which returns the models and the next page token
  - a field `api_base_override: Option<String>`
  - `fetch_models`, which returns `NotImplemented` for GeminiCli and Antigravity
  - `output_shares_context() == false`

- [ ] **Step 1: Write the failing tests.**

```rust
    fn no_page_token(req: &httpmock::prelude::HttpMockRequest) -> bool {
        !req.query_params.as_ref().is_some_and(|q| q.iter().any(|(k, _)| k == "pageToken"))
    }

    #[test]
    fn gemini_listing_strips_prefix_and_reads_limits() {
        let v = serde_json::json!({ "models": [
            { "name": "models/gemini-test-pro", "inputTokenLimit": 1048576, "outputTokenLimit": 65536,
              "supportedGenerationMethods": ["generateContent", "countTokens"] },
            { "name": "models/text-embed-1", "inputTokenLimit": 2048, "outputTokenLimit": 1,
              "supportedGenerationMethods": ["embedContent"] }
        ], "nextPageToken": "p2" });
        let (ms, next) = gemini_models_from_listing(&v, crate::provider_id::ProviderId::GoogleGeminiCli);
        assert_eq!(next.as_deref(), Some("p2"));
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].id, "gemini-test-pro");
        assert_eq!((ms[0].context_window, ms[0].max_output_tokens), (1_048_576, 65_536));
    }

    #[tokio::test]
    async fn fetch_models_ai_studio_follows_pages() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let p1 = server.mock(|when, then| {
            when.method(GET).path("/v1beta/models").header("x-goog-api-key", "g-key").matches(no_page_token);
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-one", "inputTokenLimit": 10, "outputTokenLimit": 5, "supportedGenerationMethods": ["generateContent"] }
            ], "nextPageToken": "tok2" }));
        });
        let p2 = server.mock(|when, then| {
            when.method(GET).path("/v1beta/models").query_param("pageToken", "tok2");
            then.status(200).json_body(serde_json::json!({ "models": [
                { "name": "models/g-two", "inputTokenLimit": 20, "outputTokenLimit": 6, "supportedGenerationMethods": ["generateContent"] }
            ]}));
        });
        let mut client = GoogleGeminiClient::new(
            AuthCredentials::ApiKey { key: "g-key".into() },
            GeminiVariant::AiStudio,
            None,
            Arc::new(rupu_netflow::NullSink),
        ).unwrap();
        client.api_base_override = Some(server.url(""));
        let ms = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client).await.unwrap();
        p1.assert();
        p2.assert();
        assert_eq!(ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["g-one", "g-two"]);
    }

    #[tokio::test]
    async fn fetch_models_code_assist_has_no_listing() {
        let mut client = GoogleGeminiClient::new(oauth_creds(), GeminiVariant::GeminiCli, None, Arc::new(rupu_netflow::NullSink)).unwrap();
        let err = <GoogleGeminiClient as LlmProvider>::fetch_models(&mut client).await.unwrap_err();
        assert!(matches!(err, ProviderError::NotImplemented { .. }));
        assert!(!<GoogleGeminiClient as LlmProvider>::output_shares_context(&client));
    }
```

Use the `AuthCredentials` import path the file's tests already use (see `ai_studio_rejects_oauth_credentials` at ~2125).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers google_gemini`
Expected: compile error.

- [ ] **Step 3: Implement.**

Add the field `/// Test seam: replaces variant.endpoint() for model listing only.` followed by `pub(crate) api_base_override: Option<String>,` to `GoogleGeminiClient`. Set `api_base_override: None` in every `Self { … }` in the file (`grep -n "Self {" crates/rupu-providers/src/google_gemini.rs`).

Add the parser as a free function:

```rust
/// AI Studio `GET /v1beta/models` page → (`ModelInfo`s, next page token)
/// (spec 2026-09-30 §3). Keeps models that support `generateContent`.
pub(crate) fn gemini_models_from_listing(
    v: &serde_json::Value,
    provider: crate::provider_id::ProviderId,
) -> (Vec<crate::model_pool::ModelInfo>, Option<String>) {
    let models = v
        .get("models")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter(|e| {
            e.get("supportedGenerationMethods")
                .and_then(|m| m.as_array())
                .map_or(true, |ms| ms.iter().any(|x| x.as_str() == Some("generateContent")))
        })
        .filter_map(|e| {
            let name = e.get("name")?.as_str()?;
            let n = |k: &str| e.get(k).and_then(|x| x.as_u64()).map(|x| x.min(u32::MAX as u64) as u32).unwrap_or(0);
            Some(crate::model_pool::ModelInfo {
                id: name.strip_prefix("models/").unwrap_or(name).to_string(),
                provider,
                context_window: n("inputTokenLimit"),
                max_output_tokens: n("outputTokenLimit"),
                capabilities: Vec::new(),
                cost: crate::model_pool::ModelCost::default(),
                status: crate::model_pool::ModelStatus::default(),
            })
        })
        .collect();
    let next = v.get("nextPageToken").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(str::to_string);
    (models, next)
}
```

Add to `impl LlmProvider for GoogleGeminiClient`:

```rust
    async fn fetch_models(&mut self) -> Result<Vec<crate::model_pool::ModelInfo>, ProviderError> {
        // Code Assist (`v1internal`) has no listing method (spec §3).
        if self.variant != GeminiVariant::AiStudio {
            return Err(ProviderError::NotImplemented { provider: self.provider_id().to_string() });
        }
        let base = self
            .api_base_override
            .clone()
            .unwrap_or_else(|| self.variant.endpoint().to_string());
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..50 {
            let mut query: Vec<(&str, String)> = vec![("pageSize", "1000".to_string())];
            if let Some(t) = &page_token {
                query.push(("pageToken", t.clone()));
            }
            let resp = self
                .client
                .get(format!("{base}/v1beta/models"))
                .query(&query)
                .header("x-goog-api-key", &self.access_token)
                .header(reqwest::header::ACCEPT, "application/json")
                .send()
                .await
                .map_err(|e| ProviderError::Http(e.to_string()))?;
            let status = resp.status();
            if !status.is_success() {
                let message: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
                return Err(ProviderError::Api { status: status.as_u16(), message });
            }
            let v: serde_json::Value = resp.json().await.map_err(|e| ProviderError::Http(e.to_string()))?;
            let (models, next) = gemini_models_from_listing(&v, self.variant.provider_id());
            out.extend(models);
            match next {
                Some(t) => page_token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    fn output_shares_context(&self) -> bool {
        false
    }
```

`self.variant.provider_id()` is the private method at `GeminiVariant`'s impl (74-127); it is visible within the file. Update the comment on `list_models_returns_empty_until_ai_studio_wired`: `list_models` still defaults to empty, and live limits come from `fetch_models`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-providers google_gemini`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/google_gemini.rs
git add crates/rupu-providers/src/google_gemini.rs
git commit -m "feat(gemini): AI Studio fetch_models (inputTokenLimit/outputTokenLimit); Code Assist reports no listing"
```

---

### Task 7: OpenAI-compatible (vLLM) `fetch_models`; drop the made-up defaults

**Files:**
- Modify: `crates/rupu-providers/src/openai_compatible.rs`: `impl LlmProvider` (~265)
- Modify: `crates/rupu-runtime/src/provider_factory.rs:64-94`
- Test: `openai_compatible.rs` test module; `provider_factory.rs` tests

**Interfaces:**
- Produces:
  - `pub(crate) fn vllm_models_from_listing(v: &serde_json::Value) -> Vec<ModelInfo>`
  - `fetch_models`
  - unset config limits are 0 (unknown), no longer 32,768 / 8,192

- [ ] **Step 1: Write the failing tests** in `openai_compatible.rs`:

```rust
    #[test]
    fn vllm_listing_reads_max_model_len_and_inherits_for_lora() {
        let v = serde_json::json!({ "object": "list", "data": [
            { "id": "base-model", "object": "model", "max_model_len": 131072, "parent": null },
            { "id": "my-lora", "object": "model", "max_model_len": null, "parent": "base-model" },
            { "id": "mystery", "object": "model" }
        ]});
        let ms = vllm_models_from_listing(&v);
        let get = |id: &str| ms.iter().find(|m| m.id == id).unwrap().context_window;
        assert_eq!(get("base-model"), 131_072);
        assert_eq!(get("my-lora"), 131_072);
        assert_eq!(get("mystery"), 0);
    }

    #[tokio::test]
    async fn fetch_models_calls_v1_models() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/v1/models").header("authorization", "Bearer k");
            then.status(200).json_body(serde_json::json!({ "data": [{ "id": "base-model", "max_model_len": 4096 }] }));
        });
        let mut c = OpenAiCompatibleClient::new(&format!("{}/v1", server.url("")), "k", "base-model", vec![], true, Arc::new(rupu_netflow::NullSink));
        let ms = <OpenAiCompatibleClient as LlmProvider>::fetch_models(&mut c).await.unwrap();
        m.assert();
        assert_eq!(ms[0].context_window, 4096);
        assert_eq!(ms[0].provider, crate::provider_id::ProviderId::OpenaiCompatible);
    }
```

In `provider_factory.rs`'s `tests` module:

```rust
    #[test]
    fn openai_compatible_unset_limits_are_unknown_not_fabricated() {
        let mut providers = std::collections::BTreeMap::new();
        providers.insert("box".to_string(), rupu_config::ProviderConfig {
            kind: Some("openai-compatible".into()),
            base_url: Some("http://127.0.0.1:1".into()),
            models: vec![rupu_config::CustomModel { id: "m".into(), context_window: None, max_output: None }],
            ..Default::default()
        });
        let p = openai_compatible_params("box", &providers).unwrap();
        assert_eq!((p.models[0].context_window, p.models[0].max_output), (0, 0));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers openai_compatible && cargo test -p rupu-runtime openai_compatible_unset`
Expected: FAIL.

- [ ] **Step 3: Implement.**

In `provider_factory.rs`, delete `DEFAULT_OAI_CONTEXT_WINDOW` and `DEFAULT_OAI_MAX_OUTPUT`, and map with `unwrap_or(0)`:

```rust
            // Unset means unknown (0): the live `/v1/models` `max_model_len`
            // fills it in (spec 2026-09-30 §3). Never invent a window.
            context_window: m.context_window.unwrap_or(0),
            max_output: m.max_output.unwrap_or(0),
```

Run `grep -rn "32_768\|32768" crates --include=*.rs` and update any test that asserted the old defaults.

In `openai_compatible.rs`, add the parser and `fetch_models`:

```rust
/// vLLM-style `GET /v1/models` → `ModelInfo`s (spec §3). `max_model_len` is the
/// input limit; a LoRA entry with a null value inherits its `parent`'s.
pub(crate) fn vllm_models_from_listing(v: &serde_json::Value) -> Vec<ModelInfo> {
    let entries: Vec<&serde_json::Value> = v
        .get("data")
        .and_then(|d| d.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let own = |e: &serde_json::Value| e.get("max_model_len").and_then(|x| x.as_u64());
    entries
        .iter()
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?.to_string();
            let len = own(e).or_else(|| {
                let parent = e.get("parent")?.as_str()?;
                entries
                    .iter()
                    .find(|p| p.get("id").and_then(|x| x.as_str()) == Some(parent))
                    .and_then(|p| own(p))
            });
            Some(ModelInfo {
                id,
                provider: ProviderId::OpenaiCompatible,
                context_window: len.map_or(0, |n| n.min(u32::MAX as u64) as u32),
                max_output_tokens: 0,
                capabilities: Vec::new(),
                cost: crate::model_pool::ModelCost::default(),
                status: crate::model_pool::ModelStatus::default(),
            })
        })
        .collect()
}
```

In `impl LlmProvider for OpenAiCompatibleClient`:

```rust
    async fn fetch_models(&mut self) -> Result<Vec<ModelInfo>, ProviderError> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .headers(self.headers(false)?)
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let message: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
            return Err(ProviderError::Api { status: status.as_u16(), message });
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| ProviderError::Http(e.to_string()))?;
        Ok(vllm_models_from_listing(&v))
    }
```

Use the file's existing `ProviderId` import (`crate::provider_id::ProviderId`) if `ProviderId` isn't imported yet.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-providers openai_compatible && cargo test -p rupu-runtime`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/openai_compatible.rs crates/rupu-runtime/src/provider_factory.rs
git add crates/rupu-providers/src/openai_compatible.rs crates/rupu-runtime/src/provider_factory.rs
git commit -m "feat(openai-compatible): fetch_models reads /v1/models max_model_len; stop inventing 32768/8192 limits"
```

---

### Task 8: Registry cache v2 and model-id matching

**Files:**
- Modify: `crates/rupu-providers/src/model_registry.rs`
- Test: new `#[cfg(test)] mod tests` in `model_registry.rs`

**Interfaces:**
- Produces:
  - `ModelEntry { id, context_window: u32, max_output_tokens: u32 }`
  - `CacheFile.schema`
  - `pub async fn fetched_at(&self, provider: &str) -> Option<DateTime<Utc>>`
  - `pub async fn find_live(&self, provider: &str, model: &str) -> Option<ModelInfo>`
  - `pub fn match_model_id<'a, I: IntoIterator<Item = &'a str>>(ids: I, model: &str) -> Option<&'a str>`
  - `pub fn strip_1m(model: &str) -> &str`

- [ ] **Step 1: Write the failing tests** at the end of `model_registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn mi(id: &str, cw: u32, mo: u32) -> ModelInfo {
        let mut m = make_model_info(id.to_string(), "anthropic");
        m.context_window = cw;
        m.max_output_tokens = mo;
        m
    }

    #[tokio::test]
    async fn v2_cache_round_trips_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.set_live_cache("anthropic", vec![mi("claude-a", 1_000_000, 128_000)]).await;
        r.save_cache("anthropic").await.unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty(), "atomic write leaves no temp files");
        let r2 = ModelRegistry::with_cache_dir(tmp.path());
        r2.load_cache("anthropic").await.unwrap();
        let m = r2.find_live("anthropic", "claude-a").await.unwrap();
        assert_eq!((m.context_window, m.max_output_tokens), (1_000_000, 128_000));
        assert!(r2.fetched_at("anthropic").await.is_some());
        assert!(!r2.cache_is_stale("anthropic").await);
    }

    #[tokio::test]
    async fn v1_cache_is_treated_as_stale() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("anthropic.json"),
            r#"{"fetched_at":"2026-09-30T00:00:00Z","models":[{"id":"claude-a"}]}"#,
        ).unwrap();
        let r = ModelRegistry::with_cache_dir(tmp.path());
        r.load_cache("anthropic").await.unwrap();
        assert!(r.cache_is_stale("anthropic").await);
        assert!(r.find_live("anthropic", "claude-a").await.is_none());
    }

    #[test]
    fn match_prefers_exact_then_newest_snapshot() {
        let ids = ["claude-haiku-4-5-20250101", "claude-haiku-4-5-20251001", "claude-haiku-4-5-extra"];
        assert_eq!(match_model_id(ids, "claude-haiku-4-5"), Some("claude-haiku-4-5-20251001"));
        assert_eq!(match_model_id(["claude-x", "claude-x-20250101"], "claude-x"), Some("claude-x"));
        assert_eq!(match_model_id(["claude-y"], "claude-z"), None);
    }

    #[test]
    fn strip_1m_is_case_insensitive() {
        assert_eq!(strip_1m("claude-sonnet-4-6[1m]"), "claude-sonnet-4-6");
        assert_eq!(strip_1m("claude-sonnet-4-6[1M]"), "claude-sonnet-4-6");
        assert_eq!(strip_1m("claude-sonnet-4-6"), "claude-sonnet-4-6");
        assert_eq!(match_model_id(["claude-s"], "claude-s[1m]"), Some("claude-s"));
    }
}
```

`tempfile` is already a dev-dependency of `rupu-providers` (Cargo.toml line 32).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-providers model_registry`
Expected: compile error.

- [ ] **Step 3: Implement.**

```rust
/// Cache file schema. v1 (no `schema` field) stored ids only; it is treated
/// as stale so limits are refetched (spec 2026-09-30 §4).
const CACHE_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub context_window: u32,
    #[serde(default)]
    pub max_output_tokens: u32,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    schema: u32,
    fetched_at: DateTime<Utc>,
    models: Vec<ModelEntry>,
}
```

`save_cache` writes all three fields plus `schema: CACHE_SCHEMA`. The write is atomic:

```rust
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let tmp = self.cache_dir.join(format!(
                ".{provider}.json.{}.{}.tmp",
                std::process::id(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::write(&tmp, body)?;
            std::fs::rename(&tmp, &path)?;
```

In `load_cache`, after parsing, add `if cache.schema < CACHE_SCHEMA { return Ok(()); }`. When building entries, copy the limits: `let mut m = make_model_info(e.id, provider); m.context_window = e.context_window; m.max_output_tokens = e.max_output_tokens;`.

Add the accessors:

```rust
    /// When `provider`'s live list was fetched, if it is loaded.
    pub async fn fetched_at(&self, provider: &str) -> Option<DateTime<Utc>> {
        self.state.read().await.live.get(provider).map(|(ts, _)| *ts)
    }

    /// The live entry for `model` (exact id, else newest dated snapshot).
    pub async fn find_live(&self, provider: &str, model: &str) -> Option<ModelInfo> {
        let s = self.state.read().await;
        let (_, entries) = s.live.get(provider)?;
        let id = match_model_id(entries.iter().map(|m| m.id.as_str()), model)?;
        entries.iter().find(|m| m.id == id).cloned()
    }
```

Add the free functions:

```rust
/// Strip a trailing `[1m]` opt-in suffix (case-insensitive).
pub fn strip_1m(model: &str) -> &str {
    if model.len() >= 4 && model[model.len() - 4..].eq_ignore_ascii_case("[1m]") {
        &model[..model.len() - 4]
    } else {
        model
    }
}

/// Exact id, else the newest dated snapshot `<model>-YYYYMMDD` (spec §5).
pub fn match_model_id<'a, I: IntoIterator<Item = &'a str>>(ids: I, model: &str) -> Option<&'a str> {
    let want = strip_1m(model);
    let mut best: Option<&'a str> = None;
    for id in ids {
        if id == want {
            return Some(id);
        }
        let dated = id
            .strip_prefix(want)
            .and_then(|r| r.strip_prefix('-'))
            .is_some_and(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()));
        if dated && best.map_or(true, |b| id > b) {
            best = Some(id);
        }
    }
    best
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-providers model_registry && cargo test -p rupu-providers --test registry_resolution`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-providers/src/model_registry.rs
git add crates/rupu-providers/src/model_registry.rs
git commit -m "feat(registry): v2 model cache keeps limits (atomic write), find_live + dated-snapshot matching"
```

---

### Task 9: `rupu_runtime::model_limits` (resolve, refresh, catalog)

**Files:**
- Create: `crates/rupu-runtime/src/model_limits.rs`
- Modify: `crates/rupu-runtime/src/lib.rs` (add `pub mod model_limits;`)
- Modify: `crates/rupu-runtime/Cargo.toml` (add `futures-util.workspace = true` under `[dependencies]`)
- Modify: `crates/rupu-runtime/src/provider_factory.rs` (add `provider_config_for`)
- Create: `crates/rupu-runtime/tests/model_limits.rs`

**Interfaces:**
- Consumes:
  - `LlmProvider::fetch_models` and `output_shares_context` (Task 1)
  - `ModelRegistry::{find_live, fetched_at}`, `match_model_id` (Task 8)
  - `ModelLimits` (Task 1)
- Produces (all `pub` in `rupu_runtime::model_limits`):
  - `FETCH_TIMEOUT: Duration`
  - `struct LimitsContext { pub cache_dir: PathBuf, pub custom: HashMap<String, Vec<rupu_config::CustomModel>> }`, with `LimitsContext::from_config(&Config, &Path)` and `LimitsContext::for_cache_dir(PathBuf)`
  - `fn cache_dir(global_dir: &Path) -> PathBuf`
  - `struct LimitOverrides { pub context_window_tokens: Option<u32>, pub max_tokens: Option<u32>, pub compact_at_percent: Option<u8> }` with `LimitOverrides::from_spec(&rupu_agent::AgentSpec)`
  - `async fn resolve(overrides: LimitOverrides, provider_name: &str, model: &str, provider: &mut dyn LlmProvider, ctx: &LimitsContext) -> ModelLimits`
  - `struct UnknownProvider(pub String)` (a `thiserror` error)
  - `const BUILTIN_PROVIDERS: [&str; 4]`
  - `fn provider_names(cfg) -> Vec<String>`
  - `fn resolve_targets(filter: Option<&str>, cfg, cfg_path: &Path) -> Result<Vec<String>, UnknownProvider>`
  - `#[derive(Serialize)] struct RefreshOutcome { provider: String, ok: bool, count: usize, error: Option<String> }`
  - `async fn refresh(cfg, global_dir: &Path, cfg_path: &Path, resolver: &dyn rupu_auth::CredentialResolver, only: Option<&str>) -> Result<Vec<RefreshOutcome>, UnknownProvider>`
  - `#[derive(Serialize)] struct CatalogModel { id: String, input_tokens: Option<u32>, output_tokens: Option<u32>, source: String }` and `#[derive(Serialize)] struct CatalogProvider { provider: String, fetched_at: Option<DateTime<Utc>>, stale: bool, models: Vec<CatalogModel> }`
  - `async fn catalog(cfg, global_dir: &Path, cfg_path: &Path, only: Option<&str>) -> Result<Vec<CatalogProvider>, UnknownProvider>`
  - `provider_factory::provider_config_for(name: &str, providers: &BTreeMap<String, rupu_config::ProviderConfig>) -> ProviderConfig`

- [ ] **Step 1: Write the failing tests** in `crates/rupu-runtime/tests/model_limits.rs`:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rupu_providers::model_limits::LimitSource;
use rupu_providers::types::{LlmRequest, LlmResponse, StreamEvent};
use rupu_providers::{LlmProvider, ModelCost, ModelInfo, ModelStatus, ProviderError, ProviderId};
use rupu_runtime::model_limits::{resolve, LimitOverrides, LimitsContext};

/// A provider whose `fetch_models` result is scripted and counted.
struct Fake {
    result: Result<Vec<(&'static str, u32, u32)>, &'static str>,
    calls: Arc<AtomicUsize>,
    shares: bool,
    id: ProviderId,
}

#[async_trait::async_trait]
impl LlmProvider for Fake {
    async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> { unreachable!() }
    async fn stream(&mut self, _: &LlmRequest, _: &mut (dyn FnMut(StreamEvent) + Send)) -> Result<LlmResponse, ProviderError> { unreachable!() }
    fn default_model(&self) -> &str { "m" }
    fn provider_id(&self) -> ProviderId { self.id }
    async fn fetch_models(&mut self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.result {
            Ok(ms) => Ok(ms.iter().map(|(id, cw, mo)| ModelInfo {
                id: id.to_string(), provider: self.id, context_window: *cw, max_output_tokens: *mo,
                capabilities: vec![], cost: ModelCost::default(), status: ModelStatus::default(),
            }).collect()),
            Err("not-implemented") => Err(ProviderError::NotImplemented { provider: "fake".into() }),
            Err(e) => Err(ProviderError::Http(e.to_string())),
        }
    }
    fn output_shares_context(&self) -> bool { self.shares }
}

fn fake(result: Result<Vec<(&'static str, u32, u32)>, &'static str>) -> (Fake, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (Fake { result, calls: calls.clone(), shares: true, id: ProviderId::Anthropic }, calls)
}

fn ctx(tmp: &tempfile::TempDir) -> LimitsContext {
    LimitsContext::for_cache_dir(tmp.path().join("cache/models"))
}

#[tokio::test]
async fn live_limits_are_used_and_cached() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, calls) = fake(Ok(vec![("claude-a", 1_000_000, 128_000)]));
    let l = resolve(LimitOverrides::default(), "anthropic", "claude-a", &mut p, &ctx(&tmp)).await;
    assert_eq!(l.input.tokens, Some(1_000_000));
    assert_eq!(l.output.tokens, Some(128_000));
    assert!(matches!(l.input.source, LimitSource::Live { stale: false, .. }));
    // Second resolve reads the fresh cache: no refetch.
    let l2 = resolve(LimitOverrides::default(), "anthropic", "claude-a", &mut p, &ctx(&tmp)).await;
    assert_eq!(l2.input.tokens, Some(1_000_000));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn agent_beats_config_beats_live_per_field() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = ctx(&tmp);
    c.custom.insert("anthropic".into(), vec![rupu_config::CustomModel { id: "claude-a".into(), context_window: Some(500_000), max_output: None }]);
    let (mut p, _) = fake(Ok(vec![("claude-a", 1_000_000, 128_000)]));
    let o = LimitOverrides { context_window_tokens: None, max_tokens: Some(4096), compact_at_percent: Some(60) };
    let l = resolve(o, "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!((l.input.tokens, &l.input.source), (Some(500_000), &LimitSource::Config));
    assert_eq!((l.output.tokens, &l.output.source), (Some(4096), &LimitSource::Agent));
    assert_eq!(l.compact_at_percent, 60);
}

#[tokio::test]
async fn failed_refresh_falls_back_to_stale_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cache/models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("anthropic.json"), r#"{"schema":2,"fetched_at":"2020-01-01T00:00:00Z","models":[{"id":"claude-a","context_window":200000,"max_output_tokens":64000}]}"#).unwrap();
    let (mut p, calls) = fake(Err("connection refused"));
    let l = resolve(LimitOverrides::default(), "anthropic", "claude-a", &mut p, &ctx(&tmp)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(l.input.tokens, Some(200_000));
    assert!(matches!(l.input.source, LimitSource::Live { stale: true, .. }));
    assert!(l.note.as_deref().unwrap().contains("refresh failed"));
}

#[tokio::test]
async fn no_listing_is_unknown_with_a_note() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Err("not-implemented"));
    let l = resolve(LimitOverrides::default(), "gemini", "gemini-x", &mut p, &ctx(&tmp)).await;
    assert_eq!(l.input.tokens, None);
    assert_eq!(l.compact_threshold(), None);
    assert!(l.note.as_deref().unwrap().contains("exposes no model limits"));
    assert_eq!(l.output_fallback, Some(8192), "fake reports ProviderId::Anthropic");
}

#[tokio::test]
async fn dated_snapshot_and_1m_suffix_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![("claude-haiku-4-5-20251001", 200_000, 64_000)]));
    let l = resolve(LimitOverrides::default(), "anthropic", "claude-haiku-4-5[1m]", &mut p, &ctx(&tmp)).await;
    assert_eq!(l.input.tokens, Some(200_000));
    assert_eq!(l.compact_threshold(), Some(136_000));
}

#[tokio::test]
async fn model_missing_from_list_is_noted() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![("claude-a", 1, 1)]));
    let l = resolve(LimitOverrides::default(), "anthropic", "claude-zzz", &mut p, &ctx(&tmp)).await;
    assert_eq!(l.input.tokens, None);
    assert!(l.note.as_deref().unwrap().contains("not in anthropic's model list"));
}

#[test]
fn provider_names_and_targets() {
    let cfg = rupu_config::Config::default();
    assert_eq!(rupu_runtime::model_limits::provider_names(&cfg), ["anthropic", "openai", "gemini", "copilot"]);
    let err = rupu_runtime::model_limits::resolve_targets(Some("nope"), &cfg, std::path::Path::new("/x/config.toml")).unwrap_err();
    assert!(err.to_string().contains("unknown provider 'nope'"));
}
```

Add `async-trait` and `rupu-config` to `[dev-dependencies]` in `crates/rupu-runtime/Cargo.toml` if the test won't compile without them (`async-trait.workspace = true`, `rupu-config = { path = "../rupu-config" }`).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-runtime --test model_limits`
Expected: compile error (module missing).

- [ ] **Step 3: Add `provider_config_for`** to `provider_factory.rs`, next to `resolve_kind`:

```rust
/// `ProviderConfig` for `name` with no agent-level overrides: what a bare
/// model-listing call needs (spec 2026-09-30 §5).
pub fn provider_config_for(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> ProviderConfig {
    ProviderConfig {
        anthropic_oauth_system_prefix: None,
        anthropic_prompt_cache: None,
        openai_compatible: openai_compatible_params(name, providers),
        tuning: Some(provider_tuning(name, providers)),
        kind: resolve_kind(name, providers),
    }
}
```

- [ ] **Step 4: Write `crates/rupu-runtime/src/model_limits.rs`.** Move `PROVIDERS`/`provider_names`, `resolve_targets` (with its exact error text), `kind_of`, `build_registry` and `make_model_info` here from `crates/rupu-cli/src/cmd/models.rs:55-171, 422-510`. The CLI copies are deleted in Task 12.

```rust
//! Model-limit discovery + resolution (spec
//! `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md`).
//! The CLI (`rupu models`), the CP (`ModelCatalog` port) and every run
//! launch share this module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rupu_providers::model_limits::{
    Limit, LimitSource, ModelLimits, ANTHROPIC_FALLBACK_MAX_TOKENS, DEFAULT_COMPACT_AT_PERCENT,
};
use rupu_providers::model_registry::match_model_id;
use rupu_providers::{LlmProvider, ModelRegistry, ModelSource, ProviderError, ProviderId};
use serde::Serialize;

use crate::provider_factory;

/// Model-list fetch timeout (spec §3).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// What `resolve` needs from config. Owned, so long-lived holders (the step
/// factory, the dispatcher) keep it without the whole `Config`.
#[derive(Debug, Clone, Default)]
pub struct LimitsContext {
    pub cache_dir: PathBuf,
    /// `[providers.<name>].models`, keyed by provider/account name.
    pub custom: HashMap<String, Vec<rupu_config::CustomModel>>,
}

impl LimitsContext {
    pub fn from_config(cfg: &rupu_config::Config, global_dir: &Path) -> Self {
        Self {
            cache_dir: cache_dir(global_dir),
            custom: cfg
                .providers
                .iter()
                .filter(|(_, p)| !p.models.is_empty())
                .map(|(n, p)| (n.clone(), p.models.clone()))
                .collect(),
        }
    }

    pub fn for_cache_dir(cache_dir: PathBuf) -> Self {
        Self { cache_dir, custom: HashMap::new() }
    }
}

/// `<global>/cache/models`, or `$RUPU_CACHE_DIR_OVERRIDE` (the existing
/// `rupu models` test seam).
pub fn cache_dir(global_dir: &Path) -> PathBuf {
    std::env::var("RUPU_CACHE_DIR_OVERRIDE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| global_dir.join("cache/models"))
}

/// Agent-frontmatter pins.
#[derive(Debug, Clone, Copy, Default)]
pub struct LimitOverrides {
    pub context_window_tokens: Option<u32>,
    pub max_tokens: Option<u32>,
    pub compact_at_percent: Option<u8>,
}

impl LimitOverrides {
    pub fn from_spec(spec: &rupu_agent::AgentSpec) -> Self {
        Self {
            context_window_tokens: spec.context_window_tokens,
            max_tokens: spec.max_tokens,
            compact_at_percent: spec.compact_at_percent,
        }
    }
}

/// Resolve the limits one run uses (spec §5). Precedence per field:
/// agent → config → live cache (refetched through `provider` when stale)
/// → unknown. Never errors: unknown is a valid, reported outcome.
pub async fn resolve(
    overrides: LimitOverrides,
    provider_name: &str,
    model: &str,
    provider: &mut dyn LlmProvider,
    ctx: &LimitsContext,
) -> ModelLimits {
    let registry = ModelRegistry::with_cache_dir(&ctx.cache_dir);
    if let Err(e) = registry.load_cache(provider_name).await {
        tracing::warn!(error = %e, provider = provider_name, "unreadable model cache; refetching");
    }
    let mut note: Option<String> = None;
    if registry.cache_is_stale(provider_name).await {
        match tokio::time::timeout(FETCH_TIMEOUT, provider.fetch_models()).await {
            Ok(Ok(models)) => {
                registry.set_live_cache(provider_name, models).await;
                if let Err(e) = registry.save_cache(provider_name).await {
                    tracing::warn!(error = %e, provider = provider_name, "failed to write model cache");
                }
            }
            Ok(Err(ProviderError::NotImplemented { .. })) => {
                note = Some(format!(
                    "{provider_name} exposes no model limits; set contextWindowTokens/maxTokens on the agent or [[providers.{provider_name}.models]]"
                ));
            }
            Ok(Err(e)) => note = Some(format!("model list refresh failed: {e}")),
            Err(_) => note = Some(format!("model list refresh timed out after {}s", FETCH_TIMEOUT.as_secs())),
        }
    }
    let fetched_at = registry.fetched_at(provider_name).await;
    let stale = registry.cache_is_stale(provider_name).await;
    let live = registry.find_live(provider_name, model).await;
    let custom = ctx.custom.get(provider_name).and_then(|ms| {
        let id = match_model_id(ms.iter().map(|m| m.id.as_str()), model)?;
        ms.iter().find(|m| m.id == id)
    });
    if note.is_none() && live.is_none() && fetched_at.is_some() && custom.is_none() {
        note = Some(format!("model '{model}' is not in {provider_name}'s model list"));
    }
    let pick = |pin: Option<u32>, cfg: Option<u32>, live: Option<u32>| -> Limit {
        if let Some(n) = pin {
            return Limit::new(n, LimitSource::Agent);
        }
        if let Some(n) = cfg.filter(|n| *n > 0) {
            return Limit::new(n, LimitSource::Config);
        }
        match (live.filter(|n| *n > 0), fetched_at) {
            (Some(n), Some(fetched_at)) => Limit::new(n, LimitSource::Live { fetched_at, stale }),
            _ => Limit::unknown(),
        }
    };
    ModelLimits {
        input: pick(
            overrides.context_window_tokens,
            custom.and_then(|c| c.context_window),
            live.as_ref().map(|m| m.context_window),
        ),
        output: pick(
            overrides.max_tokens,
            custom.and_then(|c| c.max_output),
            live.as_ref().map(|m| m.max_output_tokens),
        ),
        compact_at_percent: overrides
            .compact_at_percent
            .unwrap_or(DEFAULT_COMPACT_AT_PERCENT)
            .clamp(10, 95),
        output_shares_context: provider.output_shares_context(),
        output_fallback: (provider.provider_id() == ProviderId::Anthropic)
            .then_some(ANTHROPIC_FALLBACK_MAX_TOKENS),
        note,
    }
}
```

Then add, verbatim from the CLI apart from signature changes:

- **`BUILTIN_PROVIDERS` and `provider_names(cfg)`:** the CLI's `PROVIDERS` and `provider_names` (models.rs:55-83).
- **`UnknownProvider` and `resolve_targets`:**
  - Declare `#[derive(Debug, thiserror::Error)] #[error("{0}")] pub struct UnknownProvider(pub String);`.
  - `pub fn resolve_targets(filter: Option<&str>, cfg: &rupu_config::Config, cfg_path: &Path) -> Result<Vec<String>, UnknownProvider>` has the same logic as models.rs:119-139, with `anyhow::bail!(…)` replaced by `return Err(UnknownProvider(format!(…)))`. Keep the message text identical.
- **`kind_of`:** private, as in the CLI (models.rs:166-171).
- **`build_registry`:** `async fn build_registry(cfg: &rupu_config::Config, global_dir: &Path) -> ModelRegistry`. Same body as models.rs:422-485, but it uses `cache_dir(global_dir)`, returns the registry directly (no `anyhow`), and calls the moved `make_model_info` (models.rs:487-510).

Then write `refresh` and `catalog`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RefreshOutcome {
    pub provider: String,
    pub ok: bool,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Refetch live model lists (spec §8). Providers run in parallel, each
/// bounded by [`FETCH_TIMEOUT`]; one failing never fails the others.
pub async fn refresh(
    cfg: &rupu_config::Config,
    global_dir: &Path,
    cfg_path: &Path,
    resolver: &dyn rupu_auth::CredentialResolver,
    only: Option<&str>,
) -> Result<Vec<RefreshOutcome>, UnknownProvider> {
    let names = resolve_targets(only, cfg, cfg_path)?;
    let registry = ModelRegistry::with_cache_dir(cache_dir(global_dir));
    let registry = &registry;
    let jobs = names.iter().map(|name| async move {
        let fail = |error: String| RefreshOutcome { provider: name.clone(), ok: false, count: 0, error: Some(error) };
        let pcfg = provider_factory::provider_config_for(name, &cfg.providers);
        let model = provider_factory::resolve_model(
            None,
            cfg.default_model.as_deref(),
            pcfg.openai_compatible.as_ref().map(|p| p.default_model.as_str()),
        );
        let mut provider = match provider_factory::build_for_provider_with_config(
            name, &model, None, resolver, &pcfg, std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await
        {
            Ok((_, p)) => p,
            Err(provider_factory::FactoryError::NotWiredInV0(k)) => {
                return fail(format!("provider kind \"{k}\" is not wired for listing"))
            }
            Err(e) => return fail(e.to_string()),
        };
        match tokio::time::timeout(FETCH_TIMEOUT, provider.fetch_models()).await {
            Ok(Ok(models)) => {
                let count = models.len();
                registry.set_live_cache(name, models).await;
                if let Err(e) = registry.save_cache(name).await {
                    return fail(format!("fetched {count} models but could not write the cache: {e}"));
                }
                RefreshOutcome { provider: name.clone(), ok: true, count, error: None }
            }
            Ok(Err(ProviderError::NotImplemented { .. })) => fail(format!(
                "no live model-list endpoint — declare limits in [[providers.{name}.models]]"
            )),
            Ok(Err(e)) => fail(e.to_string()),
            Err(_) => fail(format!("timed out after {}s", FETCH_TIMEOUT.as_secs())),
        }
    });
    Ok(futures_util::future::join_all(jobs).await)
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogModel {
    pub id: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    /// `live` | `custom` | `baked-in`
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogProvider {
    pub provider: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub stale: bool,
    pub models: Vec<CatalogModel>,
}

/// The cached catalog, with no network (spec §8).
pub async fn catalog(
    cfg: &rupu_config::Config,
    global_dir: &Path,
    cfg_path: &Path,
    only: Option<&str>,
) -> Result<Vec<CatalogProvider>, UnknownProvider> {
    let names = resolve_targets(only, cfg, cfg_path)?;
    let registry = build_registry(cfg, global_dir).await;
    let mut out = Vec::new();
    for p in names {
        let fetched_at = registry.fetched_at(&p).await;
        let stale = fetched_at.is_some() && registry.cache_is_stale(&p).await;
        let models = registry
            .list(&p)
            .await
            .into_iter()
            .map(|m| CatalogModel {
                input_tokens: (m.entry.context_window > 0).then_some(m.entry.context_window),
                output_tokens: (m.entry.max_output_tokens > 0).then_some(m.entry.max_output_tokens),
                source: match m.source {
                    ModelSource::Custom => "custom",
                    ModelSource::Live => "live",
                    ModelSource::BakedIn => "baked-in",
                }
                .to_string(),
                id: m.entry.id,
            })
            .collect();
        out.push(CatalogProvider { provider: p, fetched_at, stale, models });
    }
    Ok(out)
}
```

Add `pub mod model_limits;` to `crates/rupu-runtime/src/lib.rs`, and `futures-util.workspace = true` to its `[dependencies]`. `rupu-runtime` already depends on `rupu-agent`, `rupu-auth`, `rupu-config`, `rupu-netflow` and `chrono`. Add `thiserror` only if it's missing; it's already listed.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-runtime`
Expected: PASS (new tests plus existing `provider_resolution`, `anthropic_prompt_cache`, `netflow_capture`).

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-runtime/src/model_limits.rs crates/rupu-runtime/src/provider_factory.rs crates/rupu-runtime/src/lib.rs crates/rupu-runtime/tests/model_limits.rs
git add crates/rupu-runtime
git commit -m "feat(runtime): model_limits — resolve (agent>config>live>unknown), refresh, catalog"
```

---

### Task 10: Runner consumes `ModelLimits`

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs`:
  - `AgentRunOpts` (674-815): remove `max_tokens`, `context_window_tokens` and `compact_at_percent`; add `limits`
  - `RunResult` (818): add `final_limits`
  - `is_context_overflow` (186-191)
  - `compact_context` (452-569)
  - the `RunStart` write (1027)
  - the per-turn request (1225-1242)
  - the overflow block (1325-1355)
  - the proactive trigger (1463-1483)
- Modify (production literals, behavior-preserving; Task 11 adds discovery):
  - `crates/rupu-cli/src/cmd/run.rs:954-960`
  - `crates/rupu-cli/src/cmd/dispatch.rs:444-450`
  - `crates/rupu-cli/src/cmd/session.rs:7738-7742`
  - `crates/rupu-orchestrator/src/step_factory.rs:490-502`
- Modify (test literals): every other `AgentRunOpts {` site listed by the compiler (about 57), plus `RunResult {` constructions (10)
- Test: `runner.rs` `mod context_trim_tests`; new `crates/rupu-agent/tests/runner_model_limits.rs`

**Interfaces:**
- Consumes: `ModelLimits` (Task 1).
- Produces:
  - `AgentRunOpts.limits: ModelLimits`
  - `RunResult.final_limits: ModelLimits`
  - `pub(crate) fn parse_context_overflow(err: &str) -> Option<Overflow>`, with `pub(crate) struct Overflow { pub tokens: Option<u32>, pub max: Option<u32> }`
  - `Notice` kinds `model_limits` and `model_limits_clamped`

- [ ] **Step 1: Write the failing unit tests.** In `mod context_trim_tests` (2669), rename the existing `is_context_overflow` tests to use the new function:

```rust
    #[test]
    fn overflow_formats_parse_tokens_and_max() {
        let cases = [
            (r#"bad request: {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#, Some(215_000), Some(200_000)),
            ("API error 400: This model's maximum context length is 128000 tokens. However, your messages resulted in 130500 tokens.", Some(130_500), Some(128_000)),
            ("API error 400: Input length (140000) exceeds model's maximum context length (131072).", Some(140_000), Some(131_072)),
            ("bad request: The input token count (1100000) exceeds the maximum number of tokens allowed (1048576).", Some(1_100_000), Some(1_048_576)),
            ("prompt is too long for the model", None, None),
            ("too many tokens in request", None, None),
            ("exceeds context window limit", None, None),
        ];
        for (msg, tokens, max) in cases {
            assert_eq!(parse_context_overflow(msg), Some(Overflow { tokens, max }), "{msg}");
        }
    }

    #[test]
    fn overflow_ignores_unrelated_errors() {
        for msg in ["network error", "invalid api key", "rate limited"] {
            assert_eq!(parse_context_overflow(msg), None);
        }
    }

    #[test]
    fn overflow_numbers_accept_thousands_separators() {
        assert_eq!(
            parse_context_overflow("prompt is too long: 215,000 tokens > 200,000 maximum"),
            Some(Overflow { tokens: Some(215_000), max: Some(200_000) })
        );
    }
```

Update the module's `use super::{…}` to import `parse_context_overflow, Overflow` instead of `is_context_overflow`.

- [ ] **Step 2: Write the failing integration test** `crates/rupu-agent/tests/runner_model_limits.rs`. Model it on `crates/rupu-agent/tests/runner_usage_hook.rs`: copy its `build_opts`, `dense_msg`, `usage` and `final_text_turn` helpers into this file, replacing the three removed fields with `limits: ModelLimits::unknown()`.

```rust
use rupu_agent::runner::{CapturingMockProvider, MockProvider, ScriptedTurn};
use rupu_agent::run_agent;
use rupu_providers::model_limits::ModelLimits;
use rupu_providers::types::{Role, StopReason};
use rupu_transcript::{Event, JsonlReader};

// … build_opts / dense_msg / usage / final_text_turn copied from runner_usage_hook.rs …

fn notices(path: &std::path::Path) -> Vec<(String, String)> {
    JsonlReader::iter(path)
        .unwrap()
        .filter_map(|e| match e.ok()? {
            Event::Notice { kind, message } => Some((kind, message)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn resolved_output_cap_goes_on_the_wire_and_is_announced() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(), stop: StopReason::EndTurn, input_tokens: 1, output_tokens: 1,
    }]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.limits = ModelLimits::fixed(1_000_000, 128_000);
    run_agent(opts).await.unwrap();
    assert_eq!(captured.lock().unwrap()[0].max_tokens, Some(128_000));
    let n = notices(&transcript);
    assert!(n.iter().any(|(k, m)| k == "model_limits" && m.starts_with("input 1,000,000 · output 128,000")), "{n:?}");
}

#[tokio::test]
async fn unknown_output_cap_is_not_sent() {
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "done".into(), stop: StopReason::EndTurn, input_tokens: 1, output_tokens: 1,
    }]);
    let captured = provider.captured.clone();
    let tmp = tempfile::tempdir().unwrap();
    let opts = build_opts(Box::new(provider), &tmp, tmp.path().join("run.jsonl"));
    run_agent(opts).await.unwrap();
    assert_eq!(captured.lock().unwrap()[0].max_tokens, None);
}

#[tokio::test]
async fn overflow_error_clamps_the_limit_and_compacts_instead_of_trimming() {
    let provider = MockProvider::new(vec![
        ScriptedTurn::ProviderError("prompt is too long: 1500 tokens > 1000 maximum".into()),
        final_text_turn(usage(100, 10, 0)), // the compaction summariser's send
        final_text_turn(usage(300, 6, 0)),  // the retried turn
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("run.jsonl");
    let mut opts = build_opts(Box::new(provider), &tmp, transcript.clone());
    opts.initial_messages = vec![
        dense_msg(Role::User, "task"),
        dense_msg(Role::Assistant, "assistant 0"),
        dense_msg(Role::User, "user 0"),
        dense_msg(Role::Assistant, "assistant 1"),
    ];
    opts.limits = ModelLimits::unknown().with_input(1_000_000).with_percent(50);
    let result = run_agent(opts).await.unwrap();
    assert_eq!(result.final_limits.input.tokens, Some(1000));
    let n = notices(&transcript);
    assert!(n.iter().any(|(k, m)| k == "model_limits_clamped" && m.contains("1,000,000 → 1,000")), "{n:?}");
    assert!(!n.iter().any(|(k, _)| k == "context_trim"), "compaction, not trimming: {n:?}");
    let compactions = JsonlReader::iter(&transcript).unwrap()
        .filter(|e| matches!(e, Ok(Event::Compaction { .. }))).count();
    assert_eq!(compactions, 1);
}
```

`build_opts` in `runner_usage_hook.rs` takes the provider by value (`build_opts(provider, &tmp, transcript_path)`). Match its real signature when copying, whether that's `impl LlmProvider + 'static` or `Box<dyn LlmProvider>`. The dense four-message seed and the 50% setting mirror `runner_usage_hook.rs:208-225`, which is known to give `partition_for_compaction` a middle to summarize.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rupu-agent`
Expected: compile errors (`limits`, `final_limits` and `parse_context_overflow` don't exist yet).

- [ ] **Step 4: Change the structs.** In `AgentRunOpts`, replace the three fields (783-788) with:

```rust
    /// Resolved model limits (spec 2026-09-30 §5): the request `max_tokens`
    /// (`output`), the compaction threshold (`compact_threshold()`), and the
    /// run-start notice. Launch sites build this with
    /// `rupu_runtime::model_limits::resolve`; tests use `ModelLimits::unknown()`
    /// / `fixed(..)`.
    pub limits: rupu_providers::model_limits::ModelLimits,
```

Add to `RunResult`:

```rust
    /// `opts.limits` as the run ended, including any limit learned from an
    /// overflow error (spec §7). Sessions persist this.
    pub final_limits: rupu_providers::model_limits::ModelLimits,
```

`DEFAULT_MAX_TOKENS` (86) stays. `rupu-cp`'s agent DTO and the docs still reference it as the Anthropic fallback value, so change its doc comment to point at `model_limits::ANTHROPIC_FALLBACK_MAX_TOKENS`.

- [ ] **Step 5: Replace `is_context_overflow`** (186-191):

```rust
/// A provider context-overflow error, with the numbers when the message
/// carries them (spec 2026-09-30 §7; formats are observed, not documented).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Overflow {
    pub tokens: Option<u32>,
    pub max: Option<u32>,
}

pub(crate) fn parse_context_overflow(err: &str) -> Option<Overflow> {
    let e = err.to_ascii_lowercase();
    let after = |needle: &str| e.find(needle).map(|i| &e[i + needle.len()..]);
    // Anthropic: "prompt is too long: N tokens > M maximum"
    if let Some(rest) = after("prompt is too long:") {
        let n = numbers(rest);
        return Some(Overflow { tokens: n.first().copied(), max: n.get(1).copied() });
    }
    // OpenAI / Copilot: "maximum context length is M tokens … resulted in N tokens"
    if let Some(rest) = after("maximum context length is") {
        let n = numbers(rest);
        return Some(Overflow { max: n.first().copied(), tokens: n.get(1).copied() });
    }
    // vLLM: "Input length (N) exceeds model's maximum context length (M)"
    if e.contains("exceeds model's maximum context length") {
        if let Some(rest) = after("input length") {
            let n = numbers(rest);
            return Some(Overflow { tokens: n.first().copied(), max: n.get(1).copied() });
        }
    }
    // Gemini: "input token count (N) exceeds the maximum number of tokens allowed (M)"
    if let Some(rest) = after("input token count") {
        let n = numbers(rest);
        return Some(Overflow { tokens: n.first().copied(), max: n.get(1).copied() });
    }
    if e.contains("prompt is too long") || e.contains("too many tokens") || e.contains("context window") {
        return Some(Overflow { tokens: None, max: None });
    }
    None
}

/// Every integer in `s`, in order. A `,` or `_` between digits is a thousands separator.
fn numbers(s: &str) -> Vec<u32> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut cur: Option<u64> = None;
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_digit() {
            cur = Some(cur.unwrap_or(0).saturating_mul(10).saturating_add(u64::from(c - b'0')));
        } else if (c == b',' || c == b'_') && cur.is_some() && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            // thousands separator
        } else if let Some(n) = cur.take() {
            out.push(n.min(u64::from(u32::MAX)) as u32);
        }
    }
    if let Some(n) = cur {
        out.push(n.min(u64::from(u32::MAX)) as u32);
    }
    out
}
```

- [ ] **Step 6: Wire the runner.**

`compact_context` (468-471, 497-503):

```rust
    let context_window_tokens = match opts.limits.input.tokens {
        Some(w) => w,
        None => return false,
    };
```

Then pass `Some(opts.limits.compact_at_percent)` where `opts.compact_at_percent` was passed to `compact_messages`.

Right after the `writer.write(&Event::RunStart { … })?;` at 1027:

```rust
    writer.write(&Event::Notice {
        kind: "model_limits".into(),
        message: opts.limits.describe(&opts.provider_name, chrono::Utc::now()),
    })?;
```

Per-turn request (1229): `max_tokens: opts.limits.output.tokens,`.

Proactive trigger (1466-1467): `if let Some(threshold) = opts.limits.compact_threshold() {`.

Declare `let mut last_turn_input_tokens: u32 = 0;` before the turn loop, next to `compaction_seq` (1216), and set `last_turn_input_tokens = resp.usage.input_tokens;` right after the usage accounting at 1428.

Declare `let mut overflow_compacted = false;` next to `trim_attempts` (1243). Then change the overflow arm (1328-1355) to:

```rust
                    CallStep::Err(e) => {
                        let e_str = e.to_string();
                        if let Some(overflow) = parse_context_overflow(&e_str) {
                            // Learn the real limit, then compact once per turn (spec §7).
                            if !overflow_compacted {
                                overflow_compacted = true;
                                if let Some(max) = overflow.max {
                                    let before = opts.limits.input.tokens;
                                    if opts.limits.clamp_input(max) {
                                        writer.write(&Event::Notice {
                                            kind: "model_limits_clamped".into(),
                                            message: format!(
                                                "input {} → {} (provider error); if this recurs, pin contextWindowTokens on the agent",
                                                before.map_or("unknown".to_string(), |b| rupu_providers::model_limits::group_thousands(u64::from(b))),
                                                rupu_providers::model_limits::group_thousands(u64::from(max)),
                                            ),
                                        })?;
                                        writer.flush()?;
                                    }
                                }
                                compaction_seq += 1;
                                let run_id_clone = opts.run_id.clone();
                                let calibration = overflow.tokens.unwrap_or(last_turn_input_tokens).max(1);
                                if compact_context(&mut messages, &mut opts, &run_id_clone, compaction_seq, &mut writer, calibration).await {
                                    req.messages = messages.clone();
                                    continue;
                                }
                            }
                            // Last resort: today's trim loop.
                            if trim_attempts <= 64 {
                                if trim_oldest_exchange(&mut req.messages) > 0 {
                                    // … existing context_trim Notice + `continue` (1332-1343), unchanged …
                                }
                            }
                            // … existing RunComplete error + `return Err(RunError::ContextOverflow { turn: turn_idx })` (1345-1354), unchanged …
                        }
```

If the borrow checker rejects `&mut opts` / `&mut messages` inside the call loop, add a `CallOutcome::Overflow(Overflow, String)` variant. Break out of the loop with it, run the same clamp-and-compact code in the outer turn body, and retry the turn by rebuilding `req` from `messages`. Keep `overflow_compacted` scoped to the turn either way.

Every `RunResult { … }` construction in `runner.rs` gets `final_limits: opts.limits.clone(),`. List them with `grep -n "RunResult {" crates/rupu-agent/src/runner.rs`.

Update the `Notice` doc comment in `crates/rupu-transcript/src/event.rs:268-269` to list `{"context_trim", "provider_retry", "model_limits", "model_limits_clamped"}`.

- [ ] **Step 7: Update the production literals, preserving today's behavior.**

In `run.rs:954-960`, `dispatch.rs:444-450` and `step_factory.rs:490-502`, replace the three fields with:

```rust
            limits: rupu_providers::model_limits::ModelLimits::from_pins(
                spec.context_window_tokens,
                spec.max_tokens,
                spec.compact_at_percent,
            ),
```

In `session.rs:7738-7742` use the same call with `session.context_window_tokens`, `session.max_tokens` and `session.compact_at_percent`.

Unpinned `max_tokens` now yields `None` on the wire, so Anthropic still sends 8192 and OpenAI-family providers stop sending an 8192 cap. That is the intended change.

- [ ] **Step 8: Sweep the test literals with the compiler.**

Run: `cargo check --workspace --all-targets 2>&1 | grep -E '^\s+--> ' | sort -u`

In each `AgentRunOpts { … }` test literal, delete `max_tokens: …,`, `context_window_tokens: …,` and `compact_at_percent: …,`, and add one field:
- If the literal set neither `context_window_tokens` nor `max_tokens` beyond `DEFAULT_MAX_TOKENS`: `limits: rupu_providers::model_limits::ModelLimits::unknown(),`.
- If it set `context_window_tokens: Some(W)` and/or `compact_at_percent: Some(P)`: `limits: rupu_providers::model_limits::ModelLimits::unknown().with_input(W).with_percent(P),` (drop the parts that weren't set).

Where a test assigns `opts.context_window_tokens = Some(W); opts.compact_at_percent = Some(P);`, write `opts.limits = ModelLimits::unknown().with_input(W).with_percent(P);` instead. For example, `runner_usage_hook.rs:224-225`.

In each `RunResult { … }` literal outside `runner.rs` (tests and fakes), add `final_limits: rupu_providers::model_limits::ModelLimits::unknown(),`.

Tests that assert an exact transcript event sequence will now see one extra `Notice { kind: "model_limits" }` right after `RunStart`. Update those expectations; don't filter the notice out in the runner. Find them with `cargo test -p rupu-agent -p rupu-orchestrator -p rupu-cli 2>&1 | grep -B2 -A20 "panicked"`.

- [ ] **Step 9: Run the tests**

Run: `cargo test -p rupu-agent && cargo test -p rupu-orchestrator && cargo test -p rupu-cli`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git diff --name-only | grep '\.rs$' | xargs rustfmt --edition 2021
git add -u crates
git add crates/rupu-agent/tests/runner_model_limits.rs
git commit -m "feat(agent): runs use ModelLimits — wire max_tokens, headroom-aware compaction, limits notice, learn the limit from overflow errors"
```

---

### Task 11: Resolve limits at every launch site

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs` (provider build 676-684; dispatcher `new` call 815; opts literal 911-963)
- Modify: `crates/rupu-cli/src/cmd/dispatch.rs` (struct 26-94; `new` 110-130; `dispatch` 216-453)
- Modify the `CliAgentDispatcher::new` callers: `crates/rupu-cli/src/resume.rs:298`, `crates/rupu-cli/src/cmd/workflow.rs:3243`, `:4814`
- Modify: `crates/rupu-orchestrator/src/step_factory.rs` (struct 124-181; `build_opts_for_step` 315-505; test constructions 1220, 1810, 1865, 1944, 2144)
- Modify the `DefaultStepFactory` constructions: `crates/rupu-cli/src/resume.rs:350`, `crates/rupu-cli/src/cmd/workflow.rs:3296`, `:4875`
- Modify: `crates/rupu-cli/src/cmd/session.rs`: `SessionRecord` (345-455); `run_turn` (7526-7751) and its post-turn session write; the readers at 3604, 3741, 6109, 6504, 6762, 7276
- Test: `crates/rupu-cli/tests/` (new `run_model_limits.rs`)

**Interfaces:**
- Consumes: `rupu_runtime::model_limits::{resolve, LimitOverrides, LimitsContext}` (Task 9) and `AgentRunOpts.limits` / `RunResult.final_limits` (Task 10).
- Produces:
  - `DefaultStepFactory.limits_ctx: LimitsContext`
  - a `CliAgentDispatcher` field `limits_ctx`, plus a new trailing `new(…, limits_ctx: LimitsContext)` parameter
  - `SessionRecord.model_limits: Option<ModelLimits>`
  - `SessionRecord::{effective_context_window, effective_compact_at_percent}`

- [ ] **Step 1: Write the failing end-to-end test** `crates/rupu-cli/tests/run_model_limits.rs`. Use the harness style of `crates/rupu-cli/tests/models_subcommand.rs`: `assert_cmd`, `RUPU_HOME`, `RUPU_CACHE_DIR_OVERRIDE`, and `RUPU_ANTHROPIC_BASE_URL_OVERRIDE` pointing at httpmock. The CLI must discover the limits and send them:

```rust
use assert_cmd::Command;
use httpmock::prelude::*;

#[test]
fn rupu_run_sends_the_discovered_output_cap_and_announces_limits() {
    let server = MockServer::start();
    let list = server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{ "id": "claude-test-1", "type": "model", "max_input_tokens": 300000, "max_tokens": 50000 }],
            "has_more": false
        }));
    });
    let msgs = server.mock(|when, then| {
        when.method(POST).path("/v1/messages").body_contains("\"max_tokens\":50000");
        then.status(200).json_body(serde_json::json!({
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-test-1",
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn", "usage": { "input_tokens": 5, "output_tokens": 1 }
        }));
    });
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("agents")).unwrap();
    std::fs::write(
        home.path().join("agents/probe.md"),
        "---\nname: probe\nprovider: anthropic\nmodel: claude-test-1\n---\nSay ok.\n",
    ).unwrap();
    let ws = tempfile::tempdir().unwrap();
    Command::cargo_bin("rupu").unwrap()
        .current_dir(ws.path())
        .env("RUPU_HOME", home.path())
        .env("RUPU_CACHE_DIR_OVERRIDE", home.path().join("cache/models"))
        .env("RUPU_ANTHROPIC_BASE_URL_OVERRIDE", format!("{}/v1/messages", server.url("")))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .args(["run", "probe", "go", "--no-stream"])
        .assert()
        .success();
    list.assert();
    msgs.assert();
}
```

Before writing this test, read one existing `rupu run` end-to-end test in `crates/rupu-cli/tests/` (`grep -ln "\"run\"" crates/rupu-cli/tests/*.rs`) and copy its exact credential env var, agent file layout and flags. The names above are the likely ones, not verified. Keep the two assertions: the listing is hit, and the messages request carries `"max_tokens":50000`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cli --test run_model_limits`
Expected: FAIL. `max_tokens` is 8192 and `/v1/models` is never called.

- [ ] **Step 3: `rupu run`** (`run.rs`). Make the provider binding mutable: `let (_resolved_auth, mut provider) = …` (676). Before the `AgentRunOpts` literal:

```rust
        let limits_ctx = rupu_runtime::model_limits::LimitsContext::from_config(&cfg, &global);
        let limits = rupu_runtime::model_limits::resolve(
            rupu_runtime::model_limits::LimitOverrides::from_spec(&spec),
            &provider_name,
            &model,
            provider.as_mut(),
            &limits_ctx,
        )
        .await;
```

In the literal, use `limits,`. Pass `limits_ctx.clone()` as the new last argument of `CliAgentDispatcher::new` at 815.

- [ ] **Step 4: Sub-agent dispatch** (`dispatch.rs`). Add the field `limits_ctx: rupu_runtime::model_limits::LimitsContext` to the struct, add a trailing `limits_ctx` parameter to `new` and store it. In `dispatch`, bind `let mut provider = match built { … }` (354-363) and resolve for the child's own provider and model:

```rust
        let limits = rupu_runtime::model_limits::resolve(
            rupu_runtime::model_limits::LimitOverrides::from_spec(&spec),
            &provider_name,
            &model,
            provider.as_mut(),
            &self.limits_ctx,
        )
        .await;
```

Replace the `from_pins(…)` from Task 10 with `limits,`. Update the other three `new` callers (resume.rs:298, workflow.rs:3243, 4814) to pass `rupu_runtime::model_limits::LimitsContext::from_config(&cfg, &global)`, using whatever the config and global-dir variables are called at each call site.

- [ ] **Step 5: Workflow steps** (`step_factory.rs`). Add `pub limits_ctx: rupu_runtime::model_limits::LimitsContext,` to `DefaultStepFactory`. In `build_opts_for_step`, make the provider binding mutable (`let mut provider: Box<dyn …> = match load_err { … }`, 317). Resolve only when the agent loaded; a load-error stub gets `ModelLimits::unknown()`:

```rust
        let limits = if provider_name == "unresolved" {
            rupu_providers::model_limits::ModelLimits::unknown()
        } else {
            rupu_runtime::model_limits::resolve(
                rupu_runtime::model_limits::LimitOverrides::from_spec(&spec),
                &provider_name,
                &model,
                provider.as_mut(),
                &self.limits_ctx,
            )
            .await
        };
```

Do this before `spec` is partially moved into the literal: compute `limits` right after the provider match, while `spec` is still whole.

In the literal, use `limits,`. The production constructions (resume.rs:350, workflow.rs:3296, 4875) set `limits_ctx: rupu_runtime::model_limits::LimitsContext::from_config(&cfg, &global),`. The step_factory.rs test constructions (1220, 1810, 1865, 1944, 2144) set `limits_ctx: rupu_runtime::model_limits::LimitsContext::for_cache_dir(global.join("cache/models")),`, using that test's global/tmp path.

- [ ] **Step 6: Sessions** (`session.rs`).

Add to `SessionRecord` (after `compact_at_percent`, 454):

```rust
    /// Limits resolved on the session's first turn (spec 2026-09-30 §6.5),
    /// including any limit learned from an overflow error. Reused on every
    /// later turn; never refetched mid-session.
    #[serde(default)]
    model_limits: Option<rupu_providers::model_limits::ModelLimits>,
```

Set `model_limits: None,` in `start` (1585-1648) and in the test fixture (10074-10076).

Add the helpers:

```rust
impl SessionRecord {
    /// The input limit compaction uses: the agent pin, else the resolved one.
    fn effective_context_window(&self) -> Option<u32> {
        self.context_window_tokens
            .or_else(|| self.model_limits.as_ref().and_then(|l| l.input.tokens))
    }

    fn effective_compact_at_percent(&self) -> u8 {
        self.compact_at_percent
            .or_else(|| self.model_limits.as_ref().map(|l| l.compact_at_percent))
            .unwrap_or(rupu_providers::model_limits::DEFAULT_COMPACT_AT_PERCENT)
    }
}
```

In `run_turn`, make the provider binding mutable at 7587 and, before the `AgentRunOpts` literal:

```rust
        let limits = match session.model_limits.clone() {
            Some(l) => l,
            None => {
                rupu_runtime::model_limits::resolve(
                    rupu_runtime::model_limits::LimitOverrides {
                        context_window_tokens: session.context_window_tokens,
                        max_tokens: session.max_tokens,
                        compact_at_percent: session.compact_at_percent,
                    },
                    &session.provider_name,
                    &session.model,
                    provider.as_mut(),
                    &rupu_runtime::model_limits::LimitsContext::from_config(&cfg, &global),
                )
                .await
            }
        };
```

Replace the Task 10 `from_pins(…)` in the literal with `limits,`. Then, where `run_turn` updates the session record after `run_agent` returns (find the `write_session(` call after the `run_agent(` call in `run_turn`), set `session.model_limits = Some(result.final_limits.clone());` before that write. If `session` isn't reachable there because of the `async move` at 7569, return `result.final_limits` out of the block alongside the existing result and assign it after.

Replace the six readers:
- 3604 and 6109: `session.context_window_tokens.is_none()` becomes `session.effective_context_window().is_none()`. Change the message to `"compact unavailable  ·  this session's model limits are unknown and it has no contextWindowTokens"`, keeping each site's existing prefix.
- 3741 and 6504: `session.context_window_tokens` becomes `session.effective_context_window()`, and `session.compact_at_percent.unwrap_or(80)` becomes `session.effective_compact_at_percent()`.
- 6762: `.or(session.context_window_tokens)` becomes `.or(session.effective_context_window())`. Where that function passes `session.compact_at_percent` to `compact_messages`, pass `Some(session.effective_compact_at_percent())`.
- 7276: `match session.context_window_tokens` becomes `match session.effective_context_window()`, with the same percent change at its `compact_messages` call (7370). Change the skip message to `"[compact skipped: this session's model limits are unknown and it has no contextWindowTokens]"`.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p rupu-cli && cargo test -p rupu-orchestrator`
Expected: PASS, including `run_model_limits`.

- [ ] **Step 8: Commit**

```bash
git diff --name-only | grep '\.rs$' | xargs rustfmt --edition 2021
git add -u crates
git add crates/rupu-cli/tests/run_model_limits.rs
git commit -m "feat(cli,orchestrator): resolve model limits at every launch site; sessions persist them"
```

---

### Task 12: `rupu models` becomes a thin wrapper over the runtime

**Files:**
- Modify: `crates/rupu-cli/src/cmd/models.rs`
- Modify: `crates/rupu-cli/tests/models_subcommand.rs`

**Interfaces:**
- Consumes: `rupu_runtime::model_limits::{catalog, refresh, provider_names, resolve_targets}` (Task 9) and `rupu_providers::model_limits::fmt_age` (Task 1).

- [ ] **Step 1: Write the failing CLI tests** in `models_subcommand.rs`, using its existing `write_cfg` (48) and `models_cmd` (53) helpers. Replace `models_refresh_openai_compatible_account_says_it_has_no_live_endpoint` (180) with:

```rust
#[test]
fn models_refresh_openai_compatible_account_fetches_its_v1_models() {
    let server = httpmock::MockServer::start();
    let m = server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({ "data": [{ "id": "box-model", "max_model_len": 65536 }] }));
    });
    let home = tempfile::tempdir().unwrap();
    write_cfg(home.path(), &format!(
        "[providers.boxy]\nkind = \"openai-compatible\"\nbase_url = \"{}\"\n",
        server.url("")
    ));
    models_cmd(home.path()).args(["refresh", "--provider", "boxy"]).assert().success()
        .stdout(predicates::str::contains("refreshed boxy (1 models)"));
    m.assert();
    models_cmd(home.path()).args(["list", "--provider", "boxy"]).assert().success()
        .stdout(predicates::str::contains("65536").and(predicates::str::contains("live")));
}

#[test]
fn models_list_shows_output_and_fetched_columns() {
    let home = tempfile::tempdir().unwrap();
    models_cmd(home.path()).args(["list"]).assert().success()
        .stdout(predicates::str::contains("OUTPUT").and(predicates::str::contains("FETCHED")));
}
```

Check `write_cfg`'s and `models_cmd`'s real signatures (the `models_cmd` helper sets `RUPU_HOME` / `RUPU_CACHE_DIR_OVERRIDE`) and whether an API key is needed for an openai-compatible account (look at `models_list_surfaces_a_declared_openai_compatible_accounts_models` at 159). Copy the credential setup it uses. Use `predicates::prelude::*` if the file already imports it.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli --test models_subcommand`
Expected: the two new tests FAIL.

- [ ] **Step 3: Rewrite `models.rs` as a thin wrapper.**

Delete the logic that moved to the runtime (Task 9):
- `PROVIDERS`, `provider_names`, `LiveList` and `live_list_target`
- `resolve_targets` and `kind_of`
- `populate_live`, `build_registry` and `make_model_info`
- the unit tests that exercised them (models.rs:548-686)

Two of those tests move to `crates/rupu-runtime/tests/model_limits.rs` with their assertions intact:
- `provider_names_appends_declared_accounts_after_the_builtins`
- `an_unresolvable_name_is_an_error_naming_both_remedies`

The `live_list_target` tests go away with the enum; the factory's kind dispatch is covered by `provider_factory`'s `dispatch_by_kind_tests`.

Keep `Action`, `handle`, `ensure_output_format`, `global_config_path` and `load_global_config`. Then:

```rust
#[derive(Serialize)]
struct ModelListRow {
    provider: String,
    model: String,
    source: String,
    context: Option<u64>,
    output: Option<u64>,
    fetched_at: Option<String>,
}

#[derive(Serialize)]
struct ModelListCsvRow {
    provider: String,
    model: String,
    source: String,
    context: String,
    output: String,
    fetched_at: String,
}

async fn list(filter: Option<String>, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let cfg = load_global_config()?;
    let global = crate::paths::global_dir()?;
    let cats = rupu_runtime::model_limits::catalog(&cfg, &global, &global_config_path()?, filter.as_deref()).await?;
    let mut rows = Vec::new();
    for p in cats {
        for m in p.models {
            let fetched_at = if m.source == "live" { p.fetched_at } else { None };
            rows.push(ModelListRow {
                provider: p.provider.clone(),
                model: m.id,
                source: m.source,
                context: m.input_tokens.map(u64::from),
                output: m.output_tokens.map(u64::from),
                fetched_at: fetched_at.map(|t| t.to_rfc3339()),
            });
        }
    }
    let csv_rows: Vec<ModelListCsvRow> = rows
        .iter()
        .map(|row| ModelListCsvRow {
            provider: row.provider.clone(),
            model: row.model.clone(),
            source: row.source.clone(),
            context: row.context.map(|v| v.to_string()).unwrap_or_default(),
            output: row.output.map(|v| v.to_string()).unwrap_or_default(),
            fetched_at: row.fetched_at.clone().unwrap_or_default(),
        })
        .collect();
    let output = ModelListOutput {
        report: ModelListReport { kind: "model_list", version: 2, rows },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}
```

In `impl CollectionOutput for ModelListOutput` (models.rs:201-239):
- CSV headers: `["provider", "model", "source", "context", "output", "fetched_at"]`
- table header: `["PROVIDER", "MODEL", "SOURCE", "CONTEXT", "OUTPUT", "FETCHED"]`
- `None` limits render `"-"`, as `context` already does
- the FETCHED cell parses `row.fetched_at` with `chrono::DateTime::parse_from_rfc3339` and renders `rupu_providers::model_limits::fmt_age(chrono::Utc::now() - t.with_timezone(&chrono::Utc))`, or `"-"` when absent

`rupu-cli` already depends on `chrono` (check `crates/rupu-cli/Cargo.toml`; add `chrono.workspace = true` if not).

```rust
async fn refresh(filter: Option<String>) -> anyhow::Result<()> {
    let cfg = load_global_config()?;
    let global = crate::paths::global_dir()?;
    let resolver = crate::accounts::resolver_for(&cfg);
    let outcomes = rupu_runtime::model_limits::refresh(
        &cfg, &global, &global_config_path()?, &resolver, filter.as_deref(),
    )
    .await?;
    for o in outcomes {
        match (o.ok, o.count, o.error) {
            (true, 0, _) => eprintln!(
                "rupu: refreshed {} (0 models — re-run with `RUST_LOG=warn` to see why)",
                o.provider
            ),
            (true, n, _) => println!("rupu: refreshed {} ({n} models)", o.provider),
            (false, _, Some(e)) => eprintln!("rupu: skip {}: {e}", o.provider),
            (false, _, None) => eprintln!("rupu: skip {}", o.provider),
        }
    }
    Ok(())
}
```

`UnknownProvider` converts into `anyhow::Error` through `?`, so the unknown-provider tests (130 and 143) keep their non-zero exit and message.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rupu-cli --test models_subcommand && cargo test -p rupu-cli models && cargo test -p rupu-runtime`
Expected: PASS. `models_refresh_named_account_reaches_its_vendor_with_its_own_credential` (84) still passes: the factory honours `RUPU_ANTHROPIC_BASE_URL_OVERRIDE`, and `fetch_models` asks for `/v1/models?limit=1000`, which matches `when.path("/v1/models")`.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/models.rs crates/rupu-cli/tests/models_subcommand.rs crates/rupu-runtime/tests/model_limits.rs
git add -u crates
git commit -m "refactor(cli): rupu models is a thin wrapper over rupu_runtime::model_limits; list shows output + fetched"
```

---

### Task 13: CP `ModelCatalog` port and endpoints

**Files:**
- Create: `crates/rupu-cp/src/model_catalog.rs`
- Create: `crates/rupu-cp/src/api/models.rs`
- Modify:
  - `crates/rupu-cp/src/lib.rs` (`pub mod model_catalog;` near `pub mod repos;` at 19; a `ServeOpts` field (39-80); `serve_on` builder chain (238-248))
  - `crates/rupu-cp/src/state.rs` (field + `AppState::new` default + `with_model_catalog`)
  - `crates/rupu-cp/src/api/mod.rs` (`pub mod models;`)
  - `crates/rupu-cp/src/server.rs:64-93` (`.merge(crate::api::models::routes())`)
- Create: `crates/rupu-cli/src/cp_model_catalog.rs`; modify `crates/rupu-cli/src/lib.rs` (`pub mod cp_model_catalog;` next to `cp_repos`, line 15) and `crates/rupu-cli/src/cmd/cp.rs` (build near 356-361; pass at 440-456)

**Interfaces:**
- Consumes: `rupu_runtime::model_limits::{catalog, refresh, CatalogProvider, RefreshOutcome}`.
- Produces:
  - trait `rupu_cp::model_catalog::ModelCatalog { list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError>; refresh(&self, provider: Option<String>) -> Result<Vec<RefreshOutcome>, ModelCatalogError> }`
  - `ModelCatalogError { UnknownProvider(String), Backend(String) }`
  - `GET /api/models`
  - `POST /api/models/refresh` (body `{ provider?: string }`)

- [ ] **Step 1: Write the failing handler tests** at the bottom of the new `crates/rupu-cp/src/api/models.rs`. They follow `api/repos.rs:29-63`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::{ModelCatalog, ModelCatalogError};
    use rupu_runtime::model_limits::{CatalogModel, CatalogProvider, RefreshOutcome};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Fake {
        asked: Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ModelCatalog for Fake {
        async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError> {
            Ok(vec![CatalogProvider {
                provider: "anthropic".into(),
                fetched_at: None,
                stale: false,
                models: vec![CatalogModel { id: "claude-a".into(), input_tokens: Some(1000), output_tokens: None, source: "live".into() }],
            }])
        }
        async fn refresh(&self, provider: Option<String>) -> Result<Vec<RefreshOutcome>, ModelCatalogError> {
            if provider.as_deref() == Some("nope") {
                return Err(ModelCatalogError::UnknownProvider("unknown provider 'nope'".into()));
            }
            self.asked.lock().unwrap().push(provider.clone());
            Ok(vec![RefreshOutcome { provider: provider.unwrap_or_else(|| "anthropic".into()), ok: true, count: 1, error: None }])
        }
    }

    #[tokio::test]
    async fn lists_from_port() {
        let port: Arc<dyn ModelCatalog> = Arc::new(Fake::default());
        let out = list_with(Some(port)).await.unwrap();
        assert_eq!(out[0].models[0].input_tokens, Some(1000));
    }

    #[tokio::test]
    async fn refresh_passes_the_provider_through() {
        let fake = Arc::new(Fake::default());
        let port: Arc<dyn ModelCatalog> = fake.clone();
        let out = refresh_with(Some(port), RefreshBody { provider: Some("anthropic".into()) }).await.unwrap();
        assert!(out[0].ok);
        assert_eq!(fake.asked.lock().unwrap().as_slice(), [Some("anthropic".to_string())]);
    }

    #[tokio::test]
    async fn unknown_provider_is_400() {
        let port: Arc<dyn ModelCatalog> = Arc::new(Fake::default());
        let err = refresh_with(Some(port), RefreshBody { provider: Some("nope".into()) }).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_port_is_501() {
        assert_eq!(list_with(None).await.unwrap_err().0, axum::http::StatusCode::NOT_IMPLEMENTED);
        assert_eq!(refresh_with(None, RefreshBody::default()).await.unwrap_err().0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cp api::models`
Expected: compile error.

- [ ] **Step 3: Implement the port** in `crates/rupu-cp/src/model_catalog.rs`:

```rust
//! `ModelCatalog` port — the discovered model-limits catalog + manual
//! refetch (spec 2026-09-30 model-limits discovery §8.2). rupu-cp defines
//! it; rupu-cli's `cp serve` provides the runtime-backed adapter.
pub use rupu_runtime::model_limits::{CatalogModel, CatalogProvider, RefreshOutcome};

#[derive(Debug, thiserror::Error)]
pub enum ModelCatalogError {
    #[error("{0}")]
    UnknownProvider(String),
    #[error("model catalog failed: {0}")]
    Backend(String),
}

#[async_trait::async_trait]
pub trait ModelCatalog: Send + Sync {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError>;
    async fn refresh(&self, provider: Option<String>) -> Result<Vec<RefreshOutcome>, ModelCatalogError>;
}
```

Implement the routes in `crates/rupu-cp/src/api/models.rs`, above the tests:

```rust
use crate::{
    error::{ApiError, ApiResult},
    model_catalog::{CatalogProvider, ModelCatalog, ModelCatalogError, RefreshOutcome},
    state::AppState,
};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/models", get(list_models))
        .route("/api/models/refresh", post(refresh_models))
}

#[derive(Debug, Default, Deserialize)]
pub struct RefreshBody {
    #[serde(default)]
    pub provider: Option<String>,
}

fn map_err(e: ModelCatalogError) -> ApiError {
    match e {
        ModelCatalogError::UnknownProvider(m) => ApiError::bad_request(m),
        ModelCatalogError::Backend(m) => ApiError::internal(m),
    }
}

fn port(p: Option<Arc<dyn ModelCatalog>>) -> ApiResult<Arc<dyn ModelCatalog>> {
    p.ok_or_else(|| ApiError::not_available("the model catalog requires `rupu cp serve`"))
}

async fn list_with(p: Option<Arc<dyn ModelCatalog>>) -> ApiResult<Vec<CatalogProvider>> {
    port(p)?.list().await.map_err(map_err)
}

/// Synchronous: providers are fetched in parallel, each bounded by the
/// runtime's 10s timeout, so a 200 means refreshed, not merely recorded.
async fn refresh_with(p: Option<Arc<dyn ModelCatalog>>, body: RefreshBody) -> ApiResult<Vec<RefreshOutcome>> {
    port(p)?.refresh(body.provider).await.map_err(map_err)
}

async fn list_models(State(s): State<AppState>) -> ApiResult<Json<Vec<CatalogProvider>>> {
    Ok(Json(list_with(s.model_catalog.clone()).await?))
}

async fn refresh_models(
    State(s): State<AppState>,
    body: Option<Json<RefreshBody>>,
) -> ApiResult<Json<Vec<RefreshOutcome>>> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(refresh_with(s.model_catalog.clone(), body).await?))
}
```

Check that `ApiError::bad_request` and `ApiError::internal` take `impl Into<String>` (error.rs:33-40), and that an `Option<Json<T>>` extractor is accepted by the crate's axum version. If it isn't, take `Json<RefreshBody>` and have the web always send `{}`.

Wire the state:
- `state.rs`: add `pub model_catalog: Option<Arc<dyn crate::model_catalog::ModelCatalog>>,` with a doc comment matching `repos`, set `model_catalog: None,` in `AppState::new`, and add `pub fn with_model_catalog(mut self, c: Option<Arc<dyn crate::model_catalog::ModelCatalog>>) -> Self { self.model_catalog = c; self }`.
- `lib.rs`:
  - add `pub mod model_catalog;`
  - add to `ServeOpts` the field `pub model_catalog: Option<std::sync::Arc<dyn crate::model_catalog::ModelCatalog>>,`, documented "`None` → `/api/models*` return 501"
  - add `.with_model_catalog(opts.model_catalog.clone())` to the builder chain (238-248)
- `api/mod.rs`: add `pub mod models;`.
- `server.rs`: add `.merge(crate::api::models::routes())` next to `repos`.

The whole `/api` router already sits behind `require_bearer` when a token is set.

- [ ] **Step 4: The `cp serve` adapter** in `crates/rupu-cli/src/cp_model_catalog.rs`:

```rust
//! `cp serve`'s `ModelCatalog` adapter: delegates to `rupu_runtime::model_limits`.
use rupu_cp::model_catalog::{CatalogProvider, ModelCatalog, ModelCatalogError, RefreshOutcome};
use std::path::PathBuf;

pub struct RuntimeModelCatalog {
    pub global_dir: PathBuf,
}

impl RuntimeModelCatalog {
    fn config(&self) -> Result<(rupu_config::Config, PathBuf), ModelCatalogError> {
        let cfg_path = self.global_dir.join("config.toml");
        let cfg = rupu_config::layer_files_locked(Some(&cfg_path), None)
            .map_err(|e| ModelCatalogError::Backend(e.to_string()))?;
        Ok((cfg, cfg_path))
    }
}

#[async_trait::async_trait]
impl ModelCatalog for RuntimeModelCatalog {
    async fn list(&self) -> Result<Vec<CatalogProvider>, ModelCatalogError> {
        let (cfg, cfg_path) = self.config()?;
        rupu_runtime::model_limits::catalog(&cfg, &self.global_dir, &cfg_path, None)
            .await
            .map_err(|e| ModelCatalogError::UnknownProvider(e.to_string()))
    }

    async fn refresh(&self, provider: Option<String>) -> Result<Vec<RefreshOutcome>, ModelCatalogError> {
        let (cfg, cfg_path) = self.config()?;
        let resolver = crate::accounts::resolver_for(&cfg);
        rupu_runtime::model_limits::refresh(&cfg, &self.global_dir, &cfg_path, &resolver, provider.as_deref())
            .await
            .map_err(|e| ModelCatalogError::UnknownProvider(e.to_string()))
    }
}
```

Add `pub mod cp_model_catalog;` to `crates/rupu-cli/src/lib.rs`. In `cmd/cp.rs`, next to the generator (356-361):

```rust
            let model_catalog: Option<Arc<dyn rupu_cp::model_catalog::ModelCatalog>> =
                Some(Arc::new(crate::cp_model_catalog::RuntimeModelCatalog {
                    global_dir: global_dir.clone(),
                }));
```

Pass `model_catalog,` in the `rupu_cp::ServeOpts { … }` literal (442-456).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p rupu-cp && cargo check -p rupu-cli`
Expected: PASS and clean.

- [ ] **Step 6: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cp/src/model_catalog.rs crates/rupu-cp/src/api/models.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/src/state.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs crates/rupu-cli/src/cp_model_catalog.rs crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/cp.rs
git add crates/rupu-cp/src crates/rupu-cli/src
git commit -m "feat(cp): ModelCatalog port + GET /api/models + POST /api/models/refresh (cp serve adapter)"
```

---

### Task 14: CP web — Settings → Models tab

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (types near `RepoEntry` at ~1773; methods in `export const api = {` at 1985, near `getRepos` at 2890)
- Create: `crates/rupu-cp/web/src/components/settings/ModelsTab.tsx`
- Create: `crates/rupu-cp/web/src/components/settings/ModelsTab.test.tsx`
- Modify: `crates/rupu-cp/web/src/pages/Settings.tsx` (`SettingsTab` 63-72, icon imports 32-46, tab bar 468-486, body 488-575)
- Modify: `crates/rupu-cp/web/src/pages/Settings.test.tsx` (one tab-presence test)

**Interfaces:**
- Consumes: `GET /api/models` and `POST /api/models/refresh` (Task 13).
- Produces: `api.getModelCatalog(): Promise<CatalogProvider[]>`, `api.refreshModels(provider?: string): Promise<RefreshOutcome[]>` and `<ModelsTab />`.

- [ ] **Step 1: Add the API types and methods** to `api.ts`. The name `ProviderModels` is already taken (lines 1756-1760), so these types use the `Catalog*` names.

```ts
// --- Model catalog (spec 2026-09-30 model-limits discovery §8) ---

/** One model from `GET /api/models`. A limit is null when unknown. */
export interface CatalogModel {
  id: string;
  input_tokens: number | null;
  output_tokens: number | null;
  source: 'live' | 'custom' | 'baked-in';
}

/** One provider block from `GET /api/models`. */
export interface CatalogProvider {
  provider: string;
  fetched_at: string | null;
  stale: boolean;
  models: CatalogModel[];
}

/** One provider's result from `POST /api/models/refresh`. */
export interface RefreshOutcome {
  provider: string;
  ok: boolean;
  count: number;
  error?: string;
}
```

Inside `export const api = {`:

```ts
  getModelCatalog(): Promise<CatalogProvider[]> {
    return request<CatalogProvider[]>('/api/models');
  },
  refreshModels(provider?: string): Promise<RefreshOutcome[]> {
    return request<RefreshOutcome[]>('/api/models/refresh', {
      method: 'POST',
      body: JSON.stringify(provider ? { provider } : {}),
    });
  },
```

- [ ] **Step 2: Write the failing component tests** in `ModelsTab.test.tsx`:

```tsx
// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api, ApiError, type CatalogProvider } from '../../lib/api';
import { ModelsTab } from './ModelsTab';

const CATALOG: CatalogProvider[] = [
  {
    provider: 'anthropic',
    fetched_at: new Date(Date.now() - 12 * 60_000).toISOString(),
    stale: false,
    models: [{ id: 'claude-demo-1', input_tokens: 1_000_000, output_tokens: 128_000, source: 'live' }],
  },
  { provider: 'gemini', fetched_at: null, stale: false, models: [] },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ModelsTab', () => {
  it('renders each provider with its limits and freshness', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    render(<ModelsTab />);
    expect(await screen.findByText('claude-demo-1')).toBeInTheDocument();
    expect(screen.getByText('1,000,000')).toBeInTheDocument();
    expect(screen.getByText('128,000')).toBeInTheDocument();
    expect(screen.getByText(/fetched 12m ago/)).toBeInTheDocument();
    expect(screen.getByText(/never fetched/)).toBeInTheDocument();
  });

  it('Refetch on a provider refreshes just that provider and reloads', async () => {
    const list = vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    const refresh = vi.spyOn(api, 'refreshModels').mockResolvedValue([{ provider: 'anthropic', ok: true, count: 1 }]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch anthropic' }));
    await waitFor(() => expect(refresh).toHaveBeenCalledWith('anthropic'));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  });

  it('Refetch all refreshes every provider', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    const refresh = vi.spyOn(api, 'refreshModels').mockResolvedValue([]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch all' }));
    await waitFor(() => expect(refresh).toHaveBeenCalledWith(undefined));
  });

  it('shows a provider refresh error inline', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    vi.spyOn(api, 'refreshModels').mockResolvedValue([
      { provider: 'gemini', ok: false, count: 0, error: 'no live model-list endpoint' },
    ]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch gemini' }));
    expect(await screen.findByText(/gemini: no live model-list endpoint/)).toBeInTheDocument();
  });

  it('explains when the CP is not cp serve', async () => {
    vi.spyOn(api, 'getModelCatalog').mockRejectedValue(new ApiError(501, 'the model catalog requires `rupu cp serve`'));
    render(<ModelsTab />);
    expect(await screen.findByText(/requires `rupu cp serve`/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/settings/ModelsTab.test.tsx`
Expected: FAIL (the module doesn't exist).

- [ ] **Step 4: Implement `ModelsTab.tsx`:**

```tsx
// Settings → Models: the discovered model-limits catalog plus a manual
// refetch (spec docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md §8.3).
import { useCallback, useEffect, useState } from 'react';
import { api, ApiError, apiErrorMessage, type CatalogModel, type CatalogProvider } from '../../lib/api';
import { relativeTime } from '../../lib/time';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import SortableTable, { type Column } from '../lists/SortableTable';
import { EmptyTabState } from '../ConfigEditor';

const fmt = (n: number | null) => (n == null ? '—' : n.toLocaleString('en-US'));

const COLUMNS: Column<CatalogModel>[] = [
  { key: 'id', header: 'Model', subject: true, sortable: true, sortValue: (m) => m.id, render: (m) => <span className="font-mono">{m.id}</span> },
  { key: 'input', header: 'Input limit', align: 'right', sortable: true, sortValue: (m) => m.input_tokens, render: (m) => fmt(m.input_tokens) },
  { key: 'output', header: 'Output cap', align: 'right', sortable: true, sortValue: (m) => m.output_tokens, render: (m) => fmt(m.output_tokens) },
  { key: 'source', header: 'Source', render: (m) => m.source },
];

export function ModelsTab() {
  const [catalog, setCatalog] = useState<CatalogProvider[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});

  const load = useCallback(async () => {
    try {
      setCatalog(await api.getModelCatalog());
      setLoadError(null);
    } catch (e) {
      if (e instanceof ApiError && e.status === 501) setUnavailable(true);
      else setLoadError(apiErrorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const refetch = async (provider?: string) => {
    setBusy(provider ?? '*');
    try {
      const outcomes = await api.refreshModels(provider);
      setErrors((prev) => {
        const next = { ...prev };
        for (const o of outcomes) {
          if (o.ok) delete next[o.provider];
          else next[o.provider] = o.error ?? 'refresh failed';
        }
        return next;
      });
      await load();
    } catch (e) {
      setLoadError(apiErrorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  if (unavailable) return <EmptyTabState text="The model catalog requires `rupu cp serve`." />;
  if (!catalog) return loadError ? <ErrorBanner>{loadError}</ErrorBanner> : <Spinner label="Loading models…" />;

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <p className="text-sm">
          Limits come from each provider's model list, cached for an hour. Agent frontmatter and
          <code className="mx-1">[[providers.&lt;name&gt;.models]]</code>
          override them.
        </p>
        <Button variant="secondary" size="sm" disabled={busy !== null} onClick={() => void refetch()} aria-label="Refetch all">
          Refetch all
        </Button>
      </div>
      {loadError && <ErrorBanner>{loadError}</ErrorBanner>}
      {catalog.map((p) => (
        <section key={p.provider} className="space-y-2">
          <div className="flex items-center gap-3">
            <h3 className="font-medium">{p.provider}</h3>
            <span className="text-sm">{p.fetched_at ? `fetched ${relativeTime(p.fetched_at)}` : 'never fetched'}</span>
            {p.stale && <Badge tone="amber">stale</Badge>}
            <div className="ml-auto">
              <Button
                variant="ghost"
                size="sm"
                disabled={busy !== null}
                onClick={() => void refetch(p.provider)}
                aria-label={`Refetch ${p.provider}`}
              >
                {busy === p.provider ? 'Refetching…' : 'Refetch'}
              </Button>
            </div>
          </div>
          {errors[p.provider] && <ErrorBanner>{`${p.provider}: ${errors[p.provider]}`}</ErrorBanner>}
          {p.models.length > 0 ? (
            <SortableTable<CatalogModel>
              columns={COLUMNS}
              rows={p.models}
              rowKey={(m) => m.id}
              initialSort={{ key: 'id', dir: 'asc' }}
            />
          ) : (
            <p className="text-sm">No models listed.</p>
          )}
        </section>
      ))}
    </div>
  );
}
```

Check `SortableTable`'s `Column` (SortableTable.tsx:16-57) for which props are required. If `sortValue` must return `string | number | null`, `(m) => m.input_tokens` already fits. Check that `relativeTime` returns `"12m ago"` (time.ts:7), which makes the rendered text `fetched 12m ago`.

- [ ] **Step 5: Add the tab to Settings.**

In `Settings.tsx`:
- add `| 'models'` to `SettingsTab`, after `'providers'`
- import `Layers` from `lucide-react` alongside the existing icons, and `import { ModelsTab } from '../components/settings/ModelsTab';`
- after the Providers `TabButton` (471): `<TabButton active={tab === 'models'} onClick={() => setTab('models')} icon={Layers} label="Models" />`
- in the body `<section>`: `{tab === 'models' && <ModelsTab />}`

Add this test to `Settings.test.tsx`, which mocks `api.getConfig` as every test there does:

```tsx
  it('Models tab loads the model catalog', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(MOCK_CONFIG);
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue([]);
    render(
      <MemoryRouter initialEntries={['/settings']}>
        <Settings />
      </MemoryRouter>,
    );
    await screen.findByLabelText('Default model');
    fireEvent.click(screen.getByRole('button', { name: 'Models' }));
    expect(await screen.findByRole('button', { name: 'Refetch all' })).toBeInTheDocument();
  });
```

- [ ] **Step 6: Run the tests and the type-check**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/settings/ModelsTab.test.tsx src/pages/Settings.test.tsx && npx tsc -b`
Expected: PASS and no type errors.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/components/settings/ModelsTab.tsx crates/rupu-cp/web/src/components/settings/ModelsTab.test.tsx crates/rupu-cp/web/src/pages/Settings.tsx crates/rupu-cp/web/src/pages/Settings.test.tsx
git commit -m "feat(cp-web): Settings → Models tab with per-provider and all-providers refetch"
```

---

### Task 15: Docs

**Files:**
- Modify: `docs/agent-format.md` (rows 76-78; sections 288-294)
- Modify: `docs/providers.md` (149; 183-186; 243-267)
- Modify: `docs/providers/gemini.md` (the model-listing mention)
- Modify: `CLAUDE.md` (`### Crates`: insert a `rupu-runtime` bullet after line 51, `rupu-cli`)

- [ ] **Step 1: `docs/agent-format.md`.**

Frontmatter table:

```
| `maxTokens` | integer | no | discovered output cap | Per-request output-token cap. Overrides the cap discovered from the provider's model list; when neither is known, Anthropic gets `8192` and other providers get no cap (model max). Extended thinking (`effort`) draws from this budget |
| `contextWindowTokens` | integer | no | discovered input limit | Input-token limit used for proactive compaction. Overrides the discovered limit; compaction is off only when neither is known |
| `compactAtPercent` | integer | no | `80` | Percentage of the input limit at which compaction triggers; clamped to `[10, 95]`. Compaction also triggers early enough that a full-length reply still fits |
```

Replace the `### maxTokens` / `### contextWindowTokens and compactAtPercent` sections with prose saying the same things:
- limits are discovered from the provider's model list (cached 1h in `~/.rupu/cache/models/`);
- frontmatter pins override them;
- every run writes a `model_limits` notice to its transcript saying which values it used and where they came from;
- a provider "prompt too long" error that reports the real maximum lowers the limit for the rest of the run and triggers compaction.

- [ ] **Step 2: `docs/providers.md`.**

- **Line 149 (`[[providers.<name>.models]]`):** these entries also override discovered limits, field by field.
- **Lines 183-186:** replace "(defaults 32768 / 8192 when omitted)" with: "When omitted, rupu reads `max_model_len` from the server's `/v1/models`; if that's missing too, the limits are unknown and the run says so."
- **Model resolution (243-267):**
  - add a paragraph listing which endpoint each provider's limits come from (the §3 table);
  - say that Gemini's CLI/Code Assist login exposes none and needs `[[providers.gemini.models]]`;
  - replace the "An `openai-compatible` account has no live listing endpoint" paragraph: `rupu models refresh` now fetches its `/v1/models`;
  - mention the CP Settings → Models tab and its Refetch buttons;
  - fix line 247's claim "lazily on first `rupu models list`": `list` reads the cache only, and runs refresh it when it's stale.

- [ ] **Step 3: `docs/providers/gemini.md`:** state that AI Studio (API key) reports limits through `GET /v1beta/models`, and that the Gemini CLI OAuth path has no listing.

- [ ] **Step 4: `CLAUDE.md`.** Insert after the `rupu-cli` bullet (line 51):

```
- **`rupu-runtime`** — run-assembly layer shared by the CLI, orchestrator and CP: `provider_factory` (build a `Box<dyn LlmProvider>` from config + credentials) and `model_limits` (spec `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md`: `resolve` gives every run its model's input limit + output cap — agent frontmatter → `[[providers.X.models]]` → the provider's live model list (1h v2 cache at `<RUPU_HOME>/cache/models/<provider>.json`) → unknown, with provenance, announced as a `model_limits` transcript notice; `refresh`/`catalog` back `rupu models` and the CP's `ModelCatalog` port, `GET /api/models` + `POST /api/models/refresh`, Settings → Models tab).
```

Also add to the "Read first" list:

```
- Model-limits discovery spec + plan: `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md`, `docs/superpowers/plans/2026-09-30-rupu-model-limits-discovery.md`
```

- [ ] **Step 5: Commit**

```bash
git add docs/agent-format.md docs/providers.md docs/providers/gemini.md CLAUDE.md
git commit -m "docs: model-limits discovery — agent overrides, provider sources, Models tab"
```

---

### Task 16: Final verification

- [ ] **Step 1: Lint.** Run `cargo clippy --workspace --all-targets -- -D warnings`. Expected: clean.
- [ ] **Step 2: Format check on touched files only.** Run `git diff --name-only origin/main...HEAD | grep '\.rs$' | xargs rustfmt --edition 2021 --check`. Expected: no diff. **Do not** run `cargo fmt`.
- [ ] **Step 3: Tests.** Run `cargo test --workspace`. Expected: PASS, or only the failures recorded in the pre-Task-1 baseline.
- [ ] **Step 4: Web.** Run `cd crates/rupu-cp/web && npx vitest run && npm run build`. Expected: PASS, and the build succeeds.
- [ ] **Step 5: Spec coverage check.** Walk the spec §3-§8 and tick each item against a task; fix any gap before opening the PR.
- [ ] **Step 6: Owed before merge, with matt's approval (live network).**
  - On matt's machine, with his Anthropic OAuth login: `rupu models refresh --provider anthropic`, then `rupu models list --provider anthropic`. Confirm the OAuth path returns `max_input_tokens`/`max_tokens` and record what `claude-sonnet-4-6` reports; spec §10 lists this as unverified.
  - matt runs the CP Settings → Models tab (GUI check) before the UI merges.
- [ ] **Step 7: Open the PR.** Push the branch with an explicit refspec (`git push origin HEAD:refs/heads/<branch>`; never a bare `git push`). Open it with `gh pr create`; the body links the spec and plan, lists the two owed checks, and ends with the Claude Code attribution line.
