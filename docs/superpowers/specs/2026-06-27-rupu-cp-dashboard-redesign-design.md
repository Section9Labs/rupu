# rupu-cp — Dashboard redesign + correct multi-model usage — Design

**Date:** 2026-06-27
**Surfaces:** `rupu-transcript` + `rupu-agent` (model-attribution fix), `rupu-cp` (timeline endpoint), `rupu-cp/web` (Dashboard redesign)
**Status:** approved direction (matt: redesign + "record both" model fix), pending spec review

## Goals
1. **Fix usage correctness** — runs collapse to a single model (`claude-mythos-preview`) and cost is wrong/missing. Record the **requested** model (for grouping + pricing) AND the server's **served** model (for reference).
2. **Per-model usage timeline** — token/cost spend over time, broken down by model (matt: "in a timeline much like other token spending").
3. **Complete Dashboard redesign** — "Spend-Forward Operations": triage ribbon → usage hero (timeline + all-models breakdown) → supporting tiles → recent runs + status donut.

## Root cause (confirmed)
The CP aggregation groups by model correctly (`usage.rs` `breakdown(_, _, GroupBy::Model)`). The collapse is upstream: `rupu-agent/src/runner.rs` writes the per-turn `Usage` event with `model: resp.model` — the **server-reported** model. Anthropic's API echoes one served id (`claude-mythos-preview`) for every Claude request regardless of which model was asked for, so all Anthropic turns carry that id (collapsing opus/sonnet/haiku) and it's absent from the price table (→ unpriced). OpenAI/Gemini/Copilot record correctly.

## ① Model-attribution fix (record both)
- **`rupu-transcript` `Event::Usage`** (`crates/rupu-transcript/src/event.rs`): `model` becomes the **requested** model; add `#[serde(default)] served_model: Option<String>` for the server-echoed id. (Consider the same on `RunStart` for completeness, but `Usage` is what aggregation reads — do `Usage` minimum.) `#[serde(default)]` keeps old transcripts parsing.
- **`rupu-agent` `runner.rs`** (the Usage emit, ~862): set `model = <requested model>` (the resolved/configured `opts.model`), `served_model = Some(resp.model)` only when `resp.model` is non-empty AND differs from the requested (else `None`). For providers that already return the real model (OpenAI/Copilot) requested≈served → `served_model = None` (or equal). For Gemini (empty `resp.model`) → `model = opts.model`, `served_model = None` (today's fallback, now explicit).
- **Aggregation** (`rupu-transcript::aggregate` + `usage.rs breakdown`/`rollup`) groups on `model` (now requested) — already correct, just gets clean data. Pricing (`pricing::lookup`) now resolves (requested ids are in the builtin table).
- **Surface served (reference):** the per-model breakdown row / run-detail usage may show `served: <id>` as secondary text when it differs. Minimal in v1 (a tooltip / sub-line); the Dashboard groups by requested.

**This fix alone makes the EXISTING Dashboard breakdown correct** (un-collapses Claude family + prices) — it's PR 1 and shippable independently.

## ② Per-model usage timeline (backend)
- New `GET /api/usage/timeline?bucket=day&since=…&until=…` → `Vec<{ bucket: "YYYY-MM-DD", rows: Vec<UsageBreakdownRow> }>` (each row = `{ model, provider, tokens_in/out/cached, cost_usd: Option, priced }`).
- Implementation (cheapest correct path, per analysis): one pass over `run_store.list()`; for each run in `[since, until]`, bucket `run.started_at` (day; `All` → weekly), aggregate that run's `run_transcript_paths` via `rupu_transcript::aggregate` → `UsageRow`s, accumulate into cells keyed `(bucket, model)`, price via `pricing::lookup`. Reuses `run_transcript_paths` + `aggregate` + `breakdown`. Thin handler in `crates/rupu-cp/src/api/usage.rs`.

## ③ Dashboard redesign (frontend) — "Spend-Forward Operations"
Layout (top→bottom):
- **Header:** title + a **global time-range** segmented control (`7d | 30d | All`) driving the timeline buckets + all "last N" figures + an "updated Ns ago ↻".
- **Triage ribbon** (thin, full-width, color-coded, clickable): **Running** (→ runs), **Awaiting approval** (pulses amber when >0 → approvals), **Failed (in window)** (→ failed runs), **Open findings** (→ findings). Each = number + verb + click-through.
- **Usage hero** (the centerpiece, ~⅔ timeline / ⅓ breakdown):
  - **Per-model timeline** — a **stacked area** (recharts `Area` + `stackId`, reusing the chart primitives) of cost/day per model; legend bands colored to match the breakdown. A **Cost $ | Tokens** toggle (default Cost; Tokens shows unpriced models as solid bands so a copilot/antigravity-heavy day isn't shown as "free"). NOT the per-run diverging style (wrong altitude — that stays on Run detail).
  - **All-models breakdown** — a table: model · provider · tokens · cost · share% bar. **Top 6 priced** + `others (N)` rollup (expandable); **unpriced models pinned below a divider**, cost `—*`, share = "unpriced", tokens still counted; footer splits `$X (priced) · Y tokens unpriced`. Sort by cost/tokens/runs. **Never render $0 for unknown cost.**
- **Secondary tiles** (small row): Total runs, Sessions (active/total), Workers, Coverage (merged `N tgt · M assertions`).
- **Bottom rail:** Recent runs (60%, with per-run `UsageChip`) + Run status donut (40%) — the donut demoted from its own full-width section.
- **Cut:** the standalone full-width status section heading + the old "top models by cost" horizontal bar (the breakdown table replaces it).

**Interaction:** keep the 15s poll (+ shimmer on refresh, awaiting chip pulses); global range drives the page; Cost/Tokens toggle; click-through (triage chip → filtered runs/approvals/findings; legend/breakdown row → Usage page filtered to that model; donut slice → filtered runs; recent row → run detail). **Empty states:** no usage → hero collapses to a one-line note with a ghost axis; all-unpriced → chart defaults to Tokens mode; no runs → triage chips show muted 0s.

## Files
**Engine fix (PR 1):** `crates/rupu-transcript/src/event.rs` (Usage `served_model`), `crates/rupu-transcript/src/aggregate.rs` (carry served if needed), `crates/rupu-agent/src/runner.rs` (record requested + served). Tests.
**Timeline (PR 2):** `crates/rupu-cp/src/api/usage.rs` (`/api/usage/timeline`), maybe a small bucketing helper. Tests.
**Redesign (PR 3):** `crates/rupu-cp/web/src/pages/Dashboard.tsx` (rewrite), new chart/section components (`UsageTimelineStacked`, `ModelBreakdownTable`, `TriageRibbon`), `lib/api.ts` (`getUsageTimeline`), `lib/usage.ts` types.

## Phasing (PRs, one at a time)
- **PR 1 — model fix** (un-collapses + prices; makes the current Dashboard correct). Highest-value correctness; ship first.
- **PR 2 — timeline endpoint** (backend, testable via curl).
- **PR 3 — Dashboard redesign** (the new UI on top of 1+2).

## Testing
- Engine: a transcript with two turns at *different requested models* but the *same served id* aggregates into TWO model groups (requested), each priced; `served_model` recorded. `rupu-transcript`/`rupu-agent` tests; CI on 1.88 for agent.
- Backend: `/api/usage/timeline` buckets per day with per-model rows; window filtering; unpriced rows have `cost_usd: null`. clippy clean.
- Frontend: stacked timeline renders per-model series + Cost/Tokens toggle; breakdown table shows all models + others rollup + unpriced-honest; triage ribbon counts + click-through; empty/unpriced states. Suite green; build strict; recharts lazy-chunked (the stacked chart goes in the chart chunk, not main).
- Visual validation by matt: the Dashboard shows correct multi-model spend over time.

## Non-goals / deferred (TODO)
- No backfill of historical transcripts (old runs keep the collapsed served id; the fix is forward-looking — note it; a price-table alias for `claude-mythos-preview` could retro-price old data if wanted).
- No cross-provider cost normalization beyond the existing price table.
- Intra-day/hour buckets (day/weekly only in v1).
