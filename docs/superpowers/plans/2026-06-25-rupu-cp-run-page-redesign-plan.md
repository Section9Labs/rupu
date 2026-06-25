# Run Detail Page Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Make the graph + usage chart persistent chrome on the run page; a Transcript/Events/Findings tab panel below that follows the selected step; replace the for_each right-slide-over with a units-left/transcript-right file-browser; and fix per-run findings to include fan-out sub-run findings.

**Spec:** `docs/superpowers/specs/2026-06-25-rupu-cp-run-page-redesign-design.md`

**Constraints (every task):** read-adapter (no `rupu-cli` dep in `rupu-cp`); no `any` in TS; static Tailwind only; recharts stays out of the main chunk (`grep -c recharts dist/assets/index-*.js` = 0); stage only specific changed files (`git add <paths>`, never `-A`, never `.rupu/*`); never package-wide `cargo fmt`. Frontend dir `crates/rupu-cp/web`; verify each task with `npm test -- --run` + `npm run build` (strict). Worktree Rust is 1.95 — `rupu-cp` clean there; ignore `rupu-cli` red baseline.

---

### Task 1: Backend — per-run findings include fan-out sub-run findings

**Files:** Modify `crates/rupu-cp/src/api/findings.rs`.

**Context:** `GET /api/findings?run_id=<id>` currently keeps findings where `declared_by.run_id == id`. Findings reported inside a `for_each` unit carry the UNIT's sub-run id, not the parent run id, so they're dropped. Fix: match the parent run id PLUS the run's fan-out unit sub-run ids. The parent run's unit checkpoints (each `UnitCheckpoint` has its own `run_id`) are how you discover the sub-run ids — see how `crates/rupu-cp/src/api/graph.rs` reads unit checkpoints for a run (via `s.run_store`), and reuse that reader.

- [ ] **Step 0:** Read `crates/rupu-cp/src/api/graph.rs` to find how it loads a run's unit checkpoints (the `UnitCheckpoint { step_id, index, run_id, ... }` records) from `RunStore`. Note the exact `RunStore` method / file (`unit_checkpoints.jsonl`).
- [ ] **Step 1: Write the failing test.** In `findings.rs` tests, exercise the run-id resolution: given a parent run id and a set of unit-checkpoint sub-run ids, the matched findings = those whose `declared_by.run_id` is the parent OR any unit sub-run id. Factor the matching into a pure helper if it isn't already (e.g. `fn run_id_set(parent: &str, unit_run_ids: &[String]) -> HashSet<String>` + filter), and unit-test it: a finding under a unit sub-run id is included for the parent run; an unrelated run's finding is excluded.
- [ ] **Step 2: Run `cargo test -p rupu-cp findings`, confirm failure.**
- [ ] **Step 3: Implement.** When `q.run_id` is `Some(parent)`: load that run's unit checkpoints (reuse graph.rs's reader; tolerate missing → empty), build the id set `{parent} ∪ {each unit.run_id}`, and keep findings whose `declared_by.run_id` is in the set. Summary over the matched set. (The handler already loops workspaces/targets to gather findings; this only changes the run_id filter step — keep `ws_id`/`workflow` filters intact and AND-combined.) One level of fan-out is sufficient (note nested fan-out as an unhandled edge in a comment).
- [ ] **Step 4: Run `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets`, confirm green/clean.**
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/src/api/findings.rs` → `fix(cp): per-run findings include for_each sub-run findings`.

---

### Task 2: Frontend — for_each file-browser component

**Files:**
- Create: `crates/rupu-cp/web/src/components/run/StepTranscriptBrowser.tsx`
- Test: `crates/rupu-cp/web/src/components/run/StepTranscriptBrowser.test.tsx`

**Context:** This replaces the `FanoutDrill` slide-over with an in-panel master-detail. Read `crates/rupu-cp/web/src/components/FanoutDrill.tsx` first — reuse its state-filter pills (all/running/done/failed with counts), `STATE_STYLE` glyphs, `ROW_CAP`/"+N more" cap, and unit-row rendering. The unit data is `UnitView { index, key, state, transcriptPath? }` from `runGraphModel.ts` (`model.nodeById(stepId).fanout.units`). `crates/rupu-cp/web/src/components/TranscriptPanel.tsx` renders a transcript (`{ path, live, label? }`).

- [ ] **Step 1: Write the failing test.** Render `<StepTranscriptBrowser stepId="scan-deps" units={[…mixed states…]} />`; assert the left column lists the units; clicking a unit row shows that unit's transcript on the right (mock/stub `TranscriptPanel` to assert it received the unit's `transcriptPath`); clicking a state-filter pill (e.g. "failed") narrows the left list.
- [ ] **Step 2: Run it, confirm failure.**
- [ ] **Step 3: Implement.** `StepTranscriptBrowser({ stepId: string; units: UnitView[] })`: a two-column flex — LEFT (~34%): the state-filter pills (reuse FanoutDrill's logic) over a scrollable unit list (glyph + `key` + state, `ROW_CAP`/"+N more"); RIGHT (~66%): `<TranscriptPanel path={selectedUnit.transcriptPath} live label={selectedUnit.key} />` or an empty-state until a unit is picked (auto-selecting the first unit is fine). Local state for the selected unit index + the active filter. Static Tailwind only; no `any`.
- [ ] **Step 4: `npm test -- --run StepTranscriptBrowser` + `npm run build`** (strict) — pass + exit 0.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/components/run/StepTranscriptBrowser.tsx crates/rupu-cp/web/src/components/run/StepTranscriptBrowser.test.tsx` → `feat(cp/web): for_each file-browser (units left, transcript right)`.

---

### Task 3: Frontend — RunDetail rewrite (always-on chrome + scoped tabs)

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/RunDetail.tsx`
- Delete: `crates/rupu-cp/web/src/components/FanoutDrill.tsx`
- Test: `crates/rupu-cp/web/src/pages/RunDetail.test.tsx` (extend if exists, else create)

**Context:** Read the current `RunDetail.tsx` fully. Today: header + usage timeline + a page-level `TabBar` (Graph/Events/Findings) where the Graph tab is `RunGraph` over a `TranscriptPanel`, plus a `FanoutDrill` overlay keyed on `drillStepId`. Depends on Task 2's `StepTranscriptBrowser`. Keep using `getRunGraph`/`subscribeRunLog`/`getRunUsageTimeline`/`getFindings`. `RunGraph` exposes node-click + for_each expand/open-unit callbacks (it already sets `sel`/`drillStepId`). `RunEventFeed` takes the events array; events carry `step_id` on step/unit events. `FindingMetrics` + `FindingRow` for findings.

- [ ] **Step 1: Write the failing test.** Render RunDetail (mock `getRunGraph` with a for_each step + a normal step, `getFindings`, stub the SSE). Assert: the graph and the "Token usage by turn" chart are BOTH rendered regardless of the active tab (persistent chrome). Selecting a normal step → Transcript tab shows that step's transcript. Selecting the for_each step → the `StepTranscriptBrowser` (file-browser) renders. The Events tab content filters to the selected step. (Use the existing RunDetail/router test idiom.)
- [ ] **Step 2: Run it, confirm failure.**
- [ ] **Step 3: Rewrite RunDetail.** Structure: header (unchanged) → **`RunGraph` (always rendered)** → **`RunUsageTimeline` (always rendered)** → a `TabBar` with **Transcript · Events · Findings** → the active tab body scoped to a single `selected` cursor (`{ stepId } | { stepId, unitIndex } | null`, driven by the graph's node/unit click callbacks — unify the old `sel`/`drillStepId` into this one cursor):
  - **Transcript:** if `selected` is a for_each step → `<StepTranscriptBrowser stepId units={model.nodeById(stepId).fanout.units} />`; else if a normal step → `<TranscriptPanel path={stepResult.transcript_path} live />`; else → "Select a step in the graph to view its transcript."
  - **Events:** `<RunEventFeed events={selected ? events.filter(e => e.step_id === selected.stepId) : events} connection=... />`. (Run-level events with no `step_id` are excluded when a step is selected.)
  - **Findings:** run-wide — `getFindings({ runId: id })` → `<FindingMetrics summary />` + `<FindingRow>` list (with provenance chips). Lazy-load on first open (keep the existing ref-guarded fetch). Empty → "No findings recorded for this run." (+ hint that findings need a coverage target). The tab badge shows the count.
  Remove the old page-level Graph tab and the `FanoutDrill` overlay/mount. No `any`; static Tailwind.
- [ ] **Step 4: Delete `FanoutDrill.tsx`** (`git rm crates/rupu-cp/web/src/components/FanoutDrill.tsx`) and confirm nothing else imports it (grep).
- [ ] **Step 5: `npm test -- --run` (full) + `npm run build`** (strict) — green + exit 0; `grep -c recharts dist/assets/index-*.js` = 0; report main chunk size.
- [ ] **Step 6: Commit.** `git add crates/rupu-cp/web/src/pages/RunDetail.tsx crates/rupu-cp/web/src/pages/RunDetail.test.tsx` + the `git rm` → `feat(cp/web): always-on graph + usage; step-scoped Transcript/Events/Findings tabs; for_each file-browser`.

---

### Final verification (after all tasks)
- `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` clean.
- `npm test -- --run` full green; `npm run build` strict exit 0; recharts out of main chunk; no dangling `FanoutDrill` import.
- Final whole-branch review (selection cursor wiring, the events filter, the findings run-id-set fix end-to-end), then hand to matt for visual validation.
