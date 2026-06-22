# rupu-cp Findings + Timeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add a global Findings view + reusable findings dashboard, rework the per-turn usage timeline into a diverging chart, and fix `for_each` units showing "awaiting" on completed runs.

**Architecture:** `rupu-cp` is a read-adapter axum backend embedding a React/TS Control Plane. Findings are aggregated from the durable coverage ledger (`findings.jsonl`) via a new `GET /api/findings`. Frontend reuses shared severity components across the new Findings page and the existing Coverage detail.

**Tech Stack:** Rust/axum/serde (backend), React/TypeScript/Tailwind/recharts/vitest (frontend).

**Spec:** `docs/superpowers/specs/2026-06-22-rupu-cp-findings-and-timeline-design.md`

**Constraints (every task):** read-adapter — no `rupu-cli` dep in `rupu-cp`; no `any` in TS; static Tailwind only (severity colors via the existing `SEVERITY_STYLE` class map — never dynamic class strings); recharts must stay lazy-chunked (`grep -c recharts dist/assets/index-*.js` = 0 after build); stage only the specific changed files with `git add <paths>` (never `-A`; never commit untracked `.rupu/*`); never run package-wide `cargo fmt`/`rustfmt`. Toolchain note: the worktree runs Homebrew Rust 1.95 while CI pins 1.88 — `rupu-cp` is clean on 1.95, so `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` are valid gates here; ignore any `rupu-cli` red baseline.

---

### Task 1: Fix `for_each` units stuck "awaiting" on completed runs

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/runGraphModel.ts`
- Test: `crates/rupu-cp/web/src/lib/runGraphModel.test.ts` (or the existing colocated test file for this module)

**Context:** `buildRunGraphModel(g, events)` builds graph nodes from a `RunGraphResponse`. Phase 3/4 set each fan-out unit's state from its `success` field — a `success: null` unit defaults to `'running'` (surfaced as "awaiting"), and nothing reconciles against the overall run status. So a *completed* run can show in-flight units. `g.run.status` is available (a `RunStatus`-style string, lowercase e.g. `'completed'`). `StepState` is the union incl. `'pending' | 'running' | 'awaiting_approval' | 'done' | 'failed' | 'skipped'`.

- [ ] **Step 1: Write the failing test.** Construct a `RunGraphResponse` with `run.status === 'completed'`, one workflow step, and one fan-out unit (a `g.units` checkpoint with `success: null`) and no terminal events. Assert the built model's unit state === `'done'` and the parent node state === `'done'`. Add a second test: same shape but `run.status === 'failed'` → the unit stays NON-terminal (`'running'`), i.e. is NOT silently marked done.
- [ ] **Step 2: Run it, confirm the first test fails** (`npm test -- --run runGraphModel`). Expected: unit is `'running'`, not `'done'`.
- [ ] **Step 3: Implement the reconciliation pass.** After Phase 5 (the unit-fold loop that sets `node.fanout`), add: if `g.run.status === 'completed'`, iterate all nodes — promote any node whose state is `'pending' | 'running' | 'awaiting_approval'` to `'done'`, and within each `node.fanout.units` promote any unit whose state is `'running' | 'awaiting_approval'` to `'done'`, then recompute that node's `fanout.byState` counts from the promoted units. Leave all other run statuses untouched. Keep the change isolated to a clearly-commented block ("Phase 5b: reconcile against terminal run status").
- [ ] **Step 4: Run the tests, confirm both pass.**
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/lib/runGraphModel.ts crates/rupu-cp/web/src/lib/runGraphModel.test.ts` → `fix(cp/web): resolve lingering fan-out units to done on completed runs`.

---

### Task 2: Diverging per-turn usage timeline

**Files:**
- Modify: `crates/rupu-cp/web/src/components/charts/RunUsageTimeline.tsx`
- Test: colocated test (new or existing) for the abs-formatter / data-mapping helper.

**Context:** Component takes `series: UsageTimelinePoint[]` (`{ turn, label, tokens_in, tokens_out, tokens_cached }`) + optional `separators`. Today it stacks three areas on one Y axis; input dominates. Used by `RunDetail.tsx` (with `separators`) and `SessionDetail.tsx` (without). recharts is already a lazy chunk — keep imports as-is.

- [ ] **Step 1: Write the failing test** for a small pure helper `formatAbsTick(v: number): string` (returns the abbreviated absolute magnitude, e.g. `-1500 → "1.5k"`, `2000 → "2k"`) — assert it strips the sign. Also assert a `toChartPoint(p)` mapping negates output (`out === -p.tokens_out`) while keeping `in`/`cached` positive.
- [ ] **Step 2: Run it, confirm failure** (helpers not yet exported).
- [ ] **Step 3: Implement the diverging chart.** Export `formatAbsTick` + the point mapping. Render: `Area` for `in` (positive, left axis, `#1860f2`); `Area` for negated `out` (below zero, same left axis, `#22c55e`); a `Line` for `cached` on a secondary right `YAxis` (`yAxisId="cache"`, `#f59e0b`); a `ReferenceLine y={0}` baseline; left-axis `tickFormatter={formatAbsTick}`. `Tooltip` formatter shows TRUE absolute values for all three (un-negate `out`). Preserve the `separators` dashed `ReferenceLine`s and both call sites unchanged.
- [ ] **Step 4: Run the helper test + `npm run build`** (strict), confirm pass + exit 0.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/components/charts/RunUsageTimeline.tsx <test>` → `feat(cp/web): diverging usage timeline (out below zero, cached on own axis)`.

---

### Task 3: Backend `GET /api/findings` aggregate

**Files:**
- Create: `crates/rupu-cp/src/api/findings.rs`
- Modify: `crates/rupu-cp/src/api/mod.rs`, `crates/rupu-cp/src/server.rs`

**Context:** Mirror the aggregation in `crates/rupu-cp/src/api/coverage.rs::list_coverage` (loops workspaces → `rupu_coverage` `discover_targets` → `CoveragePaths::new` → `read_findings`). Today it keeps only `.len()`; here, push the full `FindingRecord`s. `FindingRecord` (rupu-coverage `ledger::events`): `id, file_path: Option<String>, line_range: Option<[u32;2]>, scope, summary, severity, concern_id, evidence, declared_by, declared_at`. `Severity` serializes lowercase.

- [ ] **Step 1: Write failing handler tests** in `findings.rs`: (a) sort order — given findings of mixed severity, the response `findings` are ordered critical→info; (b) summary counts — `summary.{total,critical,high,medium,low,info}` match the inputs; (c) empty — no targets → `{ findings: [], summary: { total: 0, ... } }`. Use the same test-state construction pattern the other `api/*.rs` handler tests use (build an `AppState`/tempdir, write a `findings.jsonl`). If that fixture pattern is heavy, factor the pure logic (sort + summarize over a `Vec<(provenance, FindingRecord)>`) into a testable free function and unit-test that directly.
- [ ] **Step 2: Run `cargo test -p rupu-cp findings`, confirm failure.**
- [ ] **Step 3: Implement.** Define `FindingOut` (flattened `FindingRecord` via `#[serde(flatten)]` + `ws_id`/`project`/`target_id`), `FindingsSummary { total, critical, high, medium, low, info }`, `FindingsResponse { findings: Vec<FindingOut>, summary: FindingsSummary }`. Handler loops all workspaces/targets, collects findings with provenance, sorts by severity (critical→info) then `declared_at` desc, builds the summary, returns `Json`. A single unreadable target is skipped with `tracing::warn!` (never 500). Register `pub fn routes()` (`GET /api/findings`) and wire it in `mod.rs` + `server.rs`.
- [ ] **Step 4: Run `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets`, confirm green/clean.**
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/src/api/findings.rs crates/rupu-cp/src/api/mod.rs crates/rupu-cp/src/server.rs` → `feat(cp): GET /api/findings aggregate across all projects`.

---

### Task 4: Shared FindingRow + FindingMetrics components

**Files:**
- Create: `crates/rupu-cp/web/src/components/findings/FindingRow.tsx`, `crates/rupu-cp/web/src/components/findings/FindingMetrics.tsx`
- Modify: `crates/rupu-cp/web/src/pages/CoverageDetail.tsx` (use the shared components), `crates/rupu-cp/web/src/lib/api.ts` (add `FindingsSummary` type if not added in Task 5 — coordinate; define it here and reuse)
- Test: colocated `FindingMetrics.test.tsx`

**Context:** `CoverageDetail.tsx` has a local `FindingRow` (severity pill via `SEVERITY_STYLE` from `lib/severity.ts`, summary, `file:line` location, concern chip, collapsible evidence). Lift it verbatim into the shared component, adding an optional provenance chip (`project?`, `targetId?`). `lib/severity.ts` exports `SEVERITY_STYLE: Record<Severity, {text,bg,ring,bar,label,pill}>`. `Severity = 'info'|'low'|'medium'|'high'|'critical'`.

- [ ] **Step 1: Write the failing test** for `FindingMetrics`: render with a `summary` of known counts → the six tiles (Total/Critical/High/Medium/Low/Info) show the right numbers; when `onSelect` is provided, clicking a tile fires it with the matching severity (and the Total tile fires `null`).
- [ ] **Step 2: Run it, confirm failure** (component not yet created).
- [ ] **Step 3: Implement.** `FindingRow.tsx`: the lifted Okesu-style row + optional `[project · target]` chip. `FindingMetrics.tsx`: tile strip Total·Critical·High·Medium·Low·Info, accent per `SEVERITY_STYLE` (static classes only), props `{ summary, active?, onSelect? }` (click-to-filter only when `onSelect` set). Define `FindingsSummary` type in `lib/api.ts` and import it. Update `CoverageDetail.tsx` to import `FindingRow` (delete its local copy) and render `<FindingMetrics summary={…} />` above the findings section (compute the summary from the findings it already loads).
- [ ] **Step 4: `npm test -- --run FindingMetrics` + `npm run build`** (strict) — pass + exit 0.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/components/findings/ crates/rupu-cp/web/src/pages/CoverageDetail.tsx crates/rupu-cp/web/src/lib/api.ts` → `feat(cp/web): shared FindingRow + FindingMetrics, reuse on Coverage detail`.

---

### Task 5: Findings page + nav + route + API client

**Files:**
- Create: `crates/rupu-cp/web/src/pages/Findings.tsx`
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (`getFindings` + `FindingsResponse`/`FindingOut` types), `crates/rupu-cp/web/src/lib/sidebarNav.ts`, `crates/rupu-cp/web/src/App.tsx`

**Context:** Depends on Tasks 3 (endpoint) + 4 (shared components, `FindingsSummary`). Nav `Observe` group is in `sidebarNav.ts` (Live Events, Coverage). Routes are lazy in `App.tsx`. `lib/api.ts` is the typed client; `normFindingSeverity`/`sevRank` already exist.

- [ ] **Step 1: Add the API client.** In `lib/api.ts`: `FindingOut` (wire `FindingRecord` fields + `ws_id`/`project`/`target_id`), `FindingsResponse { findings: FindingOut[]; summary: FindingsSummary }`, `async getFindings(): Promise<FindingsResponse>`. (No `any`.)
- [ ] **Step 2: Build `Findings.tsx`.** Fetch via `getFindings`; render `<FindingMetrics summary active onSelect />` (click-to-filter by severity) over the severity-ordered `<FindingRow />` list (rows carry the `[project · target]` chip). Standard loading/error/empty states (match Coverage's patterns). Client-side filter on the active tile.
- [ ] **Step 3: Wire nav + route.** Add `{ to: '/findings', label: 'Findings', icon: ShieldAlert, enabled: true }` to the `Observe` group (import `ShieldAlert` from lucide-react). Add the lazy `/findings` route in `App.tsx`.
- [ ] **Step 4: `npm test -- --run` (full) + `npm run build`** (strict) — green + exit 0; verify `grep -c recharts dist/assets/index-*.js` = 0 and the main chunk stays lean.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/pages/Findings.tsx crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/lib/sidebarNav.ts crates/rupu-cp/web/src/App.tsx` → `feat(cp/web): global Findings page (Observe nav)`.

---

### Final verification (after all tasks)
- `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` clean.
- `npm test -- --run` full suite green; `npm run build` strict exit 0; recharts not in main chunk.
- Dispatch a final reviewer over the whole branch diff (spec compliance + quality), then hand to matt for visual validation before merge.
