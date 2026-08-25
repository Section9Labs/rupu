# rupu.app macOS UI-Redesign Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the parked visual/interaction gaps against the web CP — the dedicated polish pass matt deferred until the phases finished.

**Architecture:** Audit-first (side-by-side vs the live web CP, committed as a doc), then chrome-kit fidelity (pill geometry/tinting at the kit level), then the three parked interaction affordances (Customize reorder, SR follow/pin+fresh, stable sort identities), then the Settings-tone coherence sweep. The controller performs the audit (Task 1 needs the live web CP + screenshots — GUI work); implementers execute Tasks 2–6 from the audit's pinned findings.

**Tech Stack:** SwiftUI, RupuDesign tokens/chrome, Swift Testing.

**Spec:** `docs/superpowers/specs/2026-08-25-rupu-macos-redesign-pass-design.md`

## Global Constraints

- NEVER touch `ShellToolbar.swift` or the top-bar region — PR #612 owns it (spec §0). Backlog row 27 stays parked there.
- The web CP + `docs/macOS_design/V2-CONTRACT.md` are the visual authority; every fidelity change cites the web source it matches (file:line for code-derived values like radii/tints).
- Every task: full `make macos-test` + `make macos-build` green, actual numbers reported; `@Test @MainActor` on ANY View-type member (statics included); generation-token stubs; semaphore-gated slow stubs (never timed sleeps); condition-poll terminal states.
- Truthful citations (program record: 5 fabrication incidents, last 3 phases clean — verify before writing).
- Honest UI: no dead controls; every audit finding is fixed or backlog-recorded with citation — never silently dropped.
- Implementers never dispatch subagents. Never bare `git stash`/`git stash pop`.

---

### Task 1 (CONTROLLER, not dispatched): Side-by-side audit

**Files:**
- Create: `docs/macOS_design/redesign-pass-audit.md`

**Steps:**

- [ ] **Step 1:** Drive the app and the web CP (port 7420's embedded UI in a browser — same backend, same data) screen by screen, both themes where they differ: Overview, Activity (+run detail), Projects (+detail tabs), Security, Library, Fleet, Usage, Situation Room vs `/events`, Settings.
- [ ] **Step 2:** Write the audit table: surface · web rendering · app rendering · divergence · disposition (fix task # / park with backlog citation). MUST pin backlog nits 25 ("brand header badge") and 26 ("Awaiting wording") to concrete elements — the recorded names carry no detail; if a nit cannot be reproduced, record that finding honestly instead of inventing a fix.
- [ ] **Step 3:** Commit the audit doc. Its "fix" rows become the authoritative inputs for Tasks 2 and 6.

---

### Task 2: Chrome-kit fidelity — pill geometry + tint policy

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuDesign/Chrome/` (StatusPill.swift, Badge/Chip files — read the kit first)
- Test: existing RupuDesign test files (extend)

**Interfaces:**
- Consumes: the audit's pinned chrome findings; `crates/rupu-cp/web/src/lib/status.ts` (the per-status `pillClass` map — the tinted set vs the neutral set) and the web's pill corner radius (find the actual CSS — `rounded` utility class value — and cite it).
- Produces: kit-level shape token (rounded-rect radius matching the web, replacing `Capsule`) used by every pill/badge/chip; a status→tinted-or-flat policy matching status.ts exactly (`pending`/`cancelled`/`skipped` flat-neutral per the parked nit — verify the full set against the source, don't assume only those three).

**Steps:**

- [ ] **Step 1:** Read status.ts fully; write the policy table test FIRST (every status → expected tinted/flat + tone), asserting against the current kit — the neutral cases fail.
- [ ] **Step 2:** Implement the shape + policy change at the kit level; sweep consumers for layout fallout (`grep` for direct Capsule usage outside the kit).
- [ ] **Step 3:** Full gates green. Commit: `feat(macos): chrome fidelity — web pill geometry + flat-neutral status policy`

---

### Task 3: Overview Customize drag/reorder

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuOverview/` (OverviewScreen customize menu + block order rendering)
- Modify: the visibility-persistence home (find where Customize show/hide persists — extend the same mechanism with order)
- Test: RupuOverviewTests

**Interfaces:**
- Produces: persisted block order (same store as visibility — one persistence seam); Overview renders blocks in persisted order; the Customize affordance gains reorder (lightest mechanism: `.onMove` in a List-based popover/sheet section). Reset-to-default affordance if the web/HANDOFF has one — check HANDOFF's Customize spec and mirror; if HANDOFF specifies drag-only, don't invent extras.

**Steps:**

- [ ] **Step 1:** Read HANDOFF.md's Customize spec + the existing persistence. Failing tests: order persists round-trip; unknown/new block ids append at default position (forward-compat — a new block added in a later version must not vanish for users with persisted order).
- [ ] **Step 2:** Implement; `@Test @MainActor` view tests for the reorder affordance's presence and order-driven rendering.
- [ ] **Step 3:** Full gates green. Commit: `feat(macos): Overview Customize — drag/reorder with persisted block order`

---

### Task 4: Situation Room follow/pin + fresh-highlight

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuSituation/` (EventStreamColumn + pure derivations)
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuStore/SituationStore.swift` (fresh-key tracking if store-side)
- Test: RupuSituationTests / RupuStoreTests

**Interfaces:**
- Consumes: `crates/rupu-cp/web/src/components/situationRoom/EventStream.tsx:56-65` (follow/pin: auto-scroll pinned to newest; suspended while the operator has scrolled away; a "jump to latest (N new)" affordance while suspended — read the actual mechanism and port its DECISIONS) and `crates/rupu-cp/web/src/pages/Events.tsx:87,139-144` (freshKeys/FRESH_MS arrival highlight — port the constant and the expiry semantics).
- Produces: pure pieces in RupuSituation with table tests (fresh-set fold with expiry; follow-state decision given scroll position + arrivals); scene-side scroll anchoring via ScrollViewReader (the SwiftUI mechanism differs from the web's — port the behavior contract, not the DOM code); the 6B "content must not shift under a reading operator" gap closes.

**Steps:**

- [ ] **Step 1:** Read both web sources fully. Write the ported test tables (fresh expiry; follow suspend/resume decisions) — failing.
- [ ] **Step 2:** Implement pure pieces, then the scene wiring; the new-arrivals affordance is honest (real count).
- [ ] **Step 3:** Full gates green. Commit: `feat(macos): Situation Room — follow/pin scroll contract + fresh-arrival highlight`

---

### Task 5: Stable sortable-table identities

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuLibrary/LibraryScreen.swift` (offset-keyed ForEach sites)
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuUsage/OutlierPanel.swift`
- Test: corresponding test files

**Interfaces:**
- Consumes: Phase 5's `ListSort`/`SortableHeaderRow` groundwork; the backlog row 20 finding (`id: \.offset` sites — grep for the full set, the two named files may not be exhaustive).
- Produces: every sortable table keyed by a stable identity (definition name, run id — composite where wire ids aren't unique, per the standing ForEach rule); a reorder-under-sort regression test per converted table (rows keep identity when the sort re-orders them — assert via a state-carrying row attribute, e.g. selection/hover survives a re-sort in a store-level model test).

**Steps:**

- [ ] **Step 1:** Grep the app for `id: \.offset` / `\.enumerated()` ForEach patterns; list every sortable-table site in the report (non-sortable enumerated sites are out of scope — note them).
- [ ] **Step 2:** Failing regression tests; convert; full gates green.
- [ ] **Step 3:** Commit: `fix(macos): sortable tables keyed by stable identity, not offset`

---

### Task 6: Settings tone + audit minor-findings batch

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuShell/{SettingsView,ConfigTab,NotificationsTab}.swift` (token-palette styling)
- Modify: per the audit's fix rows (batched; files enumerated by the audit doc)
- Test: extend touched test files

**Interfaces:**
- Consumes: the audit doc's fix rows assigned to this task; RupuDesign tokens.
- Produces: Settings content styled on the app's token palette (panel/ink/background) inside the native Settings frame; each audit fix row implemented exactly as its row specifies, with the row marked done in the audit doc (same commit). Dead `ProjectDetailStore.wsID` removed if still present (one-liner rider).

**Steps:**

- [ ] **Step 1:** Implement the Settings token styling; verify no regression to the four tabs' tests.
- [ ] **Step 2:** Work the audit's fix rows screen by screen; update the audit doc's disposition column as each lands.
- [ ] **Step 3:** Full gates green. Commit: `feat(macos): redesign pass — Settings tone + audit findings batch`

---

## Post-plan (controller)

Final whole-branch review (most capable model), live before/after GUI validation (both themes; screenshot pairs for the checkpoint), checkpoint package to matt (incl. the audit doc and anything parked), PR, merge on verified-green CI. Backlog doc updated: rows 6/18/20/23–27 dispositioned; new findings recorded.
