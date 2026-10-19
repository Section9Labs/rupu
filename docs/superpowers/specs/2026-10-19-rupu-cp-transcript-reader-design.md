# CP transcript reader — a readable, scannable transcript surface

**Status:** design, pending approval
**Date:** 2026-10-19
**Surface mode:** Operate (an app screen worked in, not read once)

## Intent and the why

Today the CP renders an agent's transcript as nested boxes: the panel is a box, each
turn is a collapsible box, and each tool call is a third box with its own header bar.
The turn list summarizes each turn by its first assistant sentence — but most turns on a
real run say nothing themselves (they only run tools), so the list collapses to a wall of
identical "no assistant message" rows. A 27-turn run shows 26 of them. Opening a turn
pours whole files into fixed-height scroll areas that trap the mouse, and signal
(a finding, what the agent concluded) sits at the same visual weight as noise (a
`provider_retry` notice, a thinking toggle).

matt's ask, in his words: *"I want to read the transcripts … browse through what the agent
was thinking, what it did, what commands it ran, which tool calls it made — a FULL
transcript."* The current surface is built for folding things away; he wants one built for
reading.

This redesign replaces the nested-box rendering with **one continuous reading column with a
left spine** — a transcript read top-to-bottom like a document, with a scannable event rail
so you can see the *shape* of a run at a glance and jump through a long one. A validated
mockup (`2026-10-19-rupu-cp-transcript-reader-mockup.html`, in this directory — shown and
approved 2026-10-19) is the reference for the look. Open it in a browser before building;
the prose below is the contract, the mockup is the picture of it.

**The lever:** every transcript surface in the app renders through one shared set of
components (`TranscriptPanel` → `Turn` → `ToolCard` and the per-block views). Rebuilding
that surface upgrades the standalone transcript page, run detail, sessions, agentiflow
tabs, and the per-step browser **all at once**. This is a rendering-layer redesign: the
pure view model (`buildTranscriptView` → `{ header, self, turns, footer }`, with
`turns[].blocks` already ordered and typed) is correct and stays. We change how that model
is painted, not how it is built.

## Goals

- A transcript reads as **one continuous column**, top to bottom, no nested card-in-card-in-card.
- You can **scan what happened** without reading every word: each event carries a gutter
  glyph keyed to its kind (message, thinking, said, command, tool, read, finding), and the
  turns are numbered markers on a vertical spine.
- The reading weights are right: **assistant prose is loudest; thinking is quiet but shown
  in full; tool/command calls are one scannable line that expands inline; findings carry the
  only color.**
- **Navigate a long run (78+ turns):** a thin clickable outline rail (turn / finding /
  command tick marks, current position highlighted), top **filter chips** (All · Said ·
  Thinking · Commands · Findings), **Expand-all / Collapse-all**, and **Jump-to**.
- Tool and command output **expands inline** in the flow — never a fixed-height scroll box
  that traps the page scroll.
- **Applies to every transcript surface in the app** through the shared components.
- Works **live** (streaming run) and for completed runs, light and dark theme, and down to a
  phone-width column.
- Honors the CP's existing design tokens (`styles.css` / `tailwind.config.ts`) — it looks
  like the same app, not a bolt-on.

## Non-goals

- **No change to the transcript data model, the event schema, `buildTranscriptView`, or any
  backend endpoint.** This is frontend rendering only. (If a block kind turns out to lack a
  field the new rail needs, that is a follow-up, not part of this work.)
- **No new navigation routes or pages.** Same `/transcript` route and same embed points.
- No redesign of the finding *report* page (`/findings/:id`), the run graph, netflow, or any
  non-transcript surface. `FindingCard` inside the flow is restyled to sit in the column; the
  standalone report page is out of scope.
- No virtualization/windowing in this pass unless the gate shows a real perf problem on the
  largest real transcripts (see Risks). Keep it a clean DOM first.
- No motion beyond CP-standard feedback (expand/collapse, hover, the live pulse). This is an
  Operate screen; motion is feedback, not decoration.

## Anti-goals (these would make it a failure even if it technically works)

- Bringing back per-turn collapse as the *default* reading state — the wall of collapsed
  rows is the thing we are removing. Turns are open; it is big *outputs* that fold.
- Hiding thinking behind a click by default. matt explicitly wants to read it.
- Three levels of borders. The column uses whitespace, a spine, and tone steps — a tool row
  gets one border, a finding one border, and that is the budget.
- Color as chrome. Severity color appears on findings; status color (running/ok/error)
  appears on result chips and the live turn marker. Nowhere else.
- A fixed-height inner scroll region anywhere in the reading flow.

## What it looks like (the approved mockup, in words)

Reference: the committed mockup (`…-mockup.html`). The CP sidebar and page chrome are
unchanged. Inside the canvas, a reading column (max ~860px) holds:

1. **Run header** — the agent/codename, provider · model, a status line (live pulse when
   streaming), start time and duration. Crew codename in the mono brand chip, as today.
2. **Metrics strip** — turns · tool calls · commands · findings · tokens/cost, as plain
   tabular numbers between two hairlines (no KPI tiles, no icons-in-circles, per app-screen
   rules).
3. **Sticky controls** — the filter chips (with per-kind counts), Expand-all / Collapse-all,
   Jump-to. Sticks to the top of the canvas as you scroll.
4. **The feed** — a left **spine** (2px rule) with:
   - **Turn markers:** numbered circles on the spine; the live/running turn's marker glows
     in `status-running`.
   - **Event rows,** each with a small **gutter glyph** on the spine keyed to kind:
     - *message* (operator/lead): a tinted brand card — directives and operator messages.
     - *thinking:* quiet, indented, `ink-dim`, shown in full, with a small "Thinking" label;
       click collapses it to one line (and Collapse-all does too).
     - *said* (assistant prose): the loudest text, `ink`, ~14px, brand glyph.
     - *tool call:* one scannable row — `verb` (mono, bold) · target (mono, dim, truncates) ·
       a result chip (`ok` / `error` / `N granted` / `running`) · duration. Click the row to
       expand its input + output inline (bordered body, no fixed-height trap — tall output
       gets a generous max-height with its *own* scroll only past a threshold, and Expand-all
       reveals it).
     - *command* (`bash`): same row shape, rendered as a terminal line (`$ cmd`), exit-code
       chip, byte count; body shows cwd + output.
     - *finding:* a severity card in the flow — the one place color lives — severity pill,
       title, rationale, and the target (`host · port · path`) in mono.
     - *skill load / notice / gate / compaction / seed / outcome / recovery / unknown:* quiet
       one-line rows with a muted glyph; nothing silently vanishes (the `unknown` fallback
       stays, per the schema-forward-compat rule).
5. **Outline rail** (long runs) — a thin, clickable column of tick marks (turns, findings,
   commands) with the current scroll position highlighted; clicking scrolls to that anchor.

## The parts of the how that are decided

- **Shared components, one surface.** Rework the existing
  `crates/rupu-cp/web/src/components/transcript/` components so every mount point inherits the
  new look. Mount points that must keep working unchanged in behavior:
  `pages/RunTranscript.tsx`, `pages/RunDetail.tsx`, `pages/SessionDetail.tsx`,
  `pages/AgentiflowDetail.tsx`, `components/run/StepTranscriptBrowser.tsx`,
  `components/agentiflow/TranscriptBrowser.tsx`, `components/session/SessionConversation.tsx`,
  and `components/situationRoom/EventCard.tsx`. The `embedded` prop stays the switch for
  "drop the run header/metrics strip and the outline rail" (sessions, step browser,
  agentiflow, situation room); the feed rendering itself is identical everywhere.
- **View model unchanged.** `buildTranscriptView`, `TranscriptView`, `TurnView`, `ToolView`,
  `TurnBlock` keep their shapes. The redesign consumes them. Per-kind counts for the filter
  chips are derived in the component from the existing blocks; if that derivation is non-trivial
  it goes in a small pure helper next to `turnSeries.ts` with its own unit test — not in
  `buildTranscriptView`.
- **Default reading state (my calls, matt may veto):**
  - Turns: **open**, always visible. No per-turn collapse of the whole turn.
  - Thinking: **shown in full**, quiet style; individually collapsible; Collapse-all folds it.
  - Tool + command output bodies: **collapsed by default**, expand on row click; Expand-all
    opens every body (and un-collapses thinking); Collapse-all closes every body.
  - Findings: **always expanded.**
  - Live run: the newest turn is in view; new events append at the bottom (today's
    merge-snapshot-and-stream behavior and the `onComplete` hook are preserved exactly).
- **Filters** are a client-side view toggle over the already-built blocks: "Commands" shows
  only command rows (and their turn markers), etc. "All" is the default. Filtering never
  mutates or refetches.
- **Tokens/counts** render with thousands separators and tabular figures (the raw
  `52106 in · 186 out` becomes `52,106 in · 186 out`), via the existing number formatting.
- **Styling** uses the existing Tailwind tokens only (`ink`/`ink-dim`/`ink-mute`, `brand-*`,
  `sev-*`, `status-*`, `ok`/`err`/`warn`, `surface`/`panel`/`border`). No new raw hex. Static
  class strings only (the codebase rule: no interpolated Tailwind). Mono for code, paths, IDs,
  commands, verbs; sans for prose and labels.
- **Accessibility:** expand/collapse rows are real `<button>`s with `aria-expanded`; the
  outline rail entries are links/buttons that move focus to the anchored turn; filter chips are
  a labeled toggle group; focus-visible rings as elsewhere in the CP; honors
  `prefers-reduced-motion` (no travel, keep the state change).
- **Anchors preserved.** The existing `#msg-<id>` (message bubble), `#call-<id>`
  (`data-call-id` on tool cards, used by the netflow "transcript" link), and sub-run
  transcript open-in-place behavior must all still work. These are contracts other surfaces
  depend on — the redesign keeps the same anchor attributes.
- **Tests.** The component tests under `components/transcript/*.test.tsx` are rewritten
  alongside the components they cover (they assert on rendered structure, which changes).
  New pure helpers get their own unit tests. `buildTranscriptView`'s tests are untouched
  (its output is unchanged). Verify in a real browser against real transcripts at desktop,
  the embedded session view, and phone width, light and dark, before claiming done.

## Decisions settled after builder review (my calls — matt may veto any)

- **Wrapper vs. feed split.** The run header, metrics strip, sticky controls (filter chips,
  Expand-all / Collapse-all), and the outline rail are a **non-embedded wrapper** around a
  shared **feed** component. `embedded` renders the feed alone. This is load-bearing because
  `SessionConversation` mounts **one `TranscriptPanel` per turn, stacked**, and `EventCard`
  mounts one in a dense card — we must not get N filter bars and N outline rails down a
  session page. So: `embedded` drops header, metrics, controls, **and** the outline rail.
- **Turn-number spine in embedded mounts.** In a per-turn embedded mount (SessionConversation)
  the numbered turn marker is **suppressed** — the session already shows its own turn
  structure, so the feed renders that turn's blocks on the spine without a "Turn N" circle.
  In a whole-run embedded mount (a run-detail tab) the spine and markers stay. The switch is
  "does this mount render one turn or many," exposed as a small prop; the builder picks the
  spelling.
- **Scroll root.** The panel stays in page flow and does **not** introduce its own inner
  scroll region (that would reintroduce the fixed-height trap). The sticky controls and the
  scroll-spy outline rail attach to the **nearest scrolling ancestor** (`main.overflow-auto`
  in the standalone and run-detail cases). This must behave in the tabbed embeds; verify at
  the gate.
- **No separate "Jump-to" control.** The outline rail *is* the jump mechanism (click a tick to
  scroll to it). Drop the extra Jump-to control from the mockup; it was redundant.
- **Cost in the metrics strip.** The transcript view model exposes `totalTokens` only (no
  usd). The strip shows **tokens always**; **cost only where the host page already has it**
  (e.g. RunDetail already loads the run's `usage.cost_usd`) and passes it in as a prop. The
  feed never fabricates or fetches cost. If a mount has no cost, the strip shows tokens alone.

## What is left to the builder

- The exact DOM/component decomposition (e.g. whether the outline rail is its own component
  reading turn anchors, how the sticky controls are structured) — build it the way that keeps
  the shared components clean and the mount points unchanged.
- The precise glyph set (lucide icons already in the bundle) per event kind, matching the
  mockup's intent.
- The max-height threshold past which a very long single tool output gets its own inner
  scroll, and whether a "copy" affordance rides on command/output bodies (nice-to-have).
- The outline-rail interaction details (hover labels, how dense the tick marks get on a
  78-turn run, scroll-spy implementation).
- Whether Expand-all/Collapse-all and the active filter persist across a route change within a
  session (sensible default: reset per transcript; do the simplest correct thing).
- Dark-theme and phone-width refinements that only show up once it is rendered.

## Risks / open checks

- **Perf on the largest real transcripts.** A 78-turn run with hundreds of tool results is a
  lot of DOM if every body renders even while collapsed. Mitigation: render collapsed bodies
  lazily (only on expand); if the gate still shows jank on the biggest real run, windowing is
  the follow-up — not pre-built here.
- **Filtering + live streaming interaction.** A filter applied while a run streams must keep
  working as new events arrive; keep filtering a pure view over the current block list so new
  blocks just flow through it.
- **Situation-room `EventCard`** embeds transcript rendering in a denser context; confirm the
  feed still reads well there or that `embedded` trims enough. Check it explicitly in the gate.

## Verification

Build is not done until, on the real CP against real transcripts:
- The standalone `/transcript` page, a run-detail transcript tab, a session conversation, an
  agentiflow transcript tab, and a per-step browser all render the new reader correctly.
- Expand-all / Collapse-all, the filter chips, Jump-to, and the outline rail all work on a
  long (50+ turn) real run.
- `#msg-<id>` and `#call-<id>`/netflow "transcript" links still land on the right row; sub-run
  open-in-place still works.
- Light and dark theme, desktop and phone width, live and completed runs all look right
  (looked at, not just test-green — per the project's GUI rule).
- `npm test` green for the transcript components; `tsc` clean; no interpolated Tailwind.
