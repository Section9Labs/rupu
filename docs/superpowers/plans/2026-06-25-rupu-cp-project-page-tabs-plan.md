# Tabbed Project Detail Page — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Turn `/projects/:wsId` into a tabbed page (persistent header + tiles, then Overview/Runs/Findings/Sessions/Coverage tabs), absorbing the placeholder sub-pages and adding per-tab filtering.

**Spec:** `docs/superpowers/specs/2026-06-25-rupu-cp-project-page-tabs-design.md`

**Architecture:** Frontend-only. `ProjectDetail.tsx` becomes a shell that renders the identity header + 5 rollup tiles + a `TabBar`, and swaps in one of five tab-body components based on a `tab` prop set per-route. Each non-overview tab body owns its own data fetch + client-side filters. No backend changes.

**Constraints (every task):** no `any` in TS; static Tailwind only (severity via `SEVERITY_STYLE`; filter chips are static toggle classes — no dynamic class strings); recharts must stay out of the main chunk (`grep -c recharts dist/assets/index-*.js` = 0 after build; `UsageBarChart` is already lazy-chunked — keep it that way); stage only the specific changed files (`git add <paths>`, never `-A`, never `.rupu/*`). Frontend dir: `crates/rupu-cp/web`. Verify each task with `npm test -- --run` + `npm run build` (strict, exit 0).

**Reusable building blocks (already exist):**
- `components/TabBar.tsx` → `<TabBar>` + `<TabButton active onClick icon label>` (pill style; RunDetail uses it).
- `components/lists/{ListCard,SectionHeader,MetricRow}.tsx`; `components/StatusPill.tsx`; `components/charts/UsageBarChart.tsx`.
- `components/findings/{FindingMetrics,FindingRow}.tsx` — `FindingMetrics({summary, active?, onSelect?})` (interactive when `onSelect` given), `FindingRow({finding, project?, targetId?})`.
- API (`lib/api.ts`): `getProject(wsId)` (bundle: `project`, `runs`, `sessions`, `coverage`, `recent_runs: RunListRow[]`, `usage`), `getProjectRuns(wsId,{offset,limit}) → RunListRow[]`, `getProjectSessions(wsId,{offset,limit}) → SessionSummary[]`, `getProjectCoverage(wsId) → ProjectCoverageRow[]`, `getFindings({wsId}) → {findings, summary}`.
- Existing bodies to refactor FROM: `pages/ProjectRuns.tsx`, `pages/ProjectSessions.tsx`, `pages/ProjectCoverage.tsx`, and the current findings + summary sections inside `pages/ProjectDetail.tsx`.

---

### Task 1: Runs / Sessions / Coverage tab bodies (with filters) + lift TriggerChip

**Files:**
- Create: `crates/rupu-cp/web/src/components/project/ProjectRunsTab.tsx`, `.../ProjectSessionsTab.tsx`, `.../ProjectCoverageTab.tsx`
- Create: `crates/rupu-cp/web/src/components/TriggerChip.tsx` (lift the local `TriggerChip` + `TRIGGER_CHIP_CLS` out of `ProjectDetail.tsx`)
- Test: `crates/rupu-cp/web/src/components/project/ProjectRunsTab.test.tsx`

**Context:** These three are extracted from the like-named placeholder pages (`pages/ProjectRuns.tsx` etc.), which already do the paginated fetch + `MetricRow`/`UsageBarChart` rendering. Each tab body takes `{ wsId: string }` and self-fetches. DO NOT delete the placeholder pages in this task (Task 3 removes their routes) — just create the new components. The Runs and Sessions tabs add client-side filter chips over the loaded pages.

- [ ] **Step 1: Lift `TriggerChip`.** Move `TriggerChip` + `TRIGGER_CHIP_CLS` from `ProjectDetail.tsx` into `components/TriggerChip.tsx` (export the component; keep classes static). (Leave `ProjectDetail.tsx` importing it — but since Task 3 rewrites ProjectDetail, you may leave ProjectDetail's local copy for now OR update its import; simplest: create the shared one; Task 3 will wire ProjectDetail to it.)
- [ ] **Step 2: Write the failing Runs-filter test.** In `ProjectRunsTab.test.tsx`, mock `api.getProjectRuns` to return rows of mixed `status` and `trigger`, render `<ProjectRunsTab wsId="x" />`, assert all rows show; click the "Running" status chip → only running rows remain; click a trigger chip → narrows further; clicking the active chip again clears it.
- [ ] **Step 3: Run it, confirm failure** (`npm test -- --run ProjectRunsTab`).
- [ ] **Step 4: Implement `ProjectRunsTab`.** Port `ProjectRuns.tsx`'s fetch/load-more/render. Add a chip row: **status** (All/Running/Completed/Failed over `RunListRow.status`) + **trigger** (All/Manual/Event/Cron over `RunListRow.trigger`). Chips are static-Tailwind toggle buttons (active = filled, inactive = muted — mirror the lifecycle-chip style in `pages/runs/WorkflowRuns.tsx`). Filter is applied client-side to the loaded rows before rendering. No `any`.
- [ ] **Step 5: Implement `ProjectSessionsTab`.** Port `ProjectSessions.tsx`. Add a **scope** chip row (All/Active/Archived over the session's scope/status, using the page's existing `sessionStatusDot/Label` helpers). Same chip style.
- [ ] **Step 6: Implement `ProjectCoverageTab`.** Port `ProjectCoverage.tsx` verbatim (no filter). `{ wsId }` prop, self-fetch `getProjectCoverage`.
- [ ] **Step 7: Run `npm test -- --run` (full) + `npm run build` (strict).** Green + exit 0; recharts grep = 0.
- [ ] **Step 8: Commit.** `git add crates/rupu-cp/web/src/components/project/ crates/rupu-cp/web/src/components/TriggerChip.tsx <test>` → `feat(cp/web): project runs/sessions/coverage tab bodies with filters`.

---

### Task 2: Findings + Overview tab bodies

**Files:**
- Create: `crates/rupu-cp/web/src/components/project/ProjectFindingsTab.tsx`, `.../ProjectOverviewTab.tsx`
- Test: `crates/rupu-cp/web/src/components/project/ProjectFindingsTab.test.tsx`

**Context:** `ProjectFindingsTab` is the current findings section of `ProjectDetail.tsx` plus interactive severity filtering. `ProjectOverviewTab` is today's "at a glance" content (recent runs list + coverage/sessions summary cards + definitions link), MINUS the rollup tiles (now persistent chrome) and MINUS the inline findings list (now its own tab).

- [ ] **Step 1: Write the failing Findings test.** In `ProjectFindingsTab.test.tsx`, mock `api.getFindings` to return findings of mixed severity + summary, render `<ProjectFindingsTab wsId="x" />`, assert the `FindingMetrics` tiles show, all rows show; click the "High" tile → only high rows remain; click it again (or Total) → all rows return.
- [ ] **Step 2: Run it, confirm failure.**
- [ ] **Step 3: Implement `ProjectFindingsTab`.** `{ wsId }` prop; cancel-guarded `getFindings({ wsId })`; state `{ findings, summary } | null` + typed error. Render `<FindingMetrics summary active={activeSev} onSelect={setActiveSev} />` + the severity-ordered `<FindingRow>` list (filter by `normFindingSeverity(f.severity) === activeSev` when set; pass `project`/`targetId`). Loading/error/empty states. (Reuse the structure from `pages/Findings.tsx` which already does interactive filtering.)
- [ ] **Step 4: Implement `ProjectOverviewTab`.** `{ detail }` prop (the already-loaded `getProject` bundle — passed down from the shell so it doesn't refetch). Render: Recent runs (top ~5 from `detail.recent_runs`, rows linking to `/runs/:id`, "see all →" to the Runs tab path `/projects/:wsId/runs`) + a Coverage summary card ("open →" `/projects/:wsId/coverage`) + a Sessions summary card ("see all →" `/projects/:wsId/sessions`) + a Definitions link card (→ `/projects/:wsId/definitions`). Reuse the markup currently in `ProjectDetail.tsx` for these (move, don't reinvent). No `any`.
- [ ] **Step 5: Run `npm test -- --run` (full) + `npm run build` (strict).** Green + exit 0.
- [ ] **Step 6: Commit.** `git add crates/rupu-cp/web/src/components/project/ProjectFindingsTab.tsx crates/rupu-cp/web/src/components/project/ProjectOverviewTab.tsx <test>` → `feat(cp/web): project findings + overview tab bodies`.

---

### Task 3: Tabbed shell + routing + retire placeholders

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/ProjectDetail.tsx` (becomes the shell)
- Modify: `crates/rupu-cp/web/src/App.tsx` (routes)
- Delete: `crates/rupu-cp/web/src/pages/ProjectRuns.tsx`, `.../ProjectSessions.tsx`, `.../ProjectCoverage.tsx`
- Test: `crates/rupu-cp/web/src/pages/ProjectDetail.test.tsx`

**Context:** `ProjectDetail` blocks-renders on `getProject(wsId)` (header + tiles need it). It now takes a `tab` prop (`'overview' | 'runs' | 'findings' | 'sessions' | 'coverage'`, default `'overview'`) set per-route. `ProjectDefinitions.tsx` and its route stay.

- [ ] **Step 1: Write the failing routing test.** In `ProjectDetail.test.tsx`, render the app/router at `/projects/x/findings` (mock `getProject` + `getFindings`) → the Findings tab is active and its content renders; render at `/projects/x` → Overview active. (Use the project's existing router-test idiom — check another `*.test.tsx` that renders with `MemoryRouter`.)
- [ ] **Step 2: Run it, confirm failure.**
- [ ] **Step 3: Rewrite `ProjectDetail.tsx` as the shell.** Keep the `getProject`/`getProjectAssessedPct` loads + not-found/error/loading guards + the identity `<header>` + the 5 `RollupTile`s (persistent). Below them render `<TabBar>` with five `<TabButton>`s (Overview `LayoutDashboard`, Runs `Play`/`ListOrdered`, Findings `ShieldAlert`, Sessions `MessageSquare`, Coverage `ShieldCheck` — pick sensible lucide icons), each navigating (`useNavigate` or `<Link>`) to `/projects/:wsId`, `/runs`, `/findings`, `/sessions`, `/coverage`; `active` = the `tab` prop. Then render the active tab body: `overview → <ProjectOverviewTab detail={detail} />`, `runs → <ProjectRunsTab wsId />`, `findings → <ProjectFindingsTab wsId />`, `sessions → <ProjectSessionsTab wsId />`, `coverage → <ProjectCoverageTab wsId />`. Wire the shared `TriggerChip` import. Remove the now-moved findings + recent-runs + coverage/sessions/definitions sections from the shell (they live in the tab bodies).
- [ ] **Step 4: Update `App.tsx` routes.** All of `/projects/:wsId` (tab overview), `/projects/:wsId/runs` (runs), `/projects/:wsId/findings` (findings), `/projects/:wsId/sessions` (sessions), `/projects/:wsId/coverage` (coverage) render `<ProjectDetail tab="..." />` (lazy + Suspense as today). Keep `/projects/:wsId/definitions → ProjectDefinitions`. Remove the lazy imports + routes for `ProjectRuns`/`ProjectSessions`/`ProjectCoverage`.
- [ ] **Step 5: Delete the three retired placeholder pages.** `git rm` `pages/ProjectRuns.tsx`, `pages/ProjectSessions.tsx`, `pages/ProjectCoverage.tsx`. Verify nothing else imports them (grep) — `ProjectDefinitions.tsx` stays.
- [ ] **Step 6: Run `npm test -- --run` (full) + `npm run build` (strict).** Green + exit 0; recharts grep = 0; report main chunk size (should stay ~49–50 KB).
- [ ] **Step 7: Commit.** `git add crates/rupu-cp/web/src/pages/ProjectDetail.tsx crates/rupu-cp/web/src/App.tsx crates/rupu-cp/web/src/pages/ProjectDetail.test.tsx` + the `git rm`'d files → `feat(cp/web): tabbed project detail shell + routing; retire placeholder sub-pages`.

---

### Final verification (after all tasks)
- `npm test -- --run` full green; `npm run build` strict exit 0; recharts out of main chunk; no `any`; static Tailwind.
- Final whole-branch review (tab routing, deep-links, filters, no dangling imports to the deleted pages), then hand to matt for visual validation.
