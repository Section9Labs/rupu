# rupu.app — V2 Visual Contract

**Status:** Authoritative visual contract for the macOS app (supersedes HANDOFF.md,
2026-08-24). Values are ported from the CP web source of truth — when the web's v2
system evolves, update BOTH clients and this file in the same arc.

Sources of truth, priority order:
1. Geometry: `docs/superpowers/plans/2026-08-04-rupu-cp-shell-v2-arc.md`
2. Shell chrome: `crates/rupu-cp/web/src/components/v2/Shell.tsx`
3. Intent: `docs/redesign/` (README, mocks, screenshots)
4. Component patterns: `crates/rupu-cp/web/src/lib/status.ts`,
   `components/lists/SortableTable.tsx`, `components/ui/{Button,Badge,Chip}.tsx`,
   `components/StatusPill.tsx`
Exact values below come from `crates/rupu-cp/web/src/styles.css` and
`tailwind.config.ts`.

## Geometry
- 4px spacing grid. Panel radius **7px** (inner cards 6). **No shadows** — 1px
  borders only.
- Rail 204pt; rail nav rows 30pt; top bar 48pt; controls 24–28pt tall; table
  rows 8pt vertical padding.
- Rail active row: `surface` fill + inset 2px left `brand-500` accent (not a pill).

## Color tokens (light → dark, RGB)
Dual-theme (light default, dark via appearance), matching the web's
`:root` / `[data-theme="dark"]` split.

| Token | Light | Dark |
|---|---|---|
| bg | 250 250 250 | 10 10 10 |
| panel | 255 255 255 | 20 20 22 |
| surface | 241 245 249 | 27 27 31 |
| surface-hover | 226 232 240 | 35 35 39 |
| surface-active (fill only, never text) | 203 213 225 | 46 46 51 |
| border | 229 231 235 | 38 38 42 |
| border-strong (line/fill only, never text) | 203 213 225 | 63 63 70 |
| ink | 15 23 42 | 245 245 245 |
| ink-dim | 100 116 139 | 161 161 170 |
| ink-mute (dimmest legal text) | 148 163 184 | 113 113 122 |
| brand-500 | 124 58 237 | 124 58 237 |
| brand-700 (note: dark is LIGHTER than 500) | per styles.css | per styles.css |

Severity (critical/high/medium/low/info, light→dark):
147 51 234→168 85 247 · 220 38 38→248 113 113 · 234 88 12→251 146 60 ·
202 138 4→250 204 21 · 100 116 139→148 163 184 — each with a soft `-bg` tint pair
per styles.css.

Status palette (9 states, light→dark hex): running #3b82f6→#60a5fa ·
done #22c55e→#4ade80 · failed #ef4444→#f87171 · awaiting #f59e0b→#fbbf24 ·
paused #06b6d4→#22d3ee · pending #94a3b8→#71717a · skipped #cbd5e1→#52525b ·
cancelled #64748b→#a1a1aa · rejected = failed. The Swift `ActivityStatus` maps to
this palette 1:1 (no tone collapsing).

Semantic pairs: err/ok/warn/info each with `-bg` tint, per styles.css §56-59/99-102.

## Typography
- **Sans (system) for ALL UI text** — labels, buttons, nav, prose.
- **Mono ONLY for data** — run/step/agent ids, timestamps, numerals
  (`monospacedDigit`), code/diff/transcript blocks, badges/pills content, KPI
  numbers.
- Scale (pt): `meta` 10 · `note` 11 · `ui` 12 (default body) · `lead` 13.
- Eyebrow idiom (true captions only): mono 9–10pt, uppercase, tracking .08–.15em,
  ink-mute.

## Iconography — lucide, icon-for-icon with the web
Bundled as template assets from the pinned `lucide-react` package (ISC — ship the
license note beside the assets). Sizes: 15–16 nav, 9–11 inside pills.

Nav: Overview `LayoutDashboard` · Activity `Activity` · Projects `FolderGit2` ·
Security `ShieldCheck` (Findings `ShieldAlert`, Network `Network`) · Library
`BookMarked` · Fleet `Server` · Usage `DollarSign` · Settings `Settings` ·
Sessions `MessageSquare` · Agents `Sparkles` · Workflows `Workflow` · Autoflows
`Repeat` · Live `Radio`.
Status: running `Play` · completed `CheckCircle2` · failed `XCircle` · awaiting
`Pause` · paused `PauseCircle` · rejected `XOctagon` · cancelled `Ban` · skipped
`SkipForward` · pending `Clock`.
Actions: back `ArrowLeft` · archive `Archive` · delete/danger `Trash2` · graph
`GitBranch` · steps `ListOrdered` · transcript `FileText`.

## Chrome
- StatusPill/dot per `status.ts` descriptors (label + color + icon per state).
- Buttons: primary (brand fill) / outline / danger-outline / ghost, per
  `ui/Button.tsx` variants; text buttons "Approve" (ok tone) / "Reject"
  (danger-outline) for gate actions.
- Flat bordered panel: `panel` fill, 1px `border`, radius 7, no shadow.
- Tint banner (gates/errors): `border-<tone>/30` + `<tone>-bg` fill, radius 7,
  16/12 padding.
- Dialog/sheet: centered card over 40% black scrim.

## Composition rules
- Shell: rail (204pt, flat 7-item IA + pinned Settings + host footer
  "N hosts / M down") + 48pt top bar (scope select · range 7d/30d/all · ⌘K search
  field · live pill · theme toggle).
- Tables (SortableTable contract): sortable headers with chevrons; exactly ONE
  truncating subject column (tooltip on truncation); metadata columns fit-width,
  nowrap; numeric/time columns right-aligned tabular numerals.
- Detail pages: single vertical stack — header (back + title + status pills +
  meta line) → banners → primary visualization → tabbed panel following
  selection (≈65% viewport, min 420pt). No side rails.
- Null discipline: unknown renders `—`, never 0; partial sums `+`.
