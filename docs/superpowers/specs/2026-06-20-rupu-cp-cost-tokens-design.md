# rupu Control Plane — Cost & Tokens (Slice A.3) — Design

**Date:** 2026-06-20
**Author:** matt + Claude
**Status:** Design draft (approved in brainstorm)
**Builds on:** Slice A (#322) + A.1 (#323) + A.2 (#324), all merged to `main`. This slice branches from `main`.

## Summary

The CP renders runs, sessions, workflows, and projects but shows **no cost and almost no token data** — the Dashboard explicitly notes "NO token/cost data available," and `rupu-cp` builds an empty `PricingConfig::default()` that nothing reads. matt's visual-pass issue **#9**: *"we need to also show general token consumption, general money as well. Projects should show this for each run, each session, each workflow in total. Much like we do in the CLI. Cost and token is important."*

This slice computes **token consumption and dollar cost server-side** from the real price table and surfaces it everywhere runs are shown: per run, per session, per workflow, per project, and a global rollup on the Dashboard.

The cost machinery already mostly exists — `rupu-config` has `ModelPricing::cost_usd()` (the formula) and `PricingConfig`; `rupu-transcript` has `aggregate()` (tokens per provider/model/agent) and `JsonlReader`; `rupu-cp` already depends on both. The **only** missing CP-reachable piece is the price table + resolver, which today lives in `rupu-cli/src/pricing.rs`. The keystone of this slice is lifting that into `rupu-config`.

This slice is **read-only** and `rupu-cp` continues to have **no `rupu-cli` dependency**.

## Design decisions (locked with matt)

- **Pricing lift target:** move `rupu-cli/src/pricing.rs` into **`rupu-config`** (next to `PricingConfig`/`ModelPricing`/`cost_usd` — one conceptual unit). NOT a new `rupu-usage` crate (heavier, pulls `RunStore`/`anyhow` the CP doesn't need), NOT a duplicated table (DRY/no-mock violation).
- **Cost is an estimate, never fabricated.** Computed from the builtin price table, with the user's global `[pricing]` overrides honored. When a model has no resolvable price, show tokens + an em-dash and a `priced: false` flag — never a fake `$0.00`. Mirrors the CLI's `—`/partial convention.
- **Computation is server-side.** The CP backend computes cost in handlers; the frontend only renders. No price table in TypeScript.
- **Rendering surfaces** (as matt specified in #9): run, session, workflow, project, and a general Dashboard rollup.

## Three facts that ground the design (from the code investigation)

1. **Cost formula already exists** — `rupu_config::ModelPricing::cost_usd(input, output, cached)` (`crates/rupu-config/src/pricing_config.rs`): `((input − cached)·input_rate + cached·cached_rate + output·output_rate) / 1e6`, cached treated as a subset of input, cached rate falls back to input rate when unset.
2. **Token aggregation already exists** — `rupu_transcript::aggregate(paths, TimeWindow)` → `Vec<UsageRow>` (`crates/rupu-transcript/src/aggregate.rs`), one row per `(provider, model, agent)` summing all `Usage` events. **`UsageRow` does NOT derive `Serialize`** today (CLI renders it as a table) — the CP defines its own serializable cost DTOs rather than exposing `UsageRow` directly.
3. **The resolver is pure** — `rupu-cli/src/pricing.rs` `lookup(cfg, provider, model, agent)` (3-tier: user model config → `BUILTIN_PRICES` → user agent fallback) plus `canonicalize_provider` / `strip_date_suffix` / `strip_model_tag` depends only on `rupu_config` types and has **no `anyhow`** — it lifts cleanly. `lookup()` falls back to `BUILTIN_PRICES`, so cost works even with an empty user `PricingConfig` — which is exactly why the CP's `::default()` showed nothing (it was the *data* that was missing, not the formula).

## Architecture

### Part 1 — Pricing lift (`rupu-config`, `rupu-cli`)

- **Move** `crates/rupu-cli/src/pricing.rs` → `crates/rupu-config/src/pricing.rs`. Exports: `BUILTIN_PRICES`, `pub fn lookup(cfg: &PricingConfig, provider: &str, model: &str, agent: &str) -> Option<ModelPricing>`, and the helper fns. Add `pub mod pricing;` to `rupu-config/src/lib.rs`. The module's existing unit tests move with it.
- **`rupu-cli`** drops its `pricing` module and switches call sites (`crate::pricing::lookup` → `rupu_config::pricing::lookup`, `crate::pricing::BUILTIN_PRICES` → `rupu_config::pricing::BUILTIN_PRICES`). The CLI's `usage_report.rs` / `usage.rs` reporting layer (`UsageDataset`, `UsageFact`, `UsageRun`) is **unchanged** beyond the import swap.
- No workspace dependency changes (pure-code move within an existing dep).

### Part 2 — CP cost computation (`rupu-cp`, new module `src/usage.rs`)

A small read-side module with serializable DTOs and pure aggregation over building blocks the CP already has.

```rust
/// Token + cost summary for a run (or a rollup of runs).
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,        // input + output
    pub cost_usd: Option<f64>,    // None when no contributing row was priced
    pub priced: bool,             // false when ≥1 contributing model lacked a price
    pub runs: u64,                // distinct runs contributing (for rollups)
}

/// Per (provider, model, agent) line for the overview breakdown.
#[derive(Debug, Clone, Serialize)]
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
```

Functions:
- `fn summarize(rows: &[rupu_transcript::UsageRow], pricing: &PricingConfig) -> UsageSummary` — sums tokens; for each row resolves price via `pricing::lookup` and adds `cost_usd(...)`; if **any** row is unpriced, `priced=false` and `cost_usd` reflects only the priced rows (partial total). All-unpriced → `cost_usd=None`.
- `fn summarize_paths(paths: &[PathBuf], pricing: &PricingConfig) -> UsageSummary` — `aggregate(paths, default)` then `summarize`. Used per run.
- `fn rollup(summaries: impl Iterator<Item = UsageSummary>) -> UsageSummary` — adds token fields, ORs the unpriced flag, sums `cost_usd` treating `None` as 0 but flipping `priced=false`, sums `runs`. Used per session/workflow/project/global.
- `fn breakdown(rows: &[UsageRow], pricing: &PricingConfig, group_by: GroupBy) -> Vec<UsageBreakdownRow>` — for the overview endpoint (group by provider | model | agent).

The transcript paths for a run come from the same source the existing run/graph handlers use (`RunStore` step results → transcript paths; standalone run → its transcript path). Cost is computed **on read** — fresh each request — so active runs reflect streamed tokens.

### Part 3 — Load real pricing at startup (`rupu-cp`)

`rupu-cp::serve` builds `PricingConfig` from `rupu_config::layer_files(<global_dir>/config.toml, None)` instead of `::default()`, taking the `[pricing]` section (empty is fine — `lookup` falls back to builtins). On a config read/parse error, log a warning and fall back to `PricingConfig::default()` (cost still works via builtins; the server must not fail to boot over a malformed optional `[pricing]`). Store in `AppState.pricing` (the field already exists).

### Part 4 — API (enrich existing + one overview)

**Enrich existing responses** (additive fields; existing clients unaffected):
- `GET /api/runs` rows and `GET /api/runs/:id` → add `usage: UsageSummary`.
- `GET /api/sessions/:id` → add `usage: UsageSummary` (rollup across the session's runs).
- `GET /api/workflows/:name` → add `usage: UsageSummary` (rollup across that workflow's runs).
- `GET /api/projects/:ws_id` → add `usage: UsageSummary` (rollup across the project's runs).
- `GET /api/projects/:ws_id/runs` and `/sessions` → per-row `usage`.

**New overview endpoint** (mirrors `rupu usage`):
- `GET /api/usage?since=&until=&group_by=provider|model|agent` → `{ summary: UsageSummary, breakdown: Vec<UsageBreakdownRow> }` across all registered workspaces' runs (the firehose set, consistent with how A.1 scoped Coverage). Default window: last 30 days (matches the CLI). Registered in `server.rs` router via a new `api::usage::routes()`.

Performance note: rollups read transcript JSONL per run. The run lists already page/bound their result sets; the overview endpoint is window-bounded (default 30 days). If a rollup is measurably slow on a large store, a follow-up can cache per-run summaries keyed by transcript mtime — out of scope here; called out so it isn't silently assumed away.

### Part 5 — Frontend (`crates/rupu-cp/web`)

- **`lib/usage.ts`** — types mirroring the DTOs + `formatCost(cost: number | null, priced: boolean): string` (`$0.0312`, `—` when null) and `formatTokens(n: number): string` (`4,210`, `1.2M`). No price logic — pure formatting.
- **`components/UsageChip.tsx`** — a compact inline `· 4,210 tok · $0.03` chip (em-dash + a subtle "partial" title when `priced=false`). Reused on run rows, RunDetail, session/workflow/project headers.
- **Run rows & RunDetail** (`pages/RunDetail.tsx`, `pages/runs/WorkflowRuns.tsx`, `pages/runs/AgentRuns.tsx`) — `UsageChip` per row; RunDetail shows a small token/cost breakdown (input / output / cached / total / cost).
- **SessionDetail / ProjectDetail** — a rollup stat (total tokens + total $) in the header; per-run `UsageChip` in their lists.
- **Dashboard** (`pages/Dashboard.tsx`) — a **Usage panel**: total spend + tokens for the window, and a top-models-by-cost bar (recharts, already a dep + lazy-chunked). Fills the current "NO token/cost data available" gap. Fetches `GET /api/usage`.
- **`lib/api.ts`** — add `getUsage(params)` and thread the new `usage` fields into the existing typed responses.

All Tailwind static; no `any`; markdown/recharts stay in their existing lazy chunks; main entry chunk stays ~48 KB.

## Components & boundaries

| Unit | Responsibility | Depends on |
|---|---|---|
| `rupu-config::pricing` | price table + resolver (pure) | `rupu-config` types only |
| `rupu-cp::usage` | tokens→cost DTOs + aggregation/rollup/breakdown | `rupu-transcript::aggregate`, `rupu-config::pricing` |
| CP handlers (runs/sessions/workflows/projects/usage) | attach `UsageSummary` / serve overview | `rupu-cp::usage`, `AppState.pricing`, `RunStore` |
| `web/lib/usage.ts` | format only | nothing |
| `web/components/UsageChip` + Dashboard panel | render | `lib/usage`, `lib/api` |

## Error handling

- **Unpriced model:** `priced=false`, `cost_usd` is the partial (priced-only) total or `None` if all unpriced. UI shows `—`. Never a fabricated number.
- **Unreadable/partial transcript:** `aggregate` already skips bad files silently; a run with no `Usage` events yields a zero-token, `priced=true` (`cost_usd=Some(0.0)`) summary — consistent with the CLI.
- **Malformed `[pricing]` at startup:** warn + fall back to builtins; never fail to boot.
- **Overview window parse error:** 400 with a clear message (consistent with existing query-param handling).

## Testing

- **`rupu-config`:** the lifted pricing tests (provider canonicalization, date/tag stripping, 3-tier fallback) run in their new home; `ModelPricing::cost_usd` tier tests stay. `rupu-cli` builds + tests green after the import swap (`cargo test -p rupu-cli`).
- **`rupu-cp` (`usage.rs`):** token sum → cost via a known price; an unpriced model → `priced=false` + no fabricated cost; a mixed priced/unpriced set → partial total + `priced=false`; `rollup` across multiple runs sums correctly and propagates the unpriced flag; `breakdown` groups by provider/model/agent.
- **CP handler tests:** an enriched run response carries `usage`; a project rollup sums its runs; `/api/usage` returns summary + breakdown for a fixture store; window filtering works.
- **Frontend:** `formatCost` / `formatTokens` edge cases (zero, null/unpriced, millions); `UsageChip` renders `—` when unpriced and `$x` when priced; a Dashboard usage-panel render smoke test.
- `cargo build`, `#![deny(clippy::all)]` incl. `--all-targets`, `npm run build` (strict), `npm test -- --run`. Rendering validated by matt (Dashboard panel especially).

## Scope & non-goals

- **In:** the pricing lift, CP cost computation + config load, enriched responses + `/api/usage`, and the frontend rendering across run/session/workflow/project/dashboard.
- **Out (YAGNI / follow-ups):** per-run summary caching (only if measured slow); day/time-series breakdown beyond a single window total; budget alerts/limits; cost in the live SSE event stream (cost is computed on read of the run, which already reflects streamed tokens); CSV export (the CLI already does that).

## Decomposition

One spec, likely **two implementation plans**:
1. **Plan 3a — backend:** pricing lift + CP `usage` module + config load + enriched responses + `/api/usage`, all with tests. Ships independently (API verifiable via curl).
2. **Plan 3b — frontend:** `lib/usage`, `UsageChip`, run/session/project/workflow wiring, Dashboard usage panel.

(The writing-plans step will confirm the split.)
