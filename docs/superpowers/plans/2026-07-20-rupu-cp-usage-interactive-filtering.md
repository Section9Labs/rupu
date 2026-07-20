# Interactive Usage Filtering + Shared Project Graph — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the `/usage` spend graph interactive — select from the shown rows (models / agents / … per pivot) and outlier runs to add/remove them from the graph in real time, so a 2000×-cost outlier can be pulled out and the axis rescales to something legible. Reuse the same graph, project-scoped, on the Projects page.

**Why:** One extreme run (e.g. 2000× the median cost) flattens every other series into the axis floor — the graph is useless until you can exclude offenders. Filtering by run/model/agent/etc. is the fix, and it must be instant.

**Architecture (approved design, 2026-07-20):** The server exposes per-`(run × model)` usage rows (finest grain; a run can span models). The client holds that flat list and a pure function buckets-by-day + stacks-by-pivot + **excludes** any row whose `run_id` or pivot-key is toggled off — so every toggle is instant, no refetch. The client only *sums* costs the server already priced per row; pricing never moves client-side, so numbers can't drift. The same per-run endpoint, scoped by `workspace_id`, feeds the same graph component on the Projects page.

**Tech Stack:** Rust (axum, serde, chrono), React + TS + Recharts (no new deps), Vitest.

## Global Constraints

- Workspace deps only; NEVER a version literal in a crate Cargo.toml. rupu-cp READ-ONLY. `#![deny(clippy::all)]`; `unsafe_code` forbidden. `thiserror` for libs.
- **NEVER run `cargo fmt` in any form.** `rustfmt --edition 2021 <path>` one file at a time; never a crate root or `mod.rs`. `git diff --stat` after; revert stray files.
- **NO new npm deps.** **NEVER hardcode a color literal** in web code — `useThemeColors()` / `--c-*` tokens only.
- Reports MUST paste literal command output (this project has had a subagent claim clippy clean without running it).
- rustc resolves 1.95 (Homebrew) vs pinned 1.88. `cargo clippy -p rupu-cp --all-targets` has a small fixed set of pre-existing errors — **the implementer must first record the baseline count at the task's base commit, then verify it's unchanged after.** Do NOT assume a number.
- Do NOT `git push` / `git checkout <commit>` / detached HEAD / `git stash` (push.default=matching force-updates unrelated branches; the controller pushes with an explicit refspec).

---

### Task U1: `GET /api/usage/runs` — per-run usage rows

**Files:** `crates/rupu-cp/src/api/usage.rs` (+ `crates/rupu-cp/tests/usage.rs`), `crates/rupu-cp/src/server.rs` (route).

**Produces:** `GET /api/usage/runs?since=&workspace_id=` → `Vec<UsageRunRow>`:
```rust
struct UsageRunRow {
    run_id: String,
    started_at: DateTime<Utc>,   // RFC-3339; the day-bucket source. From the RunRecord.
    workflow_name: String,
    agent: String,
    provider: String,
    model: String,
    workspace_id: String,
    host_id: String,             // "local" for this local-only endpoint
    input_tokens: u64, output_tokens: u64, cached_tokens: u64, total_tokens: u64,
    cost_usd: Option<f64>,       // priced server-side per row; None = unpriced
    priced: bool,
}
```

**Approach — reuse, don't re-derive.** The `/api/usage` handler (`get_usage`) already iterates runs and builds `Vec<rupu_transcript::UsageRow>` per run with Task-2 attribution (`workflow`/`host_id`/`workspace_id`) and prices them. A `UsageRow` is already at `(provider, model, agent, …, tokens)` grain per run. This task emits those rows FLAT (one `UsageRunRow` per `UsageRow`), attaching the run's `run_id` + `started_at` (from the `RunRecord` in the loop) and the per-row priced `cost_usd`. Read how `get_usage` builds + prices rows and mirror it; factor a shared helper if it avoids duplication, but do NOT change `/api/usage`'s behavior.

- Local-only (like `/api/usage/timeline` + `/api/usage/outliers`) — reads `s.run_store`, no fan-out. Document it.
- `?workspace_id=<id>` filters rows to that project's runs (`r.workspace_id == id`). Absent → all local runs. This is what the Projects page uses.
- `?since=` bounds by `started_at` (reuse the existing since-parsing the other usage routes use).
- `cost_usd` per row: use the SAME price-resolution path `summarize`/`breakdown` use (do not add a second lookup).

- [ ] **Step 1: record clippy baseline** — `cargo clippy -p rupu-cp --all-targets -- -D warnings 2>&1 | grep -cE '^error'` — note the number N.
- [ ] **Step 2: write the failing integration test** in `tests/usage.rs`: seed 2 runs (different workflows/models, known tokens), `GET /api/usage/runs`, assert one row per (run×model) with correct `run_id`/`started_at`/`workflow`/`model`/`cost_usd`; and `?workspace_id=<one>` returns only that project's rows. `?since` excludes an old run.
- [ ] **Step 3: run — fail.**
- [ ] **Step 4: implement** the handler + route + `UsageRunRow`.
- [ ] **Step 5: verify** — `cargo test -p rupu-cp usage`; `cargo test -p rupu-cp` no new failures; clippy count still N. Manual: `RUPU_HOME=$HOME/.rupu rupu cp serve --bind 127.0.0.1:17890 --no-open &`, `curl -s 'http://127.0.0.1:17890/api/usage/runs' | head -c 400` — paste it; confirm flat per-run rows with `started_at`+`cost_usd`. Kill server.
- [ ] **Step 6: commit.**

---

### Task U2: client per-run aggregation (pure) + TS types + client method

**Files:** `crates/rupu-cp/web/src/lib/usage/buildTimeline.ts` (new) + `.test.ts`; `crates/rupu-cp/web/src/lib/api.ts` (types + `getUsageRuns`).

**Produces:**
- TS `UsageRunRow` matching U1's Rust struct EXACTLY (read the Rust; this project has had TS/Rust drift bugs). `cost_usd: number | null`.
- `api.getUsageRuns(range: DashboardRange, workspaceId?: string): Promise<UsageRunRow[]>` → `GET /api/usage/runs?since=&workspace_id=`.
- Pure function:
```ts
type Pivot = 'model' | 'provider' | 'agent' | 'workflow' | 'host' | 'project';
interface TimelineFilter { excludedRunIds: Set<string>; excludedKeys: Set<string>; }
function buildTimeline(rows: UsageRunRow[], pivot: Pivot, filter: TimelineFilter, bucket: 'day'|'week'):
  UsageTimelineBucket[]   // the EXISTING shape UsageTimelineStacked consumes
```

**Semantics (the correctness core — unit-test each):**
- Bucket by `started_at` truncated to the day (or ISO-Monday for week) — match the day-key convention the existing timeline uses so the x-axis is consistent.
- The stacking key per row = the pivot field (`model`/`provider`/`agent`/`workflow`/`host_id`/`workspace_id`). Emit `UsageBreakdownRow`s per bucket keyed by that dimension, summing tokens + `cost_usd` (treat `null` cost as 0 FOR SUMMING THE GRAPH — but see below).
- **Exclusion:** skip any row where `filter.excludedRunIds.has(run_id)` OR `filter.excludedKeys.has(<the pivot key>)`. This is what makes removing the 2000× run rescale the axis — its rows vanish from every bucket.
- **The client only SUMS server-priced `cost_usd`; it never prices.** A `null`-cost row contributes 0 to the cost graph (it's genuinely unpriced) — do NOT fabricate a price. (Unpriced accounting is surfaced separately by the existing `UnpricedBanner`; this function is just the graph.)
- Buckets must be contiguous where the existing timeline zero-fills — check whether `UsageTimelineStacked` needs a zero-filled grid or tolerates gaps; match its expectation.

**Tests:** two runs same day different models → one bucket, two model series summed; excluding a run_id drops its contribution (and if it was the only row for a model, that series disappears); excluding a pivot key drops all its rows; pivot=agent stacks by agent; a null-cost row adds 0 to cost but its tokens still count for the tokens metric.

- [ ] Steps: write failing tests → fail → implement → pass → `npx vitest run src/lib/usage/buildTimeline.test.ts`, `npx tsc --noEmit` (errors confined to files U3/U4 rewrite are OK; report) → commit.

---

### Task U3: interactive `/usage` page

**Files:** `crates/rupu-cp/web/src/pages/Usage.tsx` + `.test.tsx`; likely small edits to `ModelBreakdownTable.tsx` (selectable rows) and `OutlierPanel.tsx` (exclude toggle).

**Behavior:**
- Fetch `getUsageRuns(range)` (replacing `getUsageTimeline`). Keep `getUsage(range, pivot)` for the breakdown table summary + `getUsageOutliers(range)` for the outlier panel, and keep the fleet-wide `data.summary` headline + `UnpricedBanner` + `HostFreshnessStrip` as-is.
- **Pivot picker drives the graph:** `buildTimeline(runs, pivot, filter, 'day')` feeds `UsageTimelineStacked`. Changing pivot re-stacks the graph (no refetch — `getUsageRuns` is pivot-independent).
- **Breakdown table rows become checkboxes:** clicking a row toggles its pivot-key in `filter.excludedKeys`. Show ALL rows (scrollable) when interactive — the top-6 rollup hides items you might want to toggle; either always-show-all here or an expand control. An excluded row renders visibly muted/struck.
- **Outlier rows get an exclude toggle** → toggles `run_id` in `filter.excludedRunIds`.
- **"Excluded (N) · reset"** chip near the graph clears both sets.
- Filter state = two `useState<Set>`; every toggle re-runs `buildTimeline` (memoize on `[runs, pivot, filter]`). Instant.
- Keep the "local host only" caption on the graph and "(this host only)" on outliers (these endpoints stay local).

**Tests (`Usage.test.tsx`, fireEvent — user-event is NOT a dep; jsdom env + jest-dom + afterEach(cleanup)):** mock `getUsageRuns` + `getUsage` + `getUsageOutliers`; assert the graph renders; toggling a breakdown row excludes it (the chip shows "Excluded (1)"); toggling an outlier excludes its run; reset clears; changing pivot re-stacks.

- [ ] Steps: failing tests → fail → implement → pass → `npx vitest run src/pages/Usage.test.tsx src/components/usage/`, whole suite, `npx tsc --noEmit` clean, `npm run build` succeeds → commit. **Controller does browser validation.**

---

### Task U4: shared graph on the Projects page

**Files:** `crates/rupu-cp/web/src/components/project/ProjectRunsTab.tsx` (and check `ProjectDetail.tsx`); possibly extract a shared `<UsageTimeline>` wrapper.

**Behavior:**
- Replace the current `UsageBarChart` (one bar per run, stacked by token type) in the project usage view with the same spend-over-time graph: `getUsageRuns(range, wsId)` → `buildTimeline` → `UsageTimelineStacked`, scoped to the project's `workspace_id`.
- Reuse — do NOT fork the graph. If U3 produced a self-contained interactive graph section, extract it as `<UsageTimeline runs={...} .../>` (or a small wrapper that owns fetch+filter+graph) and mount it on both pages; the project page passes `workspaceId`. The project graph SHOULD get the same interactivity for free (that is the point of reuse) unless it complicates the tab — if scope-limited, at minimum the same graph + pivot; note any interactivity deferred.
- Do NOT delete `UsageBarChart` if other pages still use it (grep first — the Projects LIST page uses it too, one bar per project; leave that). Only swap the per-run project timeline usage.

**Tests:** the project usage tab renders the timeline from a mocked `getUsageRuns(range, wsId)`; the wsId is passed through.

- [ ] Steps: failing test → fail → implement → pass → vitest + `tsc --noEmit` clean + `npm run build` → commit. **Controller browser-validates the project view.**

---

## Definition of Done
- `GET /api/usage/runs` returns flat per-(run×model) rows, `?workspace_id=` scopes it; `cargo test -p rupu-cp` green; clippy at recorded baseline.
- `/usage`: pivot drives the graph; toggling breakdown rows / outlier runs adds+removes them instantly (no refetch); a 2000× outlier can be excluded and the axis rescales; "excluded (N) · reset" works.
- Projects usage view uses the same spend-over-time graph, project-scoped.
- Web: `npx vitest run`, `tsc --noEmit`, `npm run build` all clean; no new deps; no color literals.
- Numbers verified against real `~/.rupu` data; controller browser-validates both pages in light + dark.
