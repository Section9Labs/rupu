# rupu.app Phase 5A (Projects · Fleet · Library + scope-aware launch) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Projects, Fleet, and Library placeholders with real v2 screens over the existing + lightly-extended client surface, and close the umbrella's tracked scope-aware-launch gap server-side and client-side.

**Architecture:** One shared sortable-list groundwork task, one Rust task (launch scope resolution + fixtures), thin new API surface (workers, autoflow defs, project detail/sub-lists), then three screen tasks composing the proven idioms (BlockState blocks, store-per-screen lifecycle with client-identity rebuild, v2 chrome). New feature modules `RupuProjects`, `RupuFleet`, `RupuLibrary`.

**Tech Stack:** SwiftUI, Swift Testing, rupu-cp fixture rig, axum handlers (scope fix).

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-phase-5-breadth-design.md` §1–§3 (+§5 dispositions). Web references per screen: `crates/rupu-cp/web/src/pages/{Projects,ProjectDetail,ProjectDefinitions,Hosts,Workers,Agents,AgentDetail,Workflows,WorkflowDetail,AutoflowsDefs}.tsx`; server: `crates/rupu-cp/src/api/{projects,workers,agents,workflows,autoflows,hosts}.rs`.

## Global Constraints

- Branch `feat/macos-phase5a-breadth` off origin/main (post PR #600). All suites green; zero warnings; Plan-1 exit greps zero; @MainActor rule for View-member tests; condition-polling only; new URLProtocol test stubs use the generation-token isolation pattern (`DashboardStubURLProtocol` precedent).
- V2 chrome; SortableTable contract (ONE truncating subject column, chevron sort headers, fit-width metadata, right-aligned tabular numerals, 8pt rows); null discipline (`—`, never fabricated 0); no silent-noop controls; per-block BlockState rendering.
- Screen stores follow the established ownership pattern: lazy build on client, `.task` activate, `.onDisappear` deactivate, client-identity rebuild (`storeClientID`), re-activate on healthy transition IF the screen can be a cold-launch landing (these can't — Overview is; doc-comment it).
- Rust: per-file rustfmt only; `cargo test -p rupu-cp` green on Rust-touching tasks; launch-scope change is BACK-COMPATIBLE (absent fields = today's behavior).
- Fixtures for every new endpoint consumed; `make macos-fixtures` regeneration; new modules registered in Package.swift (+ `make macos-gen` when project.yml changes — check whether it needs a change; Phase 4 showed it references only the RupuKit product).
- Never bare `git stash pop`. Commit messages as given per task.

---

### Task 1: Sortable-list groundwork

**Files:**
- Create: `RupuKit/Sources/RupuDesign/ListSort.swift`, `RupuKit/Sources/RupuDesign/SortableHeader.swift`
- Test: `RupuKit/Tests/RupuDesignTests/ListSortTests.swift`

**Interfaces — Produces:**
- `public struct ListSort<Key: Hashable & CaseIterable>: Equatable { public var key: Key; public var ascending: Bool }` plus a pure `public func sortRows<Row, Key>(_ rows: [Row], sort: ListSort<Key>, value: (Row, Key) -> ListSortValue) -> [Row]` where `public enum ListSortValue: Comparable-ish { case text(String?), number(Double?), date(Date?) }` — semantics generalized from `ActivitySort`: case-insensitive text, nulls ALWAYS last regardless of direction, stable via index tiebreak (do NOT refactor ActivitySort itself — it stays as-is, its tests untouched; doc-comment the relationship).
- `public struct SortableHeaderRow<Key>: View` — the header-button idiom from ActivityTable (Eyebrow label + chevron `Icon(.chevronUp/.chevronDown, size: 9)`, reserved-space opacity trick, first-tap direction: text ascending, number/date descending — the ruled macOS-native convention) parameterized over columns `[(key: Key, label: String, width: CGFloat?, alignment: Alignment)]` with the ONE flexible column marked.

- [ ] **Step 1:** failing `sortRows` table (text case-insensitivity, nulls-last both directions, stable tiebreak, number/date keys, empty). RED → implement → GREEN.
- [ ] **Step 2:** `SortableHeaderRow` (no render test; @MainActor rule if any test touches it). Gates. **Commit** — `feat(macos-breadth): generic ListSort + sortable header idiom`

### Task 2: Scope-aware launch — server fix + fixtures

**Files:**
- Modify: `crates/rupu-cp/src/api/agents.rs` (run + session handlers, ~765-844), `crates/rupu-cp/src/api/workflows.rs` (run handler, ~593-660), `crates/rupu-cp/tests/macos_fixtures.rs` (request-body fixtures)
- Test: handler unit tests alongside the existing ones in those files

**Interfaces — Produces (wire contract):**
- `AgentRunBody`/`SessionStartBody`/`LaunchBody` gain `scope_kind: Option<String>`, `scope_id: Option<String>` (serde default). When BOTH present-and-valid: resolve the definition via the same scoped resolution the module's GET/DELETE handlers use (`resolve_agent_scoped`/`resolve_workflow_scoped`); resolution miss → 404 with a body naming the scope; scope_kind present without a resolvable combination → 400. Absent → existing path-based behavior byte-for-byte (back-compat; assert with a regression test).
- The launch request proceeds with the RESOLVED definition's path/working-dir semantics — study how the scoped resolvers hand back a definition location and thread it into `LaunchRequest` the same way the existing code derives it; if the launcher can only resolve by path today, pass the resolved definition's project path as `working_dir` when the body didn't specify one (document the mechanism chosen in the handler comment — the invariant is: the definition the picker showed is the definition that runs).

- [ ] **Step 1:** failing Rust tests — scoped launch resolves the project-scoped definition when a global name-collision exists; absent scope = unchanged behavior; bad scope → 4xx. RED → implement → GREEN (`cargo test -p rupu-cp`).
- [ ] **Step 2:** update request-body fixtures (`requests/` rig) for the three bodies incl. the new optional fields; `make macos-fixtures`; commit. **Commit** — `feat(cp): scope-aware launch — optional scope_kind/scope_id on the three launch bodies`

### Task 3: New Swift API surface (workers · autoflow defs · project detail)

**Files:**
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs` (+`workers.json`, `autoflow_defs.json`, `project_detail.json`, `project_runs.json`, `project_sessions.json`)
- Create/Modify (Swift): `RupuAPI/DefinitionModels.swift` (+`AutoflowDefinition`), `RupuAPI/Models.swift` or new files (+`APIWorkerRow`, `APIProjectDetail` with its counts blocks), `RupuAPI/CPClient.swift` (+`workers()`, `autoflowDefinitions()`, `projectDetail(wsID:)`, `projectRuns(wsID:offset:limit:)`, `projectSessions(wsID:offset:limit:)`, `projectAgents/Workflows/Autoflows(wsID:)`, `setAutoflowEnabled(name:scopeKind:scopeID:enabled:)`)
- Test: `Tests/RupuAPITests/FixtureDecodingTests.swift` additions

**Interfaces:** field-for-field vs the serde types (`WorkerView`, `AutoflowDefRow`, `ProjectDetail{project,runs{...},sessions{...},coverage{targets,findings},recent_runs,usage}`); project runs/sessions rows decode with the EXISTING `APIRunListRow`/`APISessionRow` (verify shape identity — the survey says the sub-lists serve the same row types; if they differ, own minimal types and say so). Scoped definition lists reuse `AgentDefinition`/`WorkflowDefinition`.

- [ ] **Step 1 (Rust):** fixture emitters, regen, `cargo test -p rupu-cp` green. **Commit** — `test(cp): golden fixtures — workers, autoflow defs, project detail + sub-lists`
- [ ] **Step 2 (Swift):** failing decode tests → models + client methods → GREEN; gates. **Commit** — `feat(macos-breadth): API surface — workers, autoflow definitions, project detail`

### Task 4: Launcher scope threading (macOS side of the fix)

**Files:**
- Modify: `RupuStore/LauncherStore.swift`, `RupuAPI/WriteModels.swift` (bodies gain scopeKind/scopeID), `RupuLauncher/DefinitionPicker.swift` (rows already display scope — thread selection), request-fixture decode tests
- Test: `Tests/RupuStoreTests/LauncherStoreTests.swift` additions

**Interfaces:** `LauncherStore.selectedDefinition` carries the picked row's `scopeKind`/`scopeID`; `launch()` includes them in every POST body (nil-safe); the request-body round-trip tests (`request_fixture_roundtrips` rig) updated. Prefill seam for Task 7: `public func prefill(definition:kind:)` (or equivalent) so Library's per-row Launch opens the sheet with the definition + scope preselected — design the smallest honest API and document it.

- [ ] **Step 1:** failing store tests (launch body carries scope of the picked row; global-scope rows send nil; prefill selects). RED → implement → GREEN; gates. **Commit** — `feat(macos-breadth): launcher threads definition scope into launch bodies + prefill seam`

### Task 5: Projects screens

**Files:**
- Create: `RupuKit/Sources/RupuProjects/` (`ProjectsScreen.swift`, `ProjectDetailScreen.swift`, `ProjectsStore.swift` in RupuStore or module-local — follow the DashboardStore precedent: stores live in RupuStore), Package.swift entry + test target
- Modify: `RupuStore/Route.swift` (+`.projectDetail(wsID:)` route; `.projects` keeps sidebar highlight — extend the selectedSidebarItem mapping + RoutingTests), `RupuShell/RootView.swift`
- Test: store tests + routing adaptations

**Produces:** `ProjectsStore` (list via `projects()`, BlockState) + `ProjectDetailStore` (detail + lazy per-tab fetches, each its own BlockState; runs/sessions windowed — first 50 + "show all"); list screen = sortable table (ListSort keys name/runs/lastRun/spend) rows → `.projectDetail`; detail = header + tab bar (Overview/Runs/Sessions/Findings/Coverage/Definitions) — Findings tab reuses `runFindings`-style call with `ws_id` (new client param or method — smallest honest addition), Coverage tab renders the spec's "arrives with Security" placeholder note (NO fake data), Definitions tabs reuse the definition-list row rendering from Task 7's components where compositionally clean (if Task 7 hasn't landed yet, keep local minimal rows — say which).

- [ ] **Step 1:** failing store tests (list, detail lazy-tab loads, windowing) → implement → GREEN.
- [ ] **Step 2:** screens + routes + RoutingTests adaptation; gates. **Commit** — `feat(macos-breadth): Projects — list + tabbed detail`

### Task 6: Fleet screen

**Files:**
- Create: `RupuKit/Sources/RupuFleet/FleetScreen.swift`, `RupuStore/FleetStore.swift`; Package.swift entries
- Modify: `RupuShell/RootView.swift`
- Test: store tests

**Produces:** `FleetStore` (hosts via existing `hosts()` + workers via new `workers()`; 60s reconcile reusing the HostsFooterStore loop idiom; remove-host mutation with PendingActions + confirm-first). Screen: host cards grid (auto-fill min 268pt; card = name, transport, version, active runs, status tone; faulted card = fail border + inset edge + reason panel per umbrella; Remove behind an overflow `Icon(.moreHorizontal)` menu with a confirmation dialog) + workers sortable table (name/status/active/total/last-run). No add-host forms (spec disposition — a quiet footer note points at `rupu hosts add`).

- [ ] **Step 1:** failing store tests (merge of hosts+workers blocks, remove pending-state) → implement → GREEN.
- [ ] **Step 2:** screen + wiring; gates. **Commit** — `feat(macos-breadth): Fleet — host cards + workers table + remove`

### Task 7: Library screens

**Files:**
- Create: `RupuKit/Sources/RupuLibrary/` (`LibraryScreen.swift`, `AgentDetailScreen.swift`, `WorkflowDetailScreen.swift`, `LibraryStore.swift` in RupuStore); Package.swift entries
- Modify: `RupuStore/Route.swift` (+`.agentDefinition(name:)`, `.workflowDefinition(name:)` routes + mapping + tests), `RupuShell/RootView.swift`, `RupuAPI/CPClient.swift` (+`agentDetail(name:)` if the raw-md endpoint isn't wired — check `GET /api/agents/:name` and add model/method/fixture if missing → that fixture goes through the Task 3 Rust pattern; fold the small Rust commit here)
- Test: store tests + routing

**Produces:** `LibraryStore` (three definition lists, BlockState each; enable/disable autoflow mutation with PendingActions). Screen: segmented tabs agents/workflows/autoflows; sortable tables; permission-tone badges (umbrella rule: read-only=done, ask=await, bypass=fail LOUD — from the DTO fields, never parsed from markdown); per-row Launch → LauncherSheet via Task 4's prefill; autoflow rows get the enabled chip + toggle. Agent detail: meta chips + description + raw `.md` in a mono scroll block + page Launch. Workflow detail: inputs + YAML mono block + autoflow toggle + page Launch. All read-only otherwise (spec dispositions).

- [ ] **Step 1:** failing store tests (lists, toggle pending-state, badge-tone pure mapping) → implement → GREEN.
- [ ] **Step 2:** screens + routes + prefill wiring; gates. **Commit** — `feat(macos-breadth): Library — definition tabs, detail views, per-row launch`

### Task 8: Docs + checkpoint

**Files:** `CLAUDE.md` (module map + read-first), umbrella §8 Phase 5 row (Plan A delivered portion + dispositions), spec cross-check.

- [ ] **Step 1:** docs edits; full gates incl. `cargo test -p rupu-cp`. **Commit** — `docs(macos-breadth): Phase 5A pointers + dispositions`
- [ ] **Step 2 (controller):** checkpoint screenshots (three screens, dark+light) + notes.

---

## Self-review notes

- Spec §1→T1, §2 Projects→T5 / Fleet→T6 / Library→T7, §3→T2+T4, §5 gates per task. Coverage-tab placeholder honesty in T5 matches the spec amendment.
- Type consistency: `ListSort`/`SortableHeaderRow` (T1) consumed by T5/T6/T7; `AutoflowDefinition`/`APIWorkerRow`/`APIProjectDetail` (T3) consumed by T5/T6/T7; scope fields (T2 wire ↔ T4 Swift) named identically snake/camel.
- Risk accepted: T7 may discover `GET /api/agents/:name` shape needs a model beyond the list DTO — the task owns the addition end-to-end (incl. its fixture) rather than splitting a micro-task.
- T5/T6/T7 are sequential (shared Route/RootView/Package.swift files) — never parallel, per SDD.
