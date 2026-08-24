# rupu.app macOS — Design Alignment to the CP v2 Design System

**Date:** 2026-08-24
**Status:** Approved (matt, 2026-08-24) — design session; subagent execution pre-authorized
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella)
**Supersedes as visual contract:** `docs/macOS_design/HANDOFF.md`
**New visual contract:** `docs/macOS_design/V2-CONTRACT.md` (written with this spec)

matt validated Phase 3 functionality but rejected the app's design on all four axes
(icons/visual language, navigation & flow, layout & density, typography/color feel)
relative to the CP web UI. Decision: realign the native app to the **v2 design
system** — the aspirational IA the web itself targets (arc:
`docs/superpowers/plans/2026-08-04-rupu-cp-shell-v2-arc.md`) — making the app the
first full realization of that language. Fidelity rule (settled): **web look,
native feel** — port tokens, iconography, density, and flow structure faithfully;
keep native macOS mechanics (real sheets, keyboard nav, native scrolling/menus)
where strictly better.

## 1. The contract

`docs/macOS_design/V2-CONTRACT.md` becomes the visual authority for the app. Its
sources, in priority order:

1. v2 arc geometry rules: 4px grid; 7px panel radius; **no card shadows**; nav
   rows 30px; top bar 48px; controls 24–28px; 8px vertical table padding.
2. The landed v2 shell (`crates/rupu-cp/web/src/components/v2/Shell.tsx`): 204px
   rail, flat 7-item IA + pinned Settings, active row = surface fill + inset 2px
   left brand accent, host footer, top-bar composition.
3. `docs/redesign/` mocks + screenshots for intent.
4. v1 component patterns where v2 doesn't redefine them: `lib/status.ts` (canonical
   9-state status palette + glyph map), `SortableTable` contract, Button/Badge/
   Chip/StatusPill chrome, the `meta/note/ui/lead` type scale.

The contract pins exact values ported from `crates/rupu-cp/web/src/styles.css` and
`tailwind.config.ts` so app and web cannot silently drift. When the web builds its
own v2 Plans 2–6, this document serves both clients. HANDOFF.md receives a
superseded banner (not deleted — it documents the Phase 1–3 baseline).

## 2. Plan 1 — Design language (RupuDesign v2)

- **Tokens** (both themes, from styles.css): neutrals bg/panel/surface/hover/
  active/border/border-strong/ink/dim/mute; brand 50/100/500/600/700 (dark 700
  lighter than 500 — inverted for legibility); severity critical/high/medium/low/
  info + soft `-bg` tints; the **9-state status palette** (running, done, failed,
  awaiting, paused, pending, skipped, cancelled, rejected=failed) replacing the
  5-tone `RunTone` collapse — `ActivityStatus` maps 1:1; semantic err/ok/warn/info
  + `-bg` pairs. Per-appearance token tests regenerate against the new values.
- **Typography**: sans (system) for ALL UI text — labels, buttons, prose; mono
  ONLY for data — ids, timestamps, numerals, code/transcript. Scale: meta 10 /
  note 11 / ui 12 / lead 13 (pt). Eyebrow idiom (mono 9–10pt uppercase, tracking
  .08–.15em, mute) reserved for true captions. This reverses the app's
  mono-heavy micro-label habit — the single largest "feel" fix.
- **Iconography**: lucide, icon-for-icon with the web. Bundled as template assets
  sourced from the web's pinned `lucide-react` package (ISC license; a LICENSE
  note ships with the assets; no third-party code). Set: LayoutDashboard,
  Activity, Sparkles, Workflow, Repeat, MessageSquare, FolderGit2, ShieldCheck,
  ShieldAlert, Network, BookMarked, Server, DollarSign, Settings, Radio; status
  Play, CheckCircle2, XCircle, Pause, PauseCircle, XOctagon, Ban, SkipForward,
  Clock; actions ArrowLeft, Archive, Trash2, GitBranch, ListOrdered, FileText.
  A small `Icon` view renders them template-tinted at web sizes (15–16 nav,
  9–11 in pills).
- **Chrome kit**: StatusPill/dot per `status.ts` descriptors; Button variants
  (primary/outline/danger-outline/ghost per `ui/Button.tsx`); Badge/Chip; flat
  bordered panel (7px radius, 1px border, no shadow); tint banner
  (`border-tone/30 + tone-bg fill`) for gates/errors.
- **Mechanical re-skin pass** over all existing screens: token pickup is automatic
  where screens consume RupuDesign; a sweep replaces hardcoded styles, mono-abuse,
  8px radii, and SF Symbols.
- **Checkpoint**: matt runs app beside web and judges the language before Plan 2.

## 3. Plan 2 — Flows & composition

- **Shell**: 204pt rail, flat IA — Overview · Activity · Projects · Security ·
  Library · Fleet · Usage; Settings pinned bottom; host footer ("N hosts / M
  down", using the existing hosts machinery). The current "Runs + leaves"
  sidebar collapses into the single Activity item (kind tabs live in the page —
  the route state machinery already supports this; sidebar-one-state tests
  adapt). Active row: surface fill + inset 2px left brand accent.
- **Top bar (48pt)**: scope select (project or "all projects" — wired to the
  workspace filter where list APIs accept ws_id, honest "all" otherwise), range
  segmented (7d/30d/all — existing state), **⌘K search field** opening a native
  command palette (minimal honest scope: navigation, run/definition search,
  pending-gate approve — pulled forward from Phase 6 because it is load-bearing
  in the v2 bar; grows later), live pill, theme toggle. Replaces the current
  toolbar arrangement.
- **Run detail**: recomposed to the web's shape — single vertical stack: header
  (ArrowLeft back, title, status pills, meta line) → gate banner(s) → step graph
  → **tabbed panel (Transcript · Events · Findings · Netflow) that follows graph
  selection** (fixed-height panel ≈65% viewport, min 420pt). Side rails retired;
  run facts fold into the header meta line + an Events tab. Stores unchanged.
- **Tables**: SortableTable contract — sortable headers with chevrons, ONE
  truncating subject column (tooltip on truncation), fit-width metadata columns,
  right-aligned tabular numerals, 8px vertical padding.
- **Launcher**: remains a native sheet (validated flow, native strength), restyled
  to the web's dialog chrome (centered card over 40% scrim); toolbar "+ New run"
  + ⌘N stay. Library per-row Launch arrives with the Library screen (Phase 5),
  not this phase.
- **Session/agent-run detail**: same recomposition idiom (header + stacked
  content); send box per web input chrome.

## 4. Verification & sequencing

- Store/contract layer untouched: all 242 Swift + 28 Rust tests pass throughout
  (except token-value and sidebar-mapping test updates belonging to the plans).
- Visual verification: build + matt side-by-side passes at the two plan
  checkpoints; screenshots where the screen allows.
- Phase 4 (dashboard) starts only after Plan 2; Overview follows the v2 arc's
  intent (instrument strip + needs-you queue), giving the web a reference for its
  own unbuilt Plan 3.

## 5. Out of scope

Web-side changes; new API surface; Situation Room (Phase 6); light-theme redesign
beyond porting both existing theme tables; menu bar extra; saved views; Library
screen.
