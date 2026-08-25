# rupu.app macOS — UI-redesign pass

**Date:** 2026-08-25
**Status:** Executed under matt's standing direction ("continue with all of the phases and then we will come back to redesign the UI" — phases complete as of v0.75.0-beta.3); decisions recorded for the checkpoint
**Authority:** `docs/macOS_design/V2-CONTRACT.md` + the live web CP remain the visual contract — this pass is a FIDELITY and COHERENCE sweep, not a new design direction. A from-scratch visual re-think beyond web parity is matt-at-the-keyboard work and is out of scope here.
**Plan:** `docs/superpowers/plans/2026-08-25-rupu-macos-redesign-pass.md`
**Inputs:** post-parity backlog rows 6, 18, 20, 23–27 (`docs/superpowers/specs/2026-08-25-rupu-macos-post-parity-backlog.md`); the `macos-ui-redesign-deferred` memory; PR #598/#599 checkpoint notes.

## 0. Coordination constraint

PR #612 (open, another session) owns the toolbar region (unified Range, icon-only controls, Live item). This pass does NOT touch the top bar: backlog row 27 (scope-select thinness) is **explicitly left to ride with or follow #612**, recorded here so the row isn't lost. If #612 merges mid-pass, rebase; never edit `ShellToolbar.swift` in this arc. **AMENDED mid-pass:** #612 merged (2026-08-25); the branch rebased onto it and the embargo lifted to "extend, never restructure" — T3's Customize reorder adds a menu item + sheet inside the existing `customizeMenu` content without touching #612's ToolbarItem ids/placements/native-Customize semantics (ledger RESEQUENCE ruling).

## 1. Side-by-side audit (opens the pass)

Two of the five parked nits were recorded name-only ("brand header badge", "Awaiting wording" — the web's run-status label is literally the same string, so the recorded complaint lives on some other surface). The pass opens with a screen-by-screen side-by-side against the live web CP (same data, both themes) producing an audit table: surface · web rendering · app rendering · divergence · fix/park. The audit PINS nits 25–26 to concrete elements and is allowed to surface NEW divergences; everything found is either fixed in this pass or added to the backlog doc with a citation — nothing silently dropped. The audit is committed as a doc (`docs/macOS_design/redesign-pass-audit.md`) so matt's checkpoint reviews evidence, not memory.

## 2. Chrome-kit fidelity (backlog 23–24 + audit findings)

- **Pill shape**: `Capsule` → the web's rounded-4 (`rounded` 4pt corner radius) across the chrome kit (StatusPill/Badge/Chip and any capsule-shaped derivative). One kit-level change; all consumers sweep for layout fallout.
- **Tinted-vs-flat status pills**: `pending`/`cancelled`/`skipped` render flat-neutral exactly as `web/src/lib/status.ts` renders them (read its per-status `pillClass` — the neutral statuses carry no tint); the tinted set stays byte-matched to the web's tinted set.
- Whatever pill/badge divergences the audit adds (within the chrome kit's remit).

## 3. Interaction affordances parked to this pass

- **Overview Customize drag/reorder** (backlog 6, HANDOFF-specified): the Customize menu's blocks become reorderable (SwiftUI `.onMove`-style list edit affordance in the menu or an edit sheet — pick the lightest mechanism that persists order with the existing visibility persistence); Overview renders blocks in the persisted order.
- **Situation Room follow/pin + fresh-highlight** (backlog 18): port the web's behaviors — `EventStream.tsx:56-65` (auto-follow newest with pin-to-top suspension while the operator scrolls back; content must not shift under a reading operator) and `Events.tsx:87,139-144` (`freshKeys`/FRESH_MS arrival highlight). Pure derivation pieces land in `RupuSituation` with tests; scroll behavior is scene-side.
- **Stable sortable-table identities** (backlog 20): Library's tables and Usage's OutlierPanel re-key from array offset to stable identity fields (definition name / run id), building on Phase 5's generic `ListSort` groundwork. A reorder-under-sort regression test each.

## 4. Coherence sweep

- **Settings scene tone**: the 6A validation noted the Settings window reads native-chrome-gray against the app's v2 dark. Bring the Settings tabs' content styling onto the app's token palette (background/ink/panel tokens) while keeping the native `Settings` scene frame — parity of tone, not a custom window.
- Small recorded leftovers riding along ONLY where they are one-line and in touched files: dead `ProjectDetailStore.wsID` (5A parked), the Fleet workers "capabilities" column absence note (record-or-add per audit call).
- The audit's minor findings (spacing/density/copy) — batched by screen.

## 5. Verification

Full suite + build gates per task as always (every-task full `make macos-test`; View-member tests `@Test @MainActor`; stub generation tokens; semaphore-gated stubs — never timed sleeps). Close with a live side-by-side GUI validation pass (both themes) against the web CP on the same data, screenshotting each fixed surface; checkpoint package to matt with before/after pairs where the change is visible. Merge on verified-green CI per the standing directive.

## 6. Out of scope

Top bar / toolbar (PR #612's region, incl. backlog 27); parity/feature backlog rows 1–5, 7–17, 19, 21–22; any new visual direction beyond web fidelity; web-side changes.
