# CP Cost & Tokens — Plan 3a (Backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lift the price table + resolver into `rupu-config`, compute token+dollar cost server-side in `rupu-cp`, and surface it on the run / session / project / workflow responses plus a new `/api/usage` overview.

**Architecture:** A pure-code move of `rupu-cli/src/pricing.rs` → `rupu-config/src/pricing.rs` (both `rupu-cli` and `rupu-cp` already depend on `rupu-config`). A new `rupu-cp::usage` module composes `rupu_transcript::aggregate` + `rupu_config::pricing::lookup` + `ModelPricing::cost_usd` into serializable `UsageSummary`/`UsageBreakdownRow` DTOs. Existing handlers gain an additive `usage` field; one new `/api/usage` endpoint serves the Dashboard overview. Read-only; `rupu-cp` keeps **no** `rupu-cli` dependency.

**Tech Stack:** Rust 2021, axum, serde, `rupu-transcript` (`aggregate`, `UsageRow`, `TimeWindow`), `rupu-config` (`PricingConfig`, `ModelPricing::cost_usd`, `layer_files`), `rupu-orchestrator` (`RunStore`, `StepResultRecord`).

**Conventions (enforced — read before starting):**
- Work on branch `feat-cp-cost-tokens` (already created off `main`). NEVER touch `main`.
- `#![deny(clippy::all)]` workspace-wide, incl. `cargo clippy --all-targets`. `unsafe_code` forbidden.
- Libraries use `thiserror`; the CLI binary uses `anyhow`. The new `rupu-cp` code follows the crate's existing `anyhow`/`ApiError` patterns.
- NEVER run `cargo fmt` package-wide. If you must format, run `rustfmt` on the specific files you changed only.
- Stage ONLY the files you changed (`git add <specific paths>`, never `git add -A`) — the tree has untracked `.rupu/*` samples that must never be committed.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `crates/rupu-config/src/pricing.rs` (moved from `rupu-cli`) | `BUILTIN_PRICES` + `lookup` + helpers (pure) | 1 |
| `crates/rupu-config/src/lib.rs` | declare `pub mod pricing;` | 1 |
| `crates/rupu-cli/src/lib.rs` + 6 call sites | drop local `pricing`; use `rupu_config::pricing::*` | 2 |
| `crates/rupu-cp/src/lib.rs` | load real `PricingConfig` at startup | 3 |
| `crates/rupu-cp/src/usage.rs` (new) | cost DTOs + `summarize` / `summarize_run` / `rollup` / `breakdown` | 4, 5, 6 |
| `crates/rupu-cp/src/api/runs.rs` | `usage` on run list + detail | 7 |
| `crates/rupu-cp/src/api/sessions.rs` | session token fields + per-session `usage` | 8 |
| `crates/rupu-cp/src/api/projects.rs` | project `usage` rollup + per-row usage | 9 |
| `crates/rupu-cp/src/api/workflows.rs` | `usage` on workflow detail | 10 |
| `crates/rupu-cp/src/api/usage.rs` (new) + `mod.rs` + `server.rs` | `GET /api/usage` overview | 11 |

---

## Task 1: Lift `pricing.rs` into `rupu-config`

**Files:**
- Move: `crates/rupu-cli/src/pricing.rs` → `crates/rupu-config/src/pricing.rs`
- Modify: `crates/rupu-config/src/lib.rs:16` (add module decl after `pub mod pricing_config;`)

The file is pure (no `anyhow`); it currently does `use rupu_config::{ModelPricing, PricingConfig};`. Inside `rupu-config` that self-reference must become a `crate::` reference. The unit tests (13 of them) move with the file unchanged — they use `super::*` + `PricingConfig::default()`, which resolve correctly in the new home.

- [ ] **Step 1: Move the file with git**

```bash
git mv crates/rupu-cli/src/pricing.rs crates/rupu-config/src/pricing.rs
```

- [ ] **Step 2: Fix the import line in the moved file**

In `crates/rupu-config/src/pricing.rs`, change line 20 from:

```rust
use rupu_config::{ModelPricing, PricingConfig};
```

to:

```rust
use crate::{ModelPricing, PricingConfig};
```

(Both names are re-exported from `crate` root — see `lib.rs:28` `pub use pricing_config::{ModelPricing, PricingConfig};`.)

- [ ] **Step 3: Declare the module in `rupu-config/src/lib.rs`**

After line 16 (`pub mod pricing_config;`), add:

```rust
pub mod pricing;
```

- [ ] **Step 4: Build + test `rupu-config`**

Run: `cargo test -p rupu-config`
Expected: PASS — all 13 lifted pricing tests (`user_config_wins_over_builtin`, `falls_through_to_builtin_when_no_user_entry`, `provider_alias_canonicalized_for_builtin_lookup`, `dated_openai_model_resolves_to_base_price`, …) run green in their new home.

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-config --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-config/src/pricing.rs crates/rupu-config/src/lib.rs crates/rupu-cli/src/pricing.rs
git commit -m "refactor(config): lift pricing table + resolver from rupu-cli into rupu-config

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

(The `git mv` stages the deletion of `crates/rupu-cli/src/pricing.rs`; naming it in `git add` records that deletion.)

---

## Task 2: Repoint `rupu-cli` to `rupu_config::pricing`

**Files:**
- Modify: `crates/rupu-cli/src/lib.rs:13` (remove `pub mod pricing;`)
- Modify (call sites): `crates/rupu-cli/src/cmd/session.rs`, `crates/rupu-cli/src/cmd/autoflow.rs`, `crates/rupu-cli/src/cmd/workflow.rs`, `crates/rupu-cli/src/cmd/usage.rs`, `crates/rupu-cli/src/output/live_run.rs` (and any other `crate::pricing::` reference)

The CLI's heavier reporting layer (`usage_report.rs`, the `UsageDataset`/`UsageFact` machinery) is otherwise untouched — only the `pricing` import path changes.

- [ ] **Step 1: Remove the now-empty module declaration**

In `crates/rupu-cli/src/lib.rs`, delete line 13:

```rust
pub mod pricing;
```

- [ ] **Step 2: Find every reference**

Run: `grep -rn "crate::pricing::" crates/rupu-cli/src/`
Expected: references in `session.rs`, `autoflow.rs`, `workflow.rs` (×2), `usage.rs`, `output/live_run.rs`.

- [ ] **Step 3: Replace all references**

Replace every `crate::pricing::` with `rupu_config::pricing::` across those files. For example, in `crates/rupu-cli/src/cmd/usage.rs:1031`:

```rust
    crate::pricing::lookup(pricing, &fact.provider, &fact.model, &fact.agent)
```

becomes:

```rust
    rupu_config::pricing::lookup(pricing, &fact.provider, &fact.model, &fact.agent)
```

Apply the identical `crate::pricing::` → `rupu_config::pricing::` swap at `session.rs:5727`, `autoflow.rs:6984`, `workflow.rs:1752`, `workflow.rs:1804`, and `output/live_run.rs:456`. If any `crate::pricing::BUILTIN_PRICES` references exist, swap them the same way.

- [ ] **Step 4: Verify no stale references remain**

Run: `grep -rn "crate::pricing::" crates/rupu-cli/src/`
Expected: no matches.

- [ ] **Step 5: Build + test `rupu-cli`**

Run: `cargo test -p rupu-cli`
Expected: PASS — the CLI builds and its usage/session/workflow tests stay green with the lifted resolver.

- [ ] **Step 6: Clippy gate**

Run: `cargo clippy -p rupu-cli --all-targets`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/session.rs crates/rupu-cli/src/cmd/autoflow.rs crates/rupu-cli/src/cmd/workflow.rs crates/rupu-cli/src/cmd/usage.rs crates/rupu-cli/src/output/live_run.rs
git commit -m "refactor(cli): use rupu_config::pricing after the lift

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Load a real `PricingConfig` at CP startup

**Files:**
- Modify: `crates/rupu-cp/src/lib.rs` (`serve` + a new testable `load_pricing` helper)

Today `serve` passes `PricingConfig::default()` (empty), so even `lookup`'s builtin fallback is reachable — but a user's `[pricing]` overrides in `~/.rupu/config.toml` are ignored. Load them via `rupu_config::layer_files`. On any read/parse error, warn and fall back to `default()` (builtins still price the common models; the server must not fail to boot over a malformed optional `[pricing]`).

`layer_files(global: Option<&Path>, project: Option<&Path>) -> Result<Config, LayerError>` and `Config.pricing: PricingConfig` (confirmed in `rupu-config`).

- [ ] **Step 1: Write the failing test**

Add to the bottom of `crates/rupu-cp/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_pricing_empty_when_no_config_file() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-pricing-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pricing = load_pricing(&dir);
        assert!(pricing.models.is_empty());
        assert!(pricing.agents.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_pricing_reads_user_overrides() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-pricing-some-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[pricing.anthropic.\"claude-sonnet-4-6\"]\ninput_per_mtok = 99.0\noutput_per_mtok = 99.0\n",
        )
        .unwrap();
        let pricing = load_pricing(&dir);
        let p = rupu_config::pricing::lookup(&pricing, "anthropic", "claude-sonnet-4-6", "any").unwrap();
        assert_eq!(p.input_per_mtok, 99.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_pricing_falls_back_on_malformed_config() {
        let dir = std::env::temp_dir().join(format!("rupu-cp-pricing-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "this is not = valid = toml = [[[").unwrap();
        // Must not panic; returns empty default, builtins still resolve.
        let pricing = load_pricing(&dir);
        assert!(pricing.models.is_empty());
        let p = rupu_config::pricing::lookup(&pricing, "anthropic", "claude-sonnet-4-6", "any").unwrap();
        assert_eq!(p.input_per_mtok, 3.0); // builtin
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cp load_pricing`
Expected: FAIL — `load_pricing` not defined.

- [ ] **Step 3: Implement `load_pricing` and use it in `serve`**

In `crates/rupu-cp/src/lib.rs`, add the helper above `serve` and call it. Replace the body of `serve`'s first line accordingly:

```rust
use std::path::Path;

/// Load the user's `[pricing]` overrides from `<global_dir>/config.toml`.
///
/// Returns an empty `PricingConfig` when the file is absent, and falls back
/// to `default()` (with a warning) when it exists but cannot be read/parsed.
/// `rupu_config::pricing::lookup` falls back to the builtin price table, so
/// cost still resolves for common models even when this is empty.
fn load_pricing(global_dir: &Path) -> PricingConfig {
    let config_path = global_dir.join("config.toml");
    if !config_path.exists() {
        return PricingConfig::default();
    }
    match rupu_config::layer_files(Some(&config_path), None) {
        Ok(cfg) => cfg.pricing,
        Err(e) => {
            tracing::warn!(path = %config_path.display(), error = %e, "failed to load [pricing]; using builtin prices only");
            PricingConfig::default()
        }
    }
}
```

Then change the first line of `serve`:

```rust
    let pricing = load_pricing(&opts.global_dir);
    let app_state = state::AppState::new(opts.global_dir, pricing);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp load_pricing`
Expected: PASS (all three).

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/lib.rs
git commit -m "feat(cp): load real PricingConfig at startup (was empty default)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `usage` module — `UsageSummary` + `summarize`

**Files:**
- Create: `crates/rupu-cp/src/usage.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (add `pub mod usage;`)

`rupu_transcript::aggregate(paths, TimeWindow) -> Vec<UsageRow>` gives one row per `(provider, model, agent)` with `input_tokens`/`output_tokens`/`cached_tokens`/`runs` (all `u64`). `summarize` folds those rows into a single serializable summary, pricing each row via `rupu_config::pricing::lookup(..).cost_usd(..)`. **`priced` is false if any contributing row had no resolvable price**; `cost_usd` is the partial (priced-only) total, or `None` when every row was unpriced.

- [ ] **Step 1: Declare the module**

In `crates/rupu-cp/src/lib.rs`, add after `pub mod transcript_tail;`:

```rust
pub mod usage;
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-cp/src/usage.rs` with just the tests first:

```rust
//! Token + dollar-cost aggregation for the control plane.
//!
//! Pure read-side composition of `rupu_transcript::aggregate` (tokens per
//! provider/model/agent) and `rupu_config::pricing` (USD price lookup +
//! `ModelPricing::cost_usd`). Cost is an estimate: when a model has no
//! resolvable price we report tokens with `priced = false` and never
//! fabricate a dollar figure.

use rupu_config::PricingConfig;
use rupu_transcript::UsageRow;
use serde::Serialize;

/// Token + cost summary for a run, or any rollup of runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    /// `None` when no contributing row was priced. Otherwise the sum of the
    /// priced rows' cost (a partial total when `priced == false`).
    pub cost_usd: Option<f64>,
    /// `false` when at least one contributing model lacked a price.
    pub priced: bool,
    /// Distinct runs contributing (for rollups).
    pub runs: u64,
}

/// Fold token rows into a single summary, pricing each row.
pub fn summarize(rows: &[UsageRow], pricing: &PricingConfig) -> UsageSummary {
    let mut out = UsageSummary {
        priced: true,
        ..UsageSummary::default()
    };
    let mut any_priced = false;
    let mut cost_acc = 0.0_f64;
    for row in rows {
        out.input_tokens += row.input_tokens;
        out.output_tokens += row.output_tokens;
        out.cached_tokens += row.cached_tokens;
        out.runs += row.runs;
        match rupu_config::pricing::lookup(pricing, &row.provider, &row.model, &row.agent) {
            Some(price) => {
                any_priced = true;
                cost_acc += price.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens);
            }
            None => out.priced = false,
        }
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_priced { Some(cost_acc) } else { None };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(provider: &str, model: &str, input: u64, output: u64, cached: u64) -> UsageRow {
        UsageRow {
            provider: provider.into(),
            model: model.into(),
            agent: "a".into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
            runs: 1,
        }
    }

    #[test]
    fn summarize_prices_a_known_model() {
        // anthropic/claude-sonnet-4-6 builtin: in 3.0, out 15.0, cached 0.30 per Mtok.
        let pricing = PricingConfig::default();
        let s = summarize(&[row("anthropic", "claude-sonnet-4-6", 1_000_000, 1_000_000, 0)], &pricing);
        assert_eq!(s.input_tokens, 1_000_000);
        assert_eq!(s.output_tokens, 1_000_000);
        assert_eq!(s.total_tokens, 2_000_000);
        assert!(s.priced);
        // 1M*3.0 + 1M*15.0 = $18.00
        assert!((s.cost_usd.unwrap() - 18.0).abs() < 1e-9);
    }

    #[test]
    fn summarize_unpriced_model_yields_no_cost() {
        let pricing = PricingConfig::default();
        let s = summarize(&[row("internal-vllm", "llama-3-70b", 1000, 1000, 0)], &pricing);
        assert_eq!(s.total_tokens, 2000);
        assert!(!s.priced);
        assert_eq!(s.cost_usd, None);
    }

    #[test]
    fn summarize_mixed_is_partial() {
        let pricing = PricingConfig::default();
        let s = summarize(
            &[
                row("anthropic", "claude-sonnet-4-6", 1_000_000, 0, 0), // $3.00
                row("internal-vllm", "llama-3-70b", 1000, 1000, 0),     // unpriced
            ],
            &pricing,
        );
        assert!(!s.priced); // an unpriced row was present
        assert!((s.cost_usd.unwrap() - 3.0).abs() < 1e-9); // partial: priced rows only
        assert_eq!(s.total_tokens, 1_000_000 + 2000);
    }

    #[test]
    fn summarize_empty_is_zero_priced() {
        let s = summarize(&[], &PricingConfig::default());
        assert_eq!(s.total_tokens, 0);
        assert!(s.priced);
        assert_eq!(s.cost_usd, None);
    }
}
```

- [ ] **Step 3: Run the test to verify it fails, then passes**

Run: `cargo test -p rupu-cp usage::tests`
Expected: FAIL first only if you staged tests before the impl — here the impl is in the same file, so this step is the green run. Expected: PASS (all four). If `UsageRow` field names mismatch, fix to match `rupu_transcript::aggregate::UsageRow` (`provider`, `model`, `agent`, `input_tokens`, `output_tokens`, `cached_tokens`, `runs`).

- [ ] **Step 4: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/usage.rs crates/rupu-cp/src/lib.rs
git commit -m "feat(cp): usage module — UsageSummary + summarize (tokens→cost)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `usage` module — run transcript paths, `summarize_run`, `rollup`

**Files:**
- Modify: `crates/rupu-cp/src/usage.rs`

A run's transcripts come from its step results: each `StepResultRecord` has a `transcript_path: PathBuf`, and fan-out/panel sub-units add one per `ItemResultRecord` (`record.items[*].transcript_path`). `rollup` adds many `UsageSummary`s into one (token fields add; `priced` ANDs; `cost_usd` sums treating `None` as 0 but flipping `priced=false` when any input was unpriced).

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-cp/src/usage.rs`'s `tests` module:

```rust
    #[test]
    fn summarize_run_reads_a_transcript() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rupu-cp-usage-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tpath = dir.join("t.jsonl");
        let mut f = std::fs::File::create(&tpath).unwrap();
        // Minimal transcript: run_start (provider/model/agent) + one usage event.
        writeln!(f, r#"{{"type":"run_start","data":{{"run_id":"r1","workspace_id":"w","agent":"a","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-01-01T00:00:00Z","mode":"ask"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1000000,"output_tokens":0,"cached_tokens":0}}}}"#).unwrap();
        drop(f);

        let s = summarize_paths(&[tpath], &PricingConfig::default());
        assert_eq!(s.input_tokens, 1_000_000);
        assert!(s.priced);
        assert!((s.cost_usd.unwrap() - 3.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollup_sums_and_propagates_unpriced() {
        let priced = UsageSummary { input_tokens: 10, output_tokens: 5, cached_tokens: 0, total_tokens: 15, cost_usd: Some(2.0), priced: true, runs: 1 };
        let unpriced = UsageSummary { input_tokens: 20, output_tokens: 0, cached_tokens: 0, total_tokens: 20, cost_usd: None, priced: false, runs: 1 };
        let r = rollup([priced, unpriced].into_iter());
        assert_eq!(r.input_tokens, 30);
        assert_eq!(r.output_tokens, 5);
        assert_eq!(r.total_tokens, 35);
        assert_eq!(r.runs, 2);
        assert!(!r.priced); // unpriced input present
        assert!((r.cost_usd.unwrap() - 2.0).abs() < 1e-9); // None treated as 0
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp usage::tests`
Expected: FAIL — `summarize_paths` / `rollup` not defined.

- [ ] **Step 3: Implement**

Add to `crates/rupu-cp/src/usage.rs` (after `summarize`):

```rust
use rupu_orchestrator::runs::RunStore;
use rupu_transcript::TimeWindow;
use std::path::PathBuf;

/// Aggregate the given transcript files and summarize the result.
pub fn summarize_paths(paths: &[PathBuf], pricing: &PricingConfig) -> UsageSummary {
    let rows = rupu_transcript::aggregate(paths, TimeWindow::default());
    summarize(&rows, pricing)
}

/// All transcript paths a run produced: one per step result, plus one per
/// fan-out / panel sub-unit. Missing files are tolerated downstream by
/// `rupu_transcript::aggregate` (it skips unreadable paths).
pub fn run_transcript_paths(store: &RunStore, run_id: &str) -> Vec<PathBuf> {
    let records = store.read_step_results(run_id).unwrap_or_default();
    let mut paths = Vec::new();
    for record in &records {
        paths.push(record.transcript_path.clone());
        for item in &record.items {
            paths.push(item.transcript_path.clone());
        }
    }
    paths
}

/// Token + cost summary for a single run.
pub fn summarize_run(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> UsageSummary {
    summarize_paths(&run_transcript_paths(store, run_id), pricing)
}

/// Combine many summaries into one. Token fields add; `priced` ANDs across
/// inputs; `cost_usd` sums priced contributions (a `None` contributes 0 but
/// forces `priced = false`). `runs` sums.
pub fn rollup(summaries: impl Iterator<Item = UsageSummary>) -> UsageSummary {
    let mut out = UsageSummary {
        priced: true,
        ..UsageSummary::default()
    };
    let mut any_cost = false;
    let mut cost_acc = 0.0_f64;
    for s in summaries {
        out.input_tokens += s.input_tokens;
        out.output_tokens += s.output_tokens;
        out.cached_tokens += s.cached_tokens;
        out.runs += s.runs;
        match s.cost_usd {
            Some(c) => {
                any_cost = true;
                cost_acc += c;
            }
            None => {}
        }
        if !s.priced {
            out.priced = false;
        }
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_cost { Some(cost_acc) } else { None };
    out
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-cp usage::tests`
Expected: PASS (all six).

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings. (If clippy flags the `match … { Some => …, None => {} }`, rewrite as `if let Some(c) = s.cost_usd { any_cost = true; cost_acc += c; }`.)

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/usage.rs
git commit -m "feat(cp): usage — per-run transcript aggregation + rollup

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `usage` module — `breakdown` + `UsageBreakdownRow` + `GroupBy`

**Files:**
- Modify: `crates/rupu-cp/src/usage.rs`

The `/api/usage` overview needs a per-dimension breakdown. `breakdown` groups `UsageRow`s by provider, model, or agent, prices each group, and returns serializable rows sorted by total tokens descending.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module:

```rust
    #[test]
    fn breakdown_groups_by_model_and_prices() {
        let pricing = PricingConfig::default();
        let rows = vec![
            row("anthropic", "claude-sonnet-4-6", 1_000_000, 0, 0),
            row("anthropic", "claude-sonnet-4-6", 1_000_000, 0, 0),
            row("internal-vllm", "llama-3-70b", 5, 5, 0),
        ];
        let b = breakdown(&rows, &pricing, GroupBy::Model);
        // Two groups; sonnet first (more tokens).
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].model, "claude-sonnet-4-6");
        assert_eq!(b[0].input_tokens, 2_000_000);
        assert!(b[0].priced);
        assert!((b[0].cost_usd.unwrap() - 6.0).abs() < 1e-9); // 2M * 3.0
        assert_eq!(b[0].runs, 2);
        // llama group unpriced.
        assert_eq!(b[1].model, "llama-3-70b");
        assert!(!b[1].priced);
        assert_eq!(b[1].cost_usd, None);
    }

    #[test]
    fn breakdown_group_by_provider_merges_models() {
        let pricing = PricingConfig::default();
        let rows = vec![
            row("anthropic", "claude-sonnet-4-6", 1000, 0, 0),
            row("anthropic", "claude-haiku-4-5", 1000, 0, 0),
        ];
        let b = breakdown(&rows, &pricing, GroupBy::Provider);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].provider, "anthropic");
        assert_eq!(b[0].input_tokens, 2000);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp usage::tests`
Expected: FAIL — `breakdown` / `GroupBy` / `UsageBreakdownRow` not defined.

- [ ] **Step 3: Implement**

Add to `crates/rupu-cp/src/usage.rs`:

```rust
use std::collections::BTreeMap;

/// Dimension for the overview breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Provider,
    Model,
    Agent,
}

impl GroupBy {
    /// Parse the `group_by` query param; defaults to `Model` on anything else.
    pub fn parse(s: &str) -> Self {
        match s {
            "provider" => GroupBy::Provider,
            "agent" => GroupBy::Agent,
            _ => GroupBy::Model,
        }
    }
}

/// One grouped line for the overview breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageBreakdownRow {
    pub provider: String,
    pub model: String,
    pub agent: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub priced: bool,
    pub runs: u64,
}

/// Group token rows by the chosen dimension, price each group, and return
/// rows sorted by total tokens descending. The non-grouped identity fields
/// carry the first row's value (or an empty string) — the UI labels by the
/// grouped dimension.
pub fn breakdown(rows: &[UsageRow], pricing: &PricingConfig, group_by: GroupBy) -> Vec<UsageBreakdownRow> {
    // Accumulator keyed by the grouping string.
    let mut groups: BTreeMap<String, UsageBreakdownRow> = BTreeMap::new();
    for row in rows {
        let key = match group_by {
            GroupBy::Provider => row.provider.clone(),
            GroupBy::Model => row.model.clone(),
            GroupBy::Agent => row.agent.clone(),
        };
        let entry = groups.entry(key).or_insert_with(|| UsageBreakdownRow {
            provider: if group_by == GroupBy::Provider { row.provider.clone() } else { String::new() },
            model: if group_by == GroupBy::Model { row.model.clone() } else { String::new() },
            agent: if group_by == GroupBy::Agent { row.agent.clone() } else { String::new() },
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            total_tokens: 0,
            cost_usd: None,
            priced: true,
            runs: 0,
        });
        entry.input_tokens += row.input_tokens;
        entry.output_tokens += row.output_tokens;
        entry.cached_tokens += row.cached_tokens;
        entry.runs += row.runs;
        match rupu_config::pricing::lookup(pricing, &row.provider, &row.model, &row.agent) {
            Some(price) => {
                let c = price.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens);
                entry.cost_usd = Some(entry.cost_usd.unwrap_or(0.0) + c);
            }
            None => entry.priced = false,
        }
    }
    let mut out: Vec<UsageBreakdownRow> = groups
        .into_values()
        .map(|mut r| {
            r.total_tokens = r.input_tokens + r.output_tokens;
            r
        })
        .collect();
    out.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens).then_with(|| a.model.cmp(&b.model)));
    out
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-cp usage::tests`
Expected: PASS (all eight).

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/usage.rs
git commit -m "feat(cp): usage — grouped breakdown for the overview

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Attach `usage` to `/api/runs` rows and `/api/runs/:id`

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs`

Add `usage: UsageSummary` to `RunListRow` (computed per run from its transcripts) and add a `usage` field to the `/api/runs/:id` JSON.

- [ ] **Step 1: Write the failing test**

The handlers need a `RunStore` fixture, which is heavier than a unit test. Instead, assert the DTO carries the field by serializing it. Add to a `#[cfg(test)] mod tests` at the bottom of `runs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_list_row_serializes_usage() {
        let row = RunListRow {
            id: "r1".into(),
            workflow_name: "wf".into(),
            status: RunStatus::Completed,
            started_at: chrono::Utc::now(),
            finished_at: None,
            trigger: "manual",
            usage: crate::usage::UsageSummary::default(),
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("usage").is_some());
        assert_eq!(v["usage"]["priced"], serde_json::Value::Bool(false));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp run_list_row_serializes_usage`
Expected: FAIL — `RunListRow` has no `usage` field.

- [ ] **Step 3: Implement**

In `crates/rupu-cp/src/api/runs.rs`:

1. Add the field to the struct (it can no longer use the `From` impl alone, since it needs the store — see step below):

```rust
#[derive(serde::Serialize)]
pub(crate) struct RunListRow {
    pub(crate) id: String,
    pub(crate) workflow_name: String,
    pub(crate) status: RunStatus,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    pub(crate) finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) trigger: &'static str,
    pub(crate) usage: crate::usage::UsageSummary,
}
```

2. Keep the `From<&RunRecord>` impl but default the usage (so existing callers — e.g. `projects.rs` `recent_runs` — keep compiling), then add a constructor that fills usage from the store:

```rust
impl From<&RunRecord> for RunListRow {
    fn from(r: &RunRecord) -> Self {
        Self {
            id: r.id.clone(),
            workflow_name: r.workflow_name.clone(),
            status: r.status,
            started_at: r.started_at,
            finished_at: r.finished_at,
            trigger: trigger_of(r),
            usage: crate::usage::UsageSummary::default(),
        }
    }
}

impl RunListRow {
    /// Build a row with its usage summary filled from the run's transcripts.
    pub(crate) fn with_usage(
        r: &RunRecord,
        store: &rupu_orchestrator::runs::RunStore,
        pricing: &rupu_config::PricingConfig,
    ) -> Self {
        let mut row = Self::from(r);
        row.usage = crate::usage::summarize_run(store, &r.id, pricing);
        row
    }
}
```

3. Fill usage in the list handlers:

```rust
async fn list_runs(State(s): State<AppState>) -> ApiResult<Json<Vec<RunListRow>>> {
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(
        runs.iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
}
```

Apply the same `.map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))` change in `list_workflow_runs` (after the existing `.filter(...)`).

4. Add usage to `/api/runs/:id` — in `get_run`, after loading `steps`:

```rust
    let usage = crate::usage::summarize_run(&s.run_store, &id, &s.pricing);
    Ok(Json(serde_json::json!({ "run": record, "steps": steps, "usage": usage })))
```

- [ ] **Step 4: Run to verify it passes + build**

Run: `cargo test -p rupu-cp` then `cargo build -p rupu-cp`
Expected: PASS / builds. (`projects.rs`'s `recent_runs` uses `RunListRow::from` which now defaults usage — that's intentional; recent_runs are a preview and the per-row usage on the dedicated project-runs endpoint carries the real numbers in Task 9.)

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/api/runs.rs
git commit -m "feat(cp): attach usage (tokens+cost) to run list + detail

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Session token fields + per-session `usage`

**Files:**
- Modify: `crates/rupu-cp/src/api/sessions.rs`

A session's tokens live directly on `session.json` (`total_tokens_in`, `total_tokens_out`, `total_tokens_cached`, `provider_name`, `model`, `agent_name`) — no transcript reading needed. Add those to `SessionDto`, compute a `UsageSummary` from them via `pricing::lookup`, and inject it into the serialized session value (both list scan and detail).

- [ ] **Step 1: Write the failing test**

Add to a `#[cfg(test)] mod tests` at the bottom of `sessions.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_usage_from_dto_prices_known_model() {
        let dto = SessionDto {
            session_id: "s1".into(),
            agent_name: "a".into(),
            model: "claude-sonnet-4-6".into(),
            provider_name: "anthropic".into(),
            status: serde_json::Value::String("active".into()),
            total_turns: 3,
            total_tokens_in: 1_000_000,
            total_tokens_out: 0,
            total_tokens_cached: 0,
            created_at: String::new(),
            updated_at: String::new(),
            active_run_id: None,
            target: None,
            workspace_id: "w".into(),
        };
        let u = session_usage(&dto, &rupu_config::PricingConfig::default());
        assert_eq!(u.input_tokens, 1_000_000);
        assert!(u.priced);
        assert!((u.cost_usd.unwrap() - 3.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp session_usage_from_dto`
Expected: FAIL — `SessionDto` lacks the new fields and `session_usage` is undefined.

- [ ] **Step 3: Implement**

In `crates/rupu-cp/src/api/sessions.rs`:

1. Add the fields to `SessionDto` (all `#[serde(default)]`, matching the existing style):

```rust
    #[serde(default)]
    provider_name: String,
    #[serde(default)]
    total_tokens_in: u64,
    #[serde(default)]
    total_tokens_out: u64,
    #[serde(default)]
    total_tokens_cached: u64,
```

2. Add the pure helper:

```rust
/// Token + cost summary for a session, derived from its on-disk token totals
/// (sessions record their own totals; no transcript aggregation needed).
fn session_usage(
    dto: &SessionDto,
    pricing: &rupu_config::PricingConfig,
) -> crate::usage::UsageSummary {
    let total_tokens = dto.total_tokens_in + dto.total_tokens_out;
    let cost_usd =
        rupu_config::pricing::lookup(pricing, &dto.provider_name, &dto.model, &dto.agent_name)
            .map(|p| p.cost_usd(dto.total_tokens_in, dto.total_tokens_out, dto.total_tokens_cached));
    crate::usage::UsageSummary {
        input_tokens: dto.total_tokens_in,
        output_tokens: dto.total_tokens_out,
        cached_tokens: dto.total_tokens_cached,
        total_tokens,
        priced: cost_usd.is_some(),
        cost_usd,
        runs: 1,
    }
}
```

3. Inject `usage` into the serialized value. The two serialization sites are `scan_session_dir` (list) and `get_session` (detail). Both build `serde_json::to_value(dto)` then insert `scope`. Thread the pricing in and insert `usage` alongside `scope`. Change `scan_session_dir` and `collect_sessions` to take `pricing`:

```rust
fn scan_session_dir(
    root: &std::path::Path,
    scope: &str,
    pricing: &rupu_config::PricingConfig,
    out: &mut Vec<serde_json::Value>,
) {
    // ... unchanged until the `to_value` block ...
        if let Some(dto) = try_load_session(&dir) {
            let usage = session_usage(&dto, pricing);
            match serde_json::to_value(&dto) {
                Ok(mut val) => {
                    if let serde_json::Value::Object(ref mut map) = val {
                        map.insert("scope".to_string(), serde_json::Value::String(scope.to_string()));
                        if let Ok(u) = serde_json::to_value(&usage) {
                            map.insert("usage".to_string(), u);
                        }
                    }
                    out.push(val);
                }
                Err(e) => { /* unchanged warn */ }
            }
        }
}

pub(crate) fn collect_sessions(
    global_dir: &std::path::Path,
    pricing: &rupu_config::PricingConfig,
) -> Vec<serde_json::Value> {
    let mut sessions = Vec::new();
    scan_session_dir(&global_dir.join("sessions"), "active", pricing, &mut sessions);
    scan_session_dir(&global_dir.join("sessions-archive"), "archived", pricing, &mut sessions);
    sessions
}
```

Update the three `collect_sessions(&s.global_dir)` callers to `collect_sessions(&s.global_dir, &s.pricing)` — in `list_sessions` (this file), `projects.rs` (`get_project`, `project_sessions`), and `dashboard.rs` (`get_dashboard`). For `get_session`, after building `val`, insert usage:

```rust
    let usage = session_usage(&dto, &s.pricing);
    let mut val = serde_json::to_value(&dto).map_err(|e| ApiError::internal(e.to_string()))?;
    if let serde_json::Value::Object(ref mut map) = val {
        map.insert("scope".to_string(), serde_json::Value::String(scope.to_string()));
        if let Ok(u) = serde_json::to_value(&usage) {
            map.insert("usage".to_string(), u);
        }
    }
```

- [ ] **Step 4: Run to verify it passes + build the workspace**

Run: `cargo test -p rupu-cp session_usage_from_dto` then `cargo build -p rupu-cp`
Expected: PASS / builds (the `collect_sessions` signature change ripples to `projects.rs` + `dashboard.rs` — update those call sites in this task).

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/api/sessions.rs crates/rupu-cp/src/api/projects.rs crates/rupu-cp/src/api/dashboard.rs
git commit -m "feat(cp): per-session usage (tokens+cost) from session totals

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Project rollup `usage` + per-row usage

**Files:**
- Modify: `crates/rupu-cp/src/api/projects.rs`

Add a `usage` field to the `ProjectDetail` rollup (sum across the project's runs), and fill per-row usage on `GET /api/projects/:ws_id/runs`. Project rollup is over **runs** (workflow + autoflow); sessions carry their own usage on the sessions surface (avoids double-counting; documented in the spec).

- [ ] **Step 1: Write the failing test**

Project handlers need a store fixture; assert the rollup math via a small helper test instead. Add to a `#[cfg(test)] mod tests` at the bottom of `projects.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_detail_serializes_usage() {
        let detail = ProjectDetail {
            project: ProjectRow {
                ws_id: "w".into(), name: "n".into(), path: "/p".into(),
                repo_remote: None, branch: None, created_at: String::new(), last_run_at: None,
            },
            runs: json!({}),
            sessions: json!({}),
            coverage: json!({}),
            recent_runs: vec![],
            usage: crate::usage::UsageSummary::default(),
        };
        let v = serde_json::to_value(&detail).unwrap();
        assert!(v.get("usage").is_some());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cp project_detail_serializes_usage`
Expected: FAIL — `ProjectDetail` has no `usage` field.

- [ ] **Step 3: Implement**

In `crates/rupu-cp/src/api/projects.rs`:

1. Add to the struct:

```rust
#[derive(Serialize)]
struct ProjectDetail {
    project: ProjectRow,
    runs: Value,
    sessions: Value,
    coverage: Value,
    recent_runs: Vec<RunListRow>,
    usage: crate::usage::UsageSummary,
}
```

2. In `get_project`, after `scoped_runs` is available, compute the rollup and pass it in the returned struct:

```rust
    let usage = crate::usage::rollup(
        runs.iter()
            .map(|r| crate::usage::summarize_run(&s.run_store, &r.id, &s.pricing)),
    );
```

then add `usage,` to the `ProjectDetail { … }` literal.

3. Make `project_runs` carry per-row usage:

```rust
async fn project_runs(
    State(s): State<AppState>,
    Path(ws_id): Path<String>,
) -> ApiResult<Json<Vec<RunListRow>>> {
    load_workspace(&s, &ws_id)?;
    let runs = scoped_runs(&s, &ws_id)?;
    Ok(Json(
        runs.iter()
            .map(|r| RunListRow::with_usage(r, &s.run_store, &s.pricing))
            .collect(),
    ))
}
```

- [ ] **Step 4: Run to verify it passes + build**

Run: `cargo test -p rupu-cp project_detail_serializes_usage` then `cargo build -p rupu-cp`
Expected: PASS / builds.

- [ ] **Step 5: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/src/api/projects.rs
git commit -m "feat(cp): project usage rollup + per-run usage

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Workflow detail `usage` rollup

**Files:**
- Modify: `crates/rupu-cp/src/api/workflows.rs`

Add a `usage` rollup to `GET /api/workflows/:name`, summing across all runs whose `workflow_name == name`.

- [ ] **Step 1: Implement (handler-level; verified by build + a route smoke test in Task 11)**

In `get_workflow`, after parsing the workflow, compute the rollup and add it to the JSON:

```rust
async fn get_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = s.global_dir.join("workflows").join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ApiError::not_found(format!("workflow {name} not found")));
    }
    let yaml = std::fs::read_to_string(&path).map_err(|e| ApiError::internal(e.to_string()))?;
    let workflow = Workflow::parse(&yaml).map_err(|e| ApiError::internal(e.to_string()))?;

    let runs = s.run_store.list().unwrap_or_default();
    let usage = crate::usage::rollup(
        runs.iter()
            .filter(|r| r.workflow_name == name)
            .map(|r| crate::usage::summarize_run(&s.run_store, &r.id, &s.pricing)),
    );

    Ok(Json(serde_json::json!({ "workflow": workflow, "yaml": yaml, "usage": usage })))
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p rupu-cp`
Expected: builds.

- [ ] **Step 3: Clippy gate**

Run: `cargo clippy -p rupu-cp --all-targets`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cp/src/api/workflows.rs
git commit -m "feat(cp): workflow detail usage rollup

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: `GET /api/usage` overview endpoint

**Files:**
- Create: `crates/rupu-cp/src/api/usage.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs` (add `pub mod usage;`)
- Modify: `crates/rupu-cp/src/server.rs` (merge `crate::api::usage::routes()`)

`GET /api/usage?since=&until=&group_by=provider|model|agent` returns `{ summary, breakdown }` over **all RunStore runs' transcripts** across all workspaces (the firehose — consistent with how A.1 scoped Coverage, and unambiguous re: double-counting). The window filters runs by `started_at`; default = last 30 days (matches the CLI). **Scope note for the implementer:** sessions are NOT folded into this global number (they carry their own usage on the sessions surface); standalone non-RunStore transcripts are out of scope for 3a. Do not silently widen this — the boundary is intentional and documented in the spec.

`since`/`until` accept RFC-3339; absent `since` defaults to now − 30 days. Reuse `chrono` (already a dep).

- [ ] **Step 1: Write the failing test (window parse helper)**

Create `crates/rupu-cp/src/api/usage.rs`:

```rust
//! `GET /api/usage` — global token + cost overview (summary + breakdown).

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/usage", get(get_usage))
}

#[derive(Debug, Deserialize)]
struct UsageQuery {
    since: Option<String>,
    until: Option<String>,
    group_by: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageResponse {
    summary: crate::usage::UsageSummary,
    breakdown: Vec<crate::usage::UsageBreakdownRow>,
}

/// Resolve the [since, until] window from optional RFC-3339 strings.
/// Absent `since` → now − 30 days; absent `until` → now. A present-but-unparseable
/// bound is an error (400) rather than a silent default.
fn resolve_window(
    since: Option<&str>,
    until: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let parse = |s: &str| -> Result<DateTime<Utc>, String> {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .map_err(|e| format!("invalid timestamp {s:?}: {e}"))
    };
    let start = match since {
        Some(s) => parse(s)?,
        None => now - Duration::days(30),
    };
    let end = match until {
        Some(u) => parse(u)?,
        None => now,
    };
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_window_defaults_to_30_days() {
        let now = DateTime::parse_from_rfc3339("2026-06-20T00:00:00Z").unwrap().with_timezone(&Utc);
        let (start, end) = resolve_window(None, None, now).unwrap();
        assert_eq!(end, now);
        assert_eq!(start, now - Duration::days(30));
    }

    #[test]
    fn resolve_window_parses_explicit_bounds() {
        let now = Utc::now();
        let (start, end) = resolve_window(Some("2026-01-01T00:00:00Z"), Some("2026-02-01T00:00:00Z"), now).unwrap();
        assert_eq!(start, DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().with_timezone(&Utc));
        assert_eq!(end, DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z").unwrap().with_timezone(&Utc));
    }

    #[test]
    fn resolve_window_rejects_garbage() {
        let now = Utc::now();
        assert!(resolve_window(Some("not-a-date"), None, now).is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails (module not wired)**

Add `pub mod usage;` to `crates/rupu-cp/src/api/mod.rs` (after `pub mod transcript;` keeps it alphabetical-ish; any position works). Then:

Run: `cargo test -p rupu-cp resolve_window`
Expected: PASS (the helper is self-contained). If it doesn't compile because the handler `get_usage` is referenced but undefined, add the handler in Step 3 first.

- [ ] **Step 3: Implement the handler**

Append to `crates/rupu-cp/src/api/usage.rs`:

```rust
async fn get_usage(
    State(s): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> ApiResult<Json<UsageResponse>> {
    let (start, end) = resolve_window(q.since.as_deref(), q.until.as_deref(), Utc::now())
        .map_err(ApiError::bad_request)?;
    let group_by = crate::usage::GroupBy::parse(q.group_by.as_deref().unwrap_or("model"));

    // All runs in the window, across all workspaces (the firehose).
    let runs = s
        .run_store
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    // Gather every in-window run's transcript rows, then summarize + break down.
    let mut all_rows: Vec<rupu_transcript::UsageRow> = Vec::new();
    for r in runs.iter().filter(|r| r.started_at >= start && r.started_at <= end) {
        let paths = crate::usage::run_transcript_paths(&s.run_store, &r.id);
        all_rows.extend(rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default()));
    }

    let summary = crate::usage::summarize(&all_rows, &s.pricing);
    let breakdown = crate::usage::breakdown(&all_rows, &s.pricing, group_by);
    Ok(Json(UsageResponse { summary, breakdown }))
}
```

`ApiError::bad_request(msg)` already exists in `crates/rupu-cp/src/error.rs` (tuple struct `ApiError(pub StatusCode, pub String)`), so no error-type change is needed.

- [ ] **Step 4: Register the route**

In `crates/rupu-cp/src/server.rs`, add to the `api` router chain (next to the other `.merge(...)` calls):

```rust
        .merge(crate::api::usage::routes())
```

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p rupu-cp` then `cargo build -p rupu-cp`
Expected: PASS / builds.

- [ ] **Step 6: Clippy gate + full workspace check**

Run: `cargo clippy --workspace --all-targets`
Expected: no warnings across the workspace (the lift touched `rupu-config`, `rupu-cli`, `rupu-cp`).

- [ ] **Step 7: Manual smoke (optional but recommended)**

```bash
cargo run -p rupu-cli -- cp serve --bind 127.0.0.1:8787 &
sleep 1
curl -s 'http://127.0.0.1:8787/api/usage?group_by=model' | head -c 400
curl -s 'http://127.0.0.1:8787/api/runs' | head -c 400
kill %1
```
Expected: JSON with `summary`/`breakdown`; run rows carry a `usage` object.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-cp/src/api/usage.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs
git commit -m "feat(cp): GET /api/usage overview (summary + breakdown)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Done criteria (whole plan)

- `cargo test --workspace` green; `cargo clippy --workspace --all-targets` clean.
- `rupu-cli` still builds + tests after the pricing lift (no behavior change).
- `rupu-cp` exposes `usage` on run list/detail, session list/detail, project rollup + project runs, workflow detail, and a new `/api/usage` overview.
- `rupu-cp` has **no** `rupu-cli` dependency (unchanged `Cargo.toml`).
- Cost is never fabricated: unpriced models yield `priced:false` + `cost_usd:null`/partial.
- The frontend rendering of all this is **Plan 3b** (separate).
