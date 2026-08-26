# rupu.app macOS — Design Alignment to the CP v2 Design System

**Date:** 2026-08-24 · **Amended:** 2026-08-25 (sidebar sub-items; Plans 3–4 added)
**Status:** Approved (matt, 2026-08-24; amendment approved 2026-08-25) — design session; subagent execution pre-authorized
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

- **Shell** *(amended 2026-08-25 — matt rejected the flat rail: the web's
  sub-item navigation must survive)*: 204pt rail, v2 IA **with native
  disclosure sub-items** — Overview · Activity (▸ Agents / Workflows /
  Autoflows / Sessions) · Projects · Security (▸ Findings / Coverage /
  Network) · Library (▸ Agents / Workflows / Autoflows) · Fleet (▸ Hosts /
  Workers) · Usage; Settings pinned bottom; host footer ("N hosts / M down",
  using the existing hosts machinery). A parent row both navigates (to its
  page's default tab) and discloses; a child row deep-links to the page with
  that kind tab selected — the route state machinery already models kind tabs.
  Disclosure state persists (UserDefaults, per group), defaulting open when
  the group contains the active route; a parent whose child is active tints
  brand (the web v1 "contains active" idiom). Active row: surface fill +
  inset 2px left brand accent; child rows indented one level, same 30pt
  height. This is "web look, native feel" applied to IA: v2 visual language,
  the v1 tree's reachability. *(Plan 2 shipped the flat rail before this
  amendment; the sub-items retrofit executes as Plan 3 Task 0.)*
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

## 4. Plan 3 — Run-graph parity *(added 2026-08-25)*

The web run graph's visual system, absent from the app, becomes its own plan.
Sources of truth: `lib/runGraphModel.ts` (state merge), `components/graph/*`
(node treatments), `workflow-editor/kindVisuals.ts` + `graph/stepStyle.ts`
(palette), `styles.css` `rg-*` keyframes (motion).

- **Two-channel rule** on every node: the 3px top accent bar + kind pill carry
  the KIND color; the glyph badge + state label carry the STATE color. Kind
  accents: step→status.running · for_each→brand-500 · parallel→sev.critical ·
  panel→status.awaiting · gate→status.paused · action→sev.info ·
  run→sev.medium. Kind icons (lucide): Bot / Repeat / Columns3 / ShieldCheck /
  UserCheck / Zap / Terminal — five new icons join the Plan 1 set.
- **Node treatments**: step card (accent bar, 15pt state-glyph badge, id, state
  label, kind pill, agent chip, pending at 75% opacity); gate = dashed border +
  `auto` tag + `↳ on reject` caption; action = mono tool headline + id caption;
  parallel = container tinted accent@8% with per-sub-step state chips; fan-out
  = three presentations (state-aware placeholder when 0 units; clickable
  unit-square grid ≤12; collapsed progress card with %, gradient bar, density
  preview and expand button >12); panel = round r/max counter, spinning ↻ while
  running, gate-condition line, clickable panelist chips.
- **Edges**: 2px, source-kind accent; untraversed ghosted at 35%; amber when
  the target awaits approval; marching-ants dash animation into
  running/awaiting targets. Topology stays the linear chain (parallelism lives
  inside container nodes — same as the web).
- **State model**: `NodeState` gains `paused`; live patching extends to unit
  events, panel rounds, and pause/resume. Merge precedence stays live > step
  results > pending (never inferred from position).
- **Selection**: tapping a node (or fan-out unit) drives the Plan 2 tabbed
  panel via the existing `focusStep`; selection is seeded once (a running node
  with a transcript, else the last with one) and never stolen by live updates.
  The selected node gets a real visual highlight (2px brand ring) — the web
  has none (text-only "selected:" line); the app deliberately exceeds it here.
- Motion: ring-pulse for running/awaiting, loop spin — all reduced-motion
  guarded. Graph area grows to ≈420pt to match the web.

## 5. Plan 4 — Transcript & rich rendering *(added 2026-08-25)*

The web transcript's rendering system, ported. Sources of truth:
`components/transcript/*` (Turn, ToolCard, Markdown, DiffView, TerminalBlock,
FindingCard, SourcePreview, AstTree, StructuredView), `transcriptView.ts`
(pairing model), `lib/transcript.ts` (event payloads).

- **Dependency carve-out (matt, 2026-08-25)**: HighlighterSwift
  (github.com/smittytone/HighlighterSwift — highlight.js via JavaScriptCore,
  MIT) becomes the ONE sanctioned third-party Swift package, for syntax
  highlighting parity with the web's highlight.js. Rule 4 of the macOS section
  in CLAUDE.md is amended accordingly; the pin is exact-version.
- **Turn structure**: events group into turns (`turn_start`/`turn_end`, with an
  assistant-message-boundary fallback for older transcripts); earlier turns
  collapse to a snippet row with tool-count / finding-count / result pills;
  the last turn opens by default. The app's auto-scroll live tail is kept (the
  web lacks it — again deliberately exceeded).
- **Markdown**: native block splitter (headings, paragraphs, lists, quotes,
  fenced code) + `AttributedString(markdown:)` inline parsing; fences render
  in a CodeBlock view highlighted by HighlighterSwift. GFM tables render as a
  mono block this plan (honest fallback, noted in the checkpoint).
- **Tool cards**: a Swift port of `transcriptView.ts` pairing semantics —
  result→call by `call_id`, `file_edit`/`command_run` by adjacency,
  `tool_audit` by FIFO per tool name, `action_emitted` merged onto its audit —
  with ToolKind classification (finding / read / grep / glob / diff / terminal
  / subrun / coverage / ast_grep / generic). Header = mono tool name + derived
  input summary + AuditBadge (blocked / not granted / audited) + StatusBadge
  (error / duration / ok). Requires decoding the `tool_audit` and
  `action_emitted` payloads the Phase 2 client deliberately skipped.
- **Bodies**: DiffView (unified-diff parse; `file_edit.diff` is finally
  rendered — today it is decoded and discarded), TerminalBlock, read/grep/glob
  mono views, StructuredView (recursive JSONValue renderer), sub-run link,
  FindingCard (severity bar/pill, location chip, rationale markdown), and the
  full ast_grep rich body: per-file groups, metavariable-highlighted snippet,
  bindings table, inline SourcePreview (line-numbered, target line tinted
  warn-bg, per-line highlighting) and AstTree (CST viewer, matched node tinted
  + ancestors auto-expanded). The source/CST client surface (`runSource` /
  `runAst`, `SourceModels.swift`, the preview views) shipped with Phase 6B and
  is reused; what's new is the metavar extension of `AstGrepTranscriptParsing`
  (deliberately omitted then), the tool_audit/action_emitted payload decode,
  and fixture coverage for both via the drift rig.
- Session detail and agent-run detail inherit everything (same feed).

## 6. Verification & sequencing

- Store/contract layer untouched: all 242 Swift + 28 Rust tests pass throughout
  (except token-value and sidebar-mapping test updates belonging to the plans).
- Visual verification: build + matt side-by-side passes at the plan
  checkpoints; screenshots where the screen allows.
- Order: Plan 1 (language) → Plan 2 (flows) → Plan 3 (graph) → Plan 4
  (transcript). Plans 3–4 consume Plan 1's tokens/icons and Plan 2's tabbed
  panel; matt checkpoints after each plan.
- Phase 4 (dashboard) starts only after this arc; Overview follows the v2
  arc's intent (instrument strip + needs-you queue), giving the web a
  reference for its own unbuilt Plan 3.

## 7. Out of scope

Web-side changes; new API surface; Situation Room (Phase 6); light-theme redesign
beyond porting both existing theme tables; menu bar extra; saved views; Library
screen. Web-side gaps found during the 2026-08-25 inventory (no selected-node
highlight in the web graph; no transcript live-tail pinning; no run-detail
search) are noted for a future web arc, not fixed here.
