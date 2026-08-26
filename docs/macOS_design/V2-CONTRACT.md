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
| brand-50 | 245 243 255 | 33 23 56 |
| brand-100 | 237 233 254 | 45 33 74 |
| brand-500 | 124 58 237 | 124 58 237 |
| brand-600 | 109 40 217 | 124 58 237 |
| brand-700 (note: dark is LIGHTER than 500) | 91 33 182 | 167 139 250 |

Severity (light→dark RGB, each with a soft `-bg` tint pair):
| Severity | fg light | fg dark | bg light | bg dark |
|---|---|---|---|---|
| critical | 147 51 234 | 168 85 247 | 250 245 255 | 42 28 56 |
| high | 220 38 38 | 248 113 113 | 254 242 242 | 48 24 24 |
| medium | 234 88 12 | 251 146 60 | 255 247 237 | 48 32 18 |
| low | 202 138 4 | 250 204 21 | 254 252 232 | 46 40 16 |
| info | 100 116 139 | 148 163 184 | 248 250 252 | 30 31 35 |

Status palette (9 states, light→dark hex): running #3b82f6→#60a5fa ·
done #22c55e→#4ade80 · failed #ef4444→#f87171 · awaiting #f59e0b→#fbbf24 ·
paused #06b6d4→#22d3ee · pending #94a3b8→#71717a · skipped #cbd5e1→#52525b ·
cancelled #64748b→#a1a1aa · rejected = failed. The Swift `ActivityStatus` maps to
this palette 1:1 (no tone collapsing).

Semantic pairs (light→dark RGB):
| Semantic | fg light | fg dark | bg light | bg dark |
|---|---|---|---|---|
| err | 220 38 38 | 248 113 113 | 254 242 242 | 43 22 22 |
| ok | 22 163 74 | 74 222 128 | 240 253 244 | 18 40 27 |
| warn | 217 119 6 | 251 191 36 | 255 251 235 | 48 36 12 |
| info | 37 99 235 | 96 165 250 | 239 246 255 | 20 32 54 |

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
- Shell: rail (204pt, 7-item IA **with disclosure sub-items** — Activity ▸
  Agents/Workflows/Autoflows/Sessions · Security ▸ Findings/Coverage/Network ·
  Library ▸ Agents/Workflows/Autoflows · Fleet ▸ Hosts/Workers — + pinned
  Settings + host footer "N hosts / M down"; amended 2026-08-25). Parent rows
  navigate AND disclose; child rows deep-link to the page's kind tab; child
  rows indent one level at the same 30pt height; a parent containing the
  active route tints brand. Top bar 48pt (scope select · range 7d/30d/all ·
  ⌘K search field · live pill · theme toggle).
- Tables (SortableTable contract): sortable headers with chevrons; exactly ONE
  truncating subject column (tooltip on truncation); metadata columns fit-width,
  nowrap; numeric/time columns right-aligned tabular numerals.
- Detail pages: single vertical stack — header (back + title + status pills +
  meta line) → banners → primary visualization → tabbed panel following
  selection (≈65% viewport, min 420pt). No side rails.
- Null discipline: unknown renders `—`, never 0; partial sums `+`.

## Run graph (added 2026-08-25, sources: kindVisuals.ts / stepStyle.ts / rg-* keyframes)
- Two-channel rule: 3px top accent bar + kind pill = KIND color; glyph badge +
  state label = STATE color. Never mix the channels.
- Kind accents: step→status-running · for_each→brand-500 · parallel→sev-critical
  · panel→status-awaiting · gate→status-paused · action→sev-info ·
  run→sev-medium. Kind icons (lucide): Bot · Repeat · Columns3 · ShieldCheck ·
  UserCheck · Zap · Terminal.
- Gate nodes: dashed 1px border. Action nodes: mono tool-name headline.
- Edges: 2px source-kind accent; untraversed 35% alpha; amber into an awaiting
  target; marching-ants dash (7/7, ~0.7s) into running/awaiting targets.
- Motion: expanding ring-pulse (~1.7s) for running/awaiting; 2.4s loop spin on
  panel rounds; all `accessibilityReduceMotion`-guarded.
- Selection: 2px brand-500 ring on the selected node (app-only; web has none).

## Transcript (added 2026-08-25)
- Turns: earlier collapsed to snippet + pills, last open. Assistant prose =
  markdown (sans); code/ids/diff = mono. Thinking: dim, 2px left border quote.
- Code highlighting: HighlighterSwift (highlight.js) — the ONE third-party
  Swift dependency, exact-pinned; theme pair chosen to match
  `components/codeHighlight.css`, background overridden to `surface`.
- Diff rows: add=ok-bg/ok · del=err-bg/err · hunk=ink-mute. Target/matched
  lines in previews and CST: warn-bg tint.
