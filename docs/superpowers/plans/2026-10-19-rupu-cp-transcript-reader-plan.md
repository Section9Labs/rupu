# CP Transcript Reader Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the CP's nested-box transcript rendering with one continuous reading column (spine, per-event glyphs, one-line expandable tool rows, filters, expand/collapse-all, outline rail) that every transcript surface inherits through the shared components.

**Architecture:** Pure view model (`buildTranscriptView` → `{ header, self, turns, footer }`) is unchanged. We rewrite only the rendering layer: a new event-kind helper, a rewritten one-line `ToolCard`, a rewritten feed-row `Turn`, new `TranscriptControls` + `TranscriptOutline` components, and a rewritten `TranscriptPanel` that keeps its exact public prop surface (so all mount points are untouched) plus one new optional `costUsd` prop. A small React context carries the active filter and the expand/collapse-all signal down to rows.

**Tech Stack:** React 18 + TypeScript, Tailwind (CP tokens only, static class strings), Vitest + Testing Library, lucide-react icons.

**Spec:** `docs/superpowers/specs/2026-10-19-rupu-cp-transcript-reader-design.md` (visual reference: `docs/superpowers/specs/2026-10-19-rupu-cp-transcript-reader-mockup.html` — open it in a browser before building).

## Global Constraints

- Frontend rendering only. Do **not** change `buildTranscriptView`, the `TranscriptView`/`TurnView`/`ToolView`/`TurnBlock`/`MessageView`/`FindingView` types, the event schema, or any backend endpoint.
- `buildTranscriptView`'s own tests (`transcriptView.test.ts`) must stay green and untouched.
- Tailwind: **static class strings only** — no interpolated/template class names. Colors come from existing tokens only (`ink`/`ink-dim`/`ink-mute`, `brand-*`, `sev-*`, `status-*`, `ok`/`err`/`warn`, `surface`/`panel`/`border`/`bg`). No new raw hex.
- `No any.` Mono (`font-mono`) for code/paths/IDs/commands/verbs; sans for prose/labels.
- Preserve these contracts exactly: `data-call-id` on each tool card (netflow `#call-<id>` link), `#msg-<id>` message-bubble anchor + scroll, `onOpenTranscript` sub-run open-in-place, `embedded` prop, live streaming (`mergeSnapshotAndStream`, SSE subscribe, `onComplete`).
- `TranscriptPanel`'s existing public props (`path`, `live`, `host`, `runId`, `embedded`, `onComplete`) keep their names and meaning; only **add** optional props.
- Verify in a real browser (desktop, phone width, light + dark, live + completed) before claiming done — green tests ≠ rendering done (project GUI rule).

## Review Focus

- **Redacted or empty thinking** (`thinking.text === null` or `''`): must render the quiet "[redacted reasoning · provider]" marker, never an empty block — pinned in Task 3.
- **A tool call with no output and no error, still running** (live stream, `durationMs`/`output`/`error` all absent): row must show a neutral "running" chip and an empty-but-valid expandable body, not a crash or a blank chip — pinned in Task 2.
- **`unknown`-kind block** (newer rupu wrote the transcript): must still render the labelled fallback row with its raw JSON, nothing silently dropped — pinned in Task 3.
- **A turn filtered out entirely** (filter = Findings, a turn with no finding): its turn marker and spine segment must not leave a dangling empty block or a numbered marker with nothing under it — pinned in Task 4.
- **Headerless / empty transcript** (no `run_start`; `view.header` null, zero turns): panel must show the existing empty state and any data-quality badges, not a bare spine — pinned in Task 5.

---

## Task 1: Event-kind taxonomy, counts, and glyphs (pure)

**Files:**
- Create: `crates/rupu-cp/web/src/components/transcript/eventKind.ts`
- Test: `crates/rupu-cp/web/src/components/transcript/eventKind.test.ts`

**Interfaces:**
- Consumes: `TurnBlock`, `ToolView`, `TurnView` from `./transcriptView`.
- Produces:
  - `type FilterKind = 'all' | 'said' | 'thinking' | 'commands' | 'findings'`
  - `type EventKind = 'message' | 'thinking' | 'said' | 'user' | 'command' | 'finding' | 'tool' | 'quiet'` — the glyph/row class of one block.
  - `function blockEventKind(block: TurnBlock): EventKind` — maps a block to its kind. `tool` block with `view.kind === 'finding'` → `'finding'`; with `view.kind === 'terminal'` → `'command'`; other tool kinds → `'tool'`. `assistant` → `'said'`; `thinking` → `'thinking'`; `user` → `'user'`; `message` → `'message'`; `gate`/`seed`/`notice`/`meta`/`compaction`/`outcome`/`recovery`/`assistant_block`/`injected`/`unknown` → `'quiet'`.
  - `function blockMatchesFilter(block: TurnBlock, filter: FilterKind): boolean` — `all` → true; `said` → kind `said`; `thinking` → kind `thinking`; `commands` → kind `command`; `findings` → kind `finding`.
  - `function filterCounts(turns: TurnView[]): Record<Exclude<FilterKind,'all'>, number>` — totals across all blocks of all turns.

- [ ] **Step 1: Write the failing tests** in `eventKind.test.ts`:
  - `blockEventKind` returns `'command'` for `{kind:'tool', view:{kind:'terminal',…}}`, `'finding'` for a `finding` tool view, `'tool'` for a `read` tool view, `'said'` for `assistant`, `'thinking'` for `thinking`, `'quiet'` for `notice`.
  - `blockMatchesFilter` returns true only for the matching kind and always for `'all'`.
  - `filterCounts` over a two-turn fixture (1 assistant, 1 thinking, 1 terminal tool, 1 finding tool, 1 read tool) returns `{ said:1, thinking:1, commands:1, findings:1 }`.
- [ ] **Step 2: Run to verify they fail**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/eventKind.test.ts`
  Expected: FAIL (module not found).
- [ ] **Step 3: Implement `eventKind.ts`** with the signatures above. Pure functions, no React.
- [ ] **Step 4: Run to verify they pass**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/eventKind.test.ts`
  Expected: PASS.
- [ ] **Step 5: Commit** — `feat(cp-web): event-kind taxonomy + counts for transcript reader`

---

## Task 2: One-line expandable ToolCard

**Files:**
- Create: `crates/rupu-cp/web/src/components/transcript/TranscriptViewContext.tsx` (context + `TranscriptViewProvider` + `useTranscriptView` hook, with the no-provider default — this is the first consumer, so it is created here and only *used* by Tasks 3–5)
- Modify (rewrite the card chrome, keep the body sub-components): `crates/rupu-cp/web/src/components/transcript/ToolCard.tsx`
- Modify: `crates/rupu-cp/web/src/components/transcript/ToolCard.test.tsx`

**Interfaces:**
- Consumes: `ToolView` (`./transcriptView`), `summarizeInput` (keep, exported), `formatDuration` (`../../lib/duration`), `FindingCard`, `DiffView`, `TerminalBlock`, `StructuredView`, `AstGrepBody`, the existing body sub-components (keep them; only the outer wrapper changes).
- Produces (context, for Tasks 3–5): `interface TranscriptViewState { filter: FilterKind; expandTarget: 'open' | 'closed' | null; expandNonce: number }`, `useTranscriptView(): TranscriptViewState` (default `{ filter:'all', expandTarget:null, expandNonce:0 }` when no provider), and `TranscriptViewProvider`. `ToolCard` reads this for the expand-all signal, defaulting to local state.
- Produces: default export `ToolCard` with the SAME props it has today (`{ tool, onOpenTranscript?, runId?, host? }`) plus it reads the expand context. Still renders the outer `data-call-id` wrapper.

**Approach:** Replace `CardHeader` + always-rendered body with a single clickable **row** (`<button>`, `aria-expanded`) — verb (mono bold), target summary (mono dim, truncate), a result **chip** (`ok` / `error` / `running` / the subrun/claim count when present), a duration/byte meta — and an **inline body** rendered only when expanded (lazy: the body sub-component mounts on first open). `finding`-kind keeps rendering `FindingCard` with no row wrapper (as today). `terminal`-kind row shows `$ command` and the exit-code chip. Remove the fixed `max-h-96`/`max-h-72` inner scrollers from `ReadBody`/`GrepBody`/`GlobBody` in favor of a single generous threshold constant (e.g. `max-h-[32rem]`) applied once, only past which a body scrolls. Expand state: local `useState`, seeded/overridden by the context's `expandTarget` + `expandNonce` (when the nonce changes, set open = `expandTarget === 'open'`).

- [ ] **Step 1: Rewrite the tests** in `ToolCard.test.tsx` to the new structure:
  - A `read` tool renders a row with verb `read_file` and the target summary; the output marker is **not** in the DOM until the row is clicked, then it is (expand on click).
  - A tool with `error` set renders an `error` chip and, when expanded, the error text.
  - A `terminal` tool renders `$ <command>` and the exit-code chip.
  - A running tool (`output`/`error`/`durationMs` all absent) renders a neutral `running` chip and does not throw. *(Review Focus)*
  - A `finding` tool still renders `FindingCard` content (no row wrapper).
  - The wrapper still carries `data-call-id` equal to `tool.callId`.
  - `summarizeInput` unit tests (keep the existing ones).
- [ ] **Step 2: Run to verify failures**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/ToolCard.test.tsx`
  Expected: FAIL.
- [ ] **Step 3: Implement the rewrite** per Approach. Keep `summarizeInput`, `blockedTitle`, `AuditBadge`, `StatusBadge` semantics (audit badge still shows for blocked/not-granted). Static classes only.
- [ ] **Step 4: Run to verify pass**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/ToolCard.test.tsx`
  Expected: PASS.
- [ ] **Step 5: Commit** — `feat(cp-web): one-line expandable tool rows`

---

## Task 3: Feed-row Turn with spine marker and per-block glyphs

**Files:**
- Modify (rewrite): `crates/rupu-cp/web/src/components/transcript/Turn.tsx`
- Modify: `crates/rupu-cp/web/src/components/transcript/Turn.test.tsx`

**Interfaces:**
- Consumes: `TurnView`, `TurnBlock` (`./transcriptView`), `blockEventKind` + `blockMatchesFilter` (Task 1), `ToolCard` (Task 2), `useTranscriptView()` (Task 2's context), `MessageBubble`/`InjectedChip` (keep), `Markdown` (keep).
- Produces: default export `Turn` with props `{ turn: TurnView; turnNumber: number; showMarker: boolean; tokensLabel?: string; onOpenTranscript?; runId?; host?; self?; highlightMsg? }`. Renders a turn-head row (marker when `showMarker`, "Turn N" + token label, a spine rule) then its blocks in order through a per-kind `TurnBlockView`, each with its gutter glyph. Honors the active filter from context (a block that doesn't match is not rendered); returns `null` (no marker, no spine segment) when the turn has zero matching blocks. *(Review Focus: filtered-out turn)*

**Approach:** Blocks render in order, each wrapped in an `.ev` row with a gutter glyph keyed by `blockEventKind`. Weights per spec: `said` = loud `ink` markdown; `thinking` = quiet `ink-dim`, shown in full, individually collapsible (its own `<button aria-expanded>`), and folded when context `expandTarget==='closed'`; redacted/empty thinking → the muted marker *(Review Focus)*; `message` = the existing `MessageBubble`/`InjectedChip`; `finding`/`tool`/`command` = `ToolCard`; `gate`/`notice`/`meta`/`compaction`/`seed`/`outcome`/`recovery`/`assistant_block` = quiet one-line rows (keep today's text, restyle); `unknown` = the labelled fallback with raw JSON *(Review Focus)*. Glyphs are lucide icons already in the bundle.

- [ ] **Step 1: Rewrite the tests** in `Turn.test.tsx`:
  - A turn with an assistant block + a read tool renders the assistant text and a tool row; the marker shows "1" when `showMarker`.
  - `showMarker={false}` renders no turn-number marker (embedded per-turn case).
  - A `thinking` block with `text: null` renders the redacted marker, not an empty block. *(Review Focus)*
  - An `unknown` block renders the "unrecognized event" fallback with its raw data. *(Review Focus)*
  - With a Findings filter in context, a turn whose only blocks are assistant+read renders `null` (nothing, no marker). *(Review Focus)*
- [ ] **Step 2: Run to verify failures**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/Turn.test.tsx`
  Expected: FAIL.
- [ ] **Step 3: Implement the rewrite** per Approach, reading the active filter and expand signal from `useTranscriptView()` (Task 2's context; its no-provider default keeps these tests simple).
- [ ] **Step 4: Run to verify pass**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/Turn.test.tsx`
  Expected: PASS.
- [ ] **Step 5: Commit** — `feat(cp-web): feed-row turns with spine + per-event glyphs`

---

## Task 4: Controls, outline rail, and the view context

**Files:**
- Create: `crates/rupu-cp/web/src/components/transcript/TranscriptControls.tsx`
- Create: `crates/rupu-cp/web/src/components/transcript/TranscriptOutline.tsx`
- Test: `crates/rupu-cp/web/src/components/transcript/TranscriptControls.test.tsx`

**Interfaces:**
- Consumes: `FilterKind`, `filterCounts` (Task 1), `TurnView`; the context/provider from Task 2.
- Produces:
  - `TranscriptControls({ counts, filter, onFilter, onExpandAll, onCollapseAll })` — the sticky filter chips (with counts) + Expand-all / Collapse-all buttons. A labelled toggle group; chips are `<button>` with `aria-pressed`.
  - `TranscriptOutline({ turns, filter })` — the thin clickable rail: one tick per turn (plus finding/command sub-ticks), scroll-spy highlight of the turn nearest the top of the scroll ancestor, click scrolls to that turn's anchor (`#turn-<n>`). Uses `IntersectionObserver` against the nearest scrolling ancestor; no fixed inner scroll of its own.
- Owner of expand/collapse: `TranscriptPanel` (Task 5) holds `{filter, expandTarget, expandNonce}` state, renders the provider, and bumps `expandNonce` (and sets `expandTarget`) when a control is clicked. Controls/outline are presentational.

- [ ] **Step 1: Write the failing tests** in `TranscriptControls.test.tsx`:
  - Renders five chips (All/Said/Thinking/Commands/Findings) with the counts passed in; the active filter chip has `aria-pressed="true"`.
  - Clicking "Commands" calls `onFilter('commands')`.
  - Clicking "Expand all" calls `onExpandAll`; "Collapse all" calls `onCollapseAll`.
- [ ] **Step 2: Run to verify failures**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/TranscriptControls.test.tsx`
  Expected: FAIL.
- [ ] **Step 3: Implement** `TranscriptControls` and `TranscriptOutline` (consuming Task 2's context). Sticky positioning (`sticky top-0`) and the `IntersectionObserver` wiring live in these components; keep them dependency-light. `prefers-reduced-motion` honored (no smooth-scroll travel when reduced).
- [ ] **Step 4: Run to verify pass**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/transcript/TranscriptControls.test.tsx`
  Expected: PASS.
- [ ] **Step 5: Commit** — `feat(cp-web): transcript filter controls, outline rail, view context`

---

## Task 5: TranscriptPanel chrome assembles the reader

**Files:**
- Modify (rewrite the render; keep the fetch/stream effects intact): `crates/rupu-cp/web/src/components/TranscriptPanel.tsx`

**Interfaces:**
- Consumes: everything above + the unchanged `buildTranscriptView`, `mergeSnapshotAndStream` (keep), `parseMsgHash`, `AgentName`.
- Produces: default export `TranscriptPanel` with props `{ path, live, host?, runId?, embedded?, onComplete?, costUsd? }` — **new** optional `costUsd?: number`. All existing props unchanged.

**Approach:** Keep the fetch effect, the live-tail SSE effect, `mergeSnapshotAndStream`, `onComplete`, the `unparsed`/`partial` badges, and `openTranscript` exactly as they are. Rewrite only the returned JSX:
- Non-embedded: run header (as today, via `view.header`/`AgentName` + live pill) → **metrics strip** (turns · tool calls · commands · findings · tokens; `costUsd` appended only when provided — `$X.XX`) → `TranscriptViewProvider` wrapping `TranscriptControls` (sticky) + a two-column layout of the **feed** (the `Turn` list, each with `showMarker` and an `id="turn-<n>"` anchor) and `TranscriptOutline`. Counts from `filterCounts(view.turns)`. Tool/command counts for the metrics strip reuse `filterCounts`.
- Embedded: render the feed alone (provider + `Turn` list), no header, no metrics, no controls, no outline. Add a `perTurn` notion: SessionConversation mounts one panel per turn — expose it by passing `showMarker={!embedded}` so embedded turns omit the "Turn N" marker (matches the design's per-turn suppression). Whole-run embedded tabs still get markers because they are not `embedded` (they use the full panel in a tab). *(If a whole-run surface sets `embedded`, markers are off — acceptable; revisit only if the gate shows it reads poorly.)*
- Empty/headerless: keep the existing empty-state and badge behavior. *(Review Focus)*
- Numbers: tokens render with `toLocaleString()` (thousands separators) as today; cost uses `formatCost` from `../lib/usage`.

- [ ] **Step 1: Extend the existing `components/TranscriptPanel.test.tsx`** — it already asserts header-shown-by-default and header-hidden-when-embedded. Update those for the new chrome and add: a completed-run event list renders the metrics strip; `embedded` hides the header and metrics strip and the controls; the empty/headerless state still renders. Keep the `mergeSnapshotAndStream` unit tests. (Heavy logic is covered in Tasks 1–4.)
- [ ] **Step 2: Run to verify current suite state**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/TranscriptPanel.test.ts src/components/transcript/`
  Expected: the new assertions FAIL (pre-implementation).
- [ ] **Step 3: Implement** the JSX rewrite per Approach. Thread `expandTarget`/`expandNonce`/`filter` state here and bump the nonce on Expand/Collapse-all. Preserve `data-call-id`, `#msg-<id>` (the highlight scroll), `id="turn-<n>"` anchors, and sub-run open.
- [ ] **Step 4: Run tests**
  Run: `cd crates/rupu-cp/web && npx vitest run src/components/TranscriptPanel.test.ts src/components/transcript/`
  Expected: PASS.
- [ ] **Step 5: Commit** — `feat(cp-web): assemble transcript reader in TranscriptPanel`

---

## Task 6: Cost wiring + full typecheck + browser gate

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/RunDetail.tsx` (pass `costUsd` to its `TranscriptPanel`(s) from the already-computed `displayUsage`)

**Interfaces:**
- Consumes: `displayUsage` (already in `RunDetail`, `displayUsage.cost_usd: number | null`).
- Produces: nothing new; a prop pass-through.

- [ ] **Step 1: Wire cost** — in `RunDetail.tsx`, pass `costUsd={displayUsage?.cost_usd ?? undefined}` to **both** `TranscriptPanel` mount sites (the step transcript and the selected transcript, ~lines 1374–1392). `TranscriptPanel` formats it with `formatCost` from `lib/usage`. Leave every other mount point unchanged (they omit `costUsd`, so the strip shows tokens only).
- [ ] **Step 2: Typecheck + full test run**
  Run: `cd crates/rupu-cp/web && npx tsc -b --noEmit && npx vitest run`
  Expected: clean typecheck, all green.
- [ ] **Step 3: Build the web bundle**
  Run: `cd crates/rupu-cp/web && npm run build`
  Expected: succeeds.
- [ ] **Step 4: Browser gate (looked at, not just test-green).** With a local `rupu cp serve` running, open against real transcripts and confirm each reads correctly and the contracts hold:
  - Standalone `/transcript` page (a 50+ turn real run): feed reads top-to-bottom; filters (All/Said/Thinking/Commands/Findings) work; Expand-all/Collapse-all toggles every tool body and thinking; outline rail highlights position and jumps on click.
  - A run-detail transcript tab; a session conversation (per-turn, no stray turn markers, no N filter bars); an agentiflow transcript tab; a per-step browser; situation-room `EventCard`.
  - `#call-<id>` (netflow "transcript" link) lands on the right row; `#msg-<id>` lands on the right bubble; a sub-run link opens in place.
  - Light + dark theme; phone width (the column + rail degrade cleanly); a live/running run streams and completes.
  - The largest real run (78+ turns): no obvious jank; if there is, note it for the virtualization follow-up (out of scope here).
- [ ] **Step 5: Commit** — `feat(cp-web): wire run cost into transcript metrics; reader gate`

---

## Notes for the executor

- Run one test file at a time with `npx vitest run <path>` from `crates/rupu-cp/web` (the repo's targeted-tests convention; don't cold-run the whole Rust workspace).
- The `.claude` cargo sweep hook doesn't apply to the web package; these are npm/vitest commands.
- Keep each task's commit scoped to its files. This is a cohesive branch; a whole-branch review follows the last task.
