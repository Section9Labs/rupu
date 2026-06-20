# CP Slice A.1 — bug-fixes + cheap wins — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the bugs + land the cheap wins surfaced by matt's visual pass of Slice A — the transcript-loading bug (centerpiece), the empty firehose Coverage page, the useless Autoflow-cycles page, slow Projects load, thin project-coverage, live-events ordering, and per-project definitions.

**Architecture:** Targeted fixes/extensions on top of the Slice A branch. Backend stays a read adapter (`rupu-cp`); each fix is grounded in a diagnosed root cause (file:line below). No new design — these implement the fix approaches matt approved.

**Tech Stack:** Rust (axum, rupu-coverage, rupu-runtime, rupu-agent); React 18 + TypeScript + Vite.

**Branch:** `feat-cp-slice-a1-fixes` (stacks on #322 → #321 → #320 → #319). Rebase onto `main` as the stack merges.

**Source:** matt's visual-pass feedback + the diagnostic investigation (root causes inline per task).

---

## PART A — The transcript-loading bug (#3, top priority)

### Task 1: Make fan-out unit transcripts selectable

**Root cause:** clicking a `for_each` step body is suppressed (`RunGraph.tsx:118` early-returns on `for_each`), and large fan-outs (>12 units) have NO clickable squares — only `FanoutDrill`, whose rows render `transcriptPath` as plain text (`FanoutDrill.tsx:123-138`) and never call `onSelectNode`. So no unit transcript can be selected for big fan-outs → the pane stays on the seeded default ("only the first loads"). The unit paths are already in the model (`runGraphModel.ts:159`).

**Files:** Modify `crates/rupu-cp/web/src/components/FanoutDrill.tsx`, `src/pages/RunDetail.tsx`, `src/components/RunGraph.tsx`.

- [ ] **Step 1: thread a select callback into FanoutDrill.** Give `FanoutDrill` a prop `onSelectUnit: (sel: { path: string | null; live: boolean; label: string }) => void`. Make each unit `<li>` a `<button type="button" onClick={() => onSelectUnit({ path: u.transcriptPath ?? null, live: u.state === 'running', label: u.key })} className="... w-full text-left hover:bg-slate-50 cursor-pointer">` (keep the existing glyph · key · state · path layout; the path can stay shown as a hint). Keep the "+N more" cap.
- [ ] **Step 2: wire RunDetail.** Where `RunDetail.tsx` renders `<FanoutDrill ... />` (the drill open-state), pass `onSelectUnit={setSel}` (the same setter used by `onSelectNode`). Confirm `NodeSelection`/the sel shape matches (`{path, live, label}`).
- [ ] **Step 3: small fan-out parity.** Confirm the inline unit squares (`FanoutNode.tsx`, ≤12 path) ALSO emit a selection (they call `onOpenUnit` → `handleOpenUnit` → `onSelectNode` per the diagnosis — verify; if a square click only drills and doesn't select, make it also emit the selection). 
- [ ] **Step 4: test + build.** Extend a frontend test if cheap (e.g. a `FanoutDrill` render test asserting a row click calls `onSelectUnit` with the unit's path); otherwise rely on `npm run build` (strict) + `npm test -- --run`. Paste lines.
- [ ] **Step 5: commit** `fix(cp/web): make fan-out unit transcripts selectable (large + inline)`.

---

### Task 2: Make panel panelist/fixer transcripts reachable

**Root cause:** a `panel` node only exposes the aggregate `step_results[].transcript_path` (one file); the panelist/fixer transcripts are emitted as `unit_started` events (`UnitStarted.transcript_path`) and for a COMPLETED run live only in `events.jsonl` (NOT in `step_results.jsonl`/`unit_checkpoints.jsonl` — the runner keeps only the final iteration's panelists as ItemResults, and never the fixer). `PanelLoopNode.tsx` has no unit affordance, and the graph endpoint's `units` array (from `read_unit_checkpoints`) is empty for panels → those transcripts are unreachable on reload.

**Files:** Modify `crates/rupu-cp/src/api/graph.rs` (harvest panel units from events.jsonl), `crates/rupu-cp/web/src/lib/runGraphModel.ts`, `src/components/graph/PanelLoopNode.tsx`.

- [ ] **Step 1: backend — surface panel units from events.jsonl.** In `graph.rs`'s `run_graph` handler, in addition to `read_unit_checkpoints`, read the run's `events.jsonl` (via `RunStore::events_path` + the existing `JsonlReader`-style read, or `read_events`) and harvest `Event::UnitStarted { step_id, index, unit_key, transcript_path, .. }` rows into the `units` array (merge with checkpoints; dedupe by `(step_id, index)`). This makes panel panelist/fixer (and any) unit transcript paths available for completed runs. Confirm the `units` JSON shape stays `{ step_id, index, item/unit_key, transcript_path, success?, ... }` the frontend already consumes. Add a backend test: a run whose `events.jsonl` has 2 `UnitStarted` for a panel step → `/api/runs/{id}/graph` `units` includes them with their transcript_path.
- [ ] **Step 2: frontend — fold panel units into the model.** Ensure `buildRunGraphModel` attaches these units to the panel node's `fanout.units` (the `unit_started`/checkpoint folding at `runGraphModel.ts:199-228` + `:158` should already handle them if the `units`/events carry the panel `step_id` — verify; extend if panel units aren't folded). A model test: a panel node with units → `nodeById(panelId).fanout.units` populated.
- [ ] **Step 3: frontend — PanelLoopNode clickable units.** Render the panel node's `fanout.units` (if any) as clickable chips (panelist/fixer), each emitting `onSelectNode({ path: u.transcriptPath ?? null, live: u.state === 'running', label: u.key })` — mirror `FanoutNode`'s square click. Keep the gate/round header.
- [ ] **Step 4: build + tests** (`npm run build` strict + `npm test -- --run`; `cargo test -p rupu-cp` for the backend test; `cargo clippy -p rupu-cp --all-targets`). Paste lines.
- [ ] **Step 5: commit** `fix(cp): reach panel panelist/fixer transcripts (harvest events.jsonl units + clickable chips)`.

---

## PART B — Empty/broken pages

### Task 3: Firehose Coverage is registry-driven (#6)

**Root cause:** `GET /api/coverage` (`coverage.rs:32-56`) + the dashboard tile (`dashboard.rs:142`) scope coverage to `state.workspace_dir` = the CP launch dir (`state.rs:17` `current_dir()`), not the registered workspaces. Coverage lives per-project under each workspace path → the global page is empty when the CP isn't launched in the assessed dir.

**Files:** Modify `crates/rupu-cp/src/api/coverage.rs`, `src/api/dashboard.rs`. Test `tests/`.

- [ ] **Step 1: failing test** — seed 2 workspaces in the registry, each with a coverage target under its `path/.rupu/coverage/`; assert `GET /api/coverage` returns targets from BOTH (not just `workspace_dir`), each tagged with its `ws_id`/project name.
- [ ] **Step 2: implement** — rewrite `list_coverage` to iterate `WorkspaceStore { root: global_dir/workspaces }.list()`; for each workspace, `discover_targets(Path::new(&w.path))` + `CoveragePaths::new(&w.path, &t.target_id)` + `read_findings`; aggregate the per-target `CoverageSummary` rows across all workspaces. Add `ws_id` + `project` (name) to `CoverageSummary` (and the TS `CoverageSummary` in `api.ts`) so the page can group by project. Apply the same registry iteration to `get_coverage` (the per-target detail — locate the owning workspace, or accept a `ws_id` query param) and to the dashboard coverage tile (`dashboard.rs:142`).
- [ ] **Step 3: frontend** — the existing global Coverage page (`pages/Coverage.tsx`) groups rows by project (use the new `project`/`ws_id`); a target row links to its project's coverage detail. (Minimal — the deep grid is Slice B.)
- [ ] **Step 4: tests + build** (`cargo test -p rupu-cp`, clippy, web build). Commit `fix(cp): firehose Coverage aggregates all registered workspaces`.

---

### Task 4: Autoflows page lists launched runs, not opaque ticks (#7)

**Root cause:** `/api/runs/autoflows` returns batch *cycle* records; the DTO (`run_streams.rs:31-61`) drops the autoflow `workflow`/`issue_display_ref`/`kind` and keeps only `run_ids`; idle ticks (ran 0) dominate; rows aren't links and the prominent chip is the `mode` ("serve"/"tick"). The richer data is in `AutoflowCycleEvent` (`autoflow_history.rs:46-47`) and `AutoflowHistoryStore::list_recent_events` (`autoflow_history.rs:262`) already returns flat newest-first events with `kind, run_id, workflow, issue_display_ref, status`.

**Files:** Modify `crates/rupu-cp/src/api/run_streams.rs`, `crates/rupu-cp/web/src/pages/runs/AutoflowRuns.tsx`, `src/lib/api.ts`.

- [ ] **Step 1: backend — events endpoint.** Add `GET /api/runs/autoflows/events` → `AutoflowEventRow { event_id, cycle_id, at, kind, workflow: Option<String>, issue_display_ref: Option<String>, run_id: Option<String>, status: Option<String>, worker_name: Option<String> }` from `AutoflowHistoryStore::list_recent_events(<limit, e.g. 200>)`, filtered to launch-relevant kinds (`RunLaunched`, optionally `AwaitingHuman`/`AwaitingExternal`/`CycleFailed`). Backend test: seed a cycle with a `RunLaunched` event → the endpoint returns a row with the workflow name + run_id.
- [ ] **Step 2: frontend — rework AutoflowRuns.** Call `getAutoflowEvents()`; render one clickable row per launched run: workflow name (headline) + issue_display_ref + a status badge + relative `at`, wrapped in `<Link to={/runs/${run_id}}>` when `run_id` is present; demote `mode`/cycle-batch info. Keep the old cycle list behind a secondary "scheduling activity" toggle OR drop it (your call — lead with the launched-runs view). Fix the `MODE_CLS` map to include `tick`/`serve` keys if the mode chip stays.
- [ ] **Step 3: api.ts** — add `AutoflowEventRow` + `getAutoflowEvents()`; keep `getAutoflowRuns()` (cycle view) if the secondary toggle stays.
- [ ] **Step 4: tests + build.** Commit `fix(cp): Autoflows lists launched runs (clickable) instead of opaque ticks`.

---

### Task 5: Project coverage — findings + per-file list (#2)

**Root cause:** `project_coverage` (`projects.rs:245-264`) computes `read_findings` but keeps only `.len()`; no detail page. The rich data (`read_findings -> Vec<FindingRecord>`, `file_views(read_file_events) -> Vec<FileView>`) is one call away.

**Files:** Modify `crates/rupu-cp/src/api/projects.rs`; create `crates/rupu-cp/web/src/pages/ProjectCoverageDetail.tsx`; modify `src/pages/ProjectCoverage.tsx`, `src/lib/api.ts`, `src/App.tsx`.

- [ ] **Step 1: backend detail endpoint.** Add `GET /api/projects/:ws_id/coverage/:target` → `{ target_id, findings: Vec<FindingRecord>, files: Vec<FileView> }` via `read_findings(&paths)` (full records) + `file_views(&read_file_events(&paths)?)`. Import `read_file_events, file_views` from `rupu_coverage`. `load_workspace` + `CoveragePaths::new(wp, &target)` like the existing handler. Tolerate missing files (the readers return empty vecs). Backend test: seed a target with `findings.jsonl` (a couple findings) + `files.jsonl` → the endpoint returns the findings (with severity/file/summary) + file views.
- [ ] **Step 2: api.ts** — `FindingRecord { id, summary, severity, file_path?, line_range?, concern_id?, evidence?, scope }` + `FileView { path, strongest, read_lines, grep_matches, edits, last_at }` (loose where needed) + `getProjectCoverageDetail(wsId, target)`.
- [ ] **Step 3: ProjectCoverageDetail page** (route `/projects/:wsId/coverage/:target`): **Findings list first** — one row per finding, severity-colored (Okesu ramp: critical/high/medium/low/info → the `sev.*` palette), `summary`, `file_path:line_range`, `concern_id`, expandable evidence. Then a **Files** list — `path`, strongest touch, edits/grep counts, last-touched. Make the `ProjectCoverage.tsx` summary rows clickable `<Link>`s into this page.
- [ ] **Step 4: build + tests.** Commit `feat(cp): project coverage detail (findings + file heatmap)`.

---

## PART C — Definitions + ordering + perf

### Task 6: Per-project definitions (#8)

**Root cause/opportunity:** Build endpoints are global-only, but `rupu_agent::loader::load_agents(global, Some(project))` already supports project scope (project shadows global by name); the workflow/autoflow scans just need a project dir.

**Files:** Modify `crates/rupu-cp/src/api/projects.rs` (+ make the `AgentDto`/scan helpers reusable from `api/{agents,workflows,autoflows}.rs`); create `crates/rupu-cp/web/src/pages/ProjectDefinitions.tsx`; modify `src/lib/api.ts`, `src/pages/ProjectDetail.tsx`, `src/App.tsx`.

- [ ] **Step 1: backend endpoints.** Add to `projects.rs`: `GET /api/projects/:ws_id/agents` → `load_agents(&s.global_dir, Some(Path::new(&w.path)))` mapped to the existing `AgentDto` (make it `pub(crate)`), each tagged `scope: "project"|"global"` (project = a `<project>/.rupu/agents/<name>.md` exists). `GET /api/projects/:ws_id/workflows` + `.../autoflows` → factor the dir-scan in `api/workflows.rs`/`api/autoflows.rs` into `pub(crate) fn scan_workflows(dir, scope)` / the autoflow-enabled filter, call for global + `<project>/.rupu/workflows`, merge with project shadowing global by name. Backend test: seed a project-local `<project>/.rupu/agents/foo.md` + a global `bar.md` → `/api/projects/{id}/agents` returns both, `foo` tagged project.
- [ ] **Step 2: api.ts** — `getProjectAgents/Workflows/Autoflows(wsId)` + a `scope` field on the rows.
- [ ] **Step 3: frontend** — a "Definitions" section/page on the project (`ProjectDefinitions.tsx`, route `/projects/:wsId/definitions`, tabs Agents/Workflows/Autoflows, each row badged project/global), linked from ProjectDetail. (Leave the global Build views as-is; project-scoped lives on the project page.)
- [ ] **Step 4: build + tests.** Commit `feat(cp): per-project definitions (agents/workflows/autoflows)`.

---

### Task 7: Live Events — newest-first + Okesu-style ticking timeline (#5)

**Files:** Modify `crates/rupu-cp/web/src/pages/Events.tsx`, `src/components/EventTimelineList.tsx`.

- [ ] **Step 1:** render events **newest-first** (reverse the accumulation order, or prepend new SSE events to the top). Auto-scroll behavior: when following, new events appear at the top (no scroll needed); keep a "follow"/"paused" affordance.
- [ ] **Step 2:** apply the Okesu timeline treatment to `EventTimelineList` — a vertical timeline with a connecting spine + per-event dot/glyph + a subtle enter animation ("ticks") as new events arrive (reuse the `.timeline-*` CSS classes already vendored from Okesu in `styles.css`; add a fade/slide-in keyframe for new rows if not present). Keep it tasteful.
- [ ] **Step 3: build + tests** (`npm run build` strict + `npm test -- --run`). Commit `feat(cp/web): live events newest-first + ticking timeline`.

---

### Task 8: Projects load perf — defer the audit + scope the run scan (#1 backend)

**Root cause:** `get_project` (`projects.rs:115-198`) reads+parses ALL runs (`run_store.list()`, `runs.rs:706`, no limit) and runs `run_audit` per coverage target (uncached, glob-join heavy) on every overview load.

**Files:** Modify `crates/rupu-cp/src/api/projects.rs`.

- [ ] **Step 1: drop the audit from the hot path.** Remove the `run_audit`-per-target loop from `get_project`'s coverage rollup; return `coverage: { targets, findings }` (counts only — both cheap) and OMIT `assessed_pct` from the synchronous rollup. Update the `ProjectDetail` TS type: `coverage.assessed_pct` moves to a separate lazy fetch.
- [ ] **Step 2: a lazy assessed-% endpoint.** Add `GET /api/projects/:ws_id/coverage/assessed` → the aggregated `run_audit` `complete/total` → `{ assessed_pct: number | null }`. (This isolates the heavy audit so the overview paints immediately.)
- [ ] **Step 3: frontend — parallel fetch.** `ProjectDetail.tsx` renders runs/sessions/coverage-count tiles from `getProject(wsId)` immediately, and fetches `getProjectAssessedPct(wsId)` in PARALLEL, filling the Coverage tile's % when it resolves (show a small spinner/"…" until then). Add `getProjectAssessedPct` to api.ts.
- [ ] **Step 4 (optional, note if skipped): scope the run scan.** If `run_store.list()` parsing all runs is still slow, add a bounded/workspace-scoped read to `RunStore` (a follow-up; the audit removal is the bigger win). Note explicitly if deferred.
- [ ] **Step 5: tests + build.** Commit `perf(cp): defer coverage audit out of project rollup (lazy assessed-%)`.

---

### Task 9: Bundle code-splitting (#1 frontend)

**Root cause:** one 873 KB JS chunk (no `manualChunks`, no `React.lazy`) — every page pays the full parse on first load.

**Files:** Modify `crates/rupu-cp/web/vite.config.ts`, `src/App.tsx`.

- [ ] **Step 1: lazy routes.** In `App.tsx`, convert the heavy page components to `const X = React.lazy(() => import('./pages/X'))` (at least the graph/run-detail, charts/Dashboard, coverage, and the transcript pages — the ones pulling `@xyflow/react`, `recharts`, `@dagrejs/dagre`, CodeMirror). Wrap the `<Routes>` in `<Suspense fallback={<…spinner…/>}>`.
- [ ] **Step 2: manualChunks.** In `vite.config.ts` add `build.rollupOptions.output.manualChunks` splitting big vendors into their own chunks (e.g. `{ xyflow: ['@xyflow/react','@dagrejs/dagre'], charts: ['recharts'], react: ['react','react-dom','react-router-dom'] }`).
- [ ] **Step 3: verify.** `npm run build` → confirm the main chunk shrinks substantially (multiple chunks, main well under the prior 873 KB) + the chunk-size warning is gone or much reduced. `npm test -- --run` still passes. Paste the build chunk summary.
- [ ] **Step 4: commit** `perf(cp/web): code-split routes + vendor chunks`.

---

## Self-review

**Coverage of matt's items:** #3 transcript loading ✓ (T1 fan-out + T2 panel); #6 firehose coverage ✓ (T3); #7 autoflow runs ✓ (T4); #2 project coverage findings/files ✓ (T5); #8 per-project definitions ✓ (T6); #5 live-events order/timeline ✓ (T7); #1 perf ✓ (T8 backend defer + T9 code-split). (#4 transcript rendering = Slice A.2; #9 cost/tokens = Slice A.3 — out of scope here, as agreed.)

**Placeholder scan:** the few "verify/confirm" steps (T1 small-fan-out parity, T2 panel-unit folding) are targeted checks with the exact file:line to confirm + the fix if the check fails — not hand-waves. Backend fixes have concrete readers named (`read_findings`/`file_views`/`list_recent_events`/`discover_targets`/`load_agents`).

**Type consistency:** `NodeSelection {path, live, label}` shared T1↔T2; `CoverageSummary` gains `ws_id`/`project` consistently T3 (backend + api.ts); `assessed_pct` removed from the sync `ProjectDetail` and added as `getProjectAssessedPct` T8 (frontend + backend agree); `FindingRecord`/`FileView` types T5 (api.ts ↔ backend JSON); `AutoflowEventRow` T4 (api.ts ↔ run_streams.rs).

**Notes for the executor:** rupu rules — `rupu-cp` stays a read adapter (no rupu-cli dep); workspace dep versions in root Cargo.toml; `#![deny(clippy::all)]` incl `--all-targets`; never package-wide `cargo fmt`. The web UI is matt-validated. Tasks are largely independent — recommended order: T1, T2 (the critical bug) → T3, T4, T5, T6 (backend-led pages) → T7 (events) → T8, T9 (perf). Stacks on #322.
