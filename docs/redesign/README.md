# Handoff: rupu Control Plane — Shell v2

## Overview

A holistic redesign of the `rupu cp serve` web UI (`crates/rupu-cp/web`). It keeps the
product's brand and every existing capability, and changes three things:

1. **Information architecture** — 15 sidebar destinations collapse to 7, and the
   duplicate `Agents / Workflows / Autoflows` entries (which appear today under both
   **Runs** and **Build**) are eliminated by splitting the app along one explicit axis:
   *definitions* (Library) vs *executions* (Activity) vs *subjects* (Projects).
2. **The Overview page leads with work, not numbers** — a **Needs you** queue of the
   actual gates, failures and critical findings, with Approve / Reject inline. Approvals
   go from five clicks to zero. The metric tiles compress into one instrument strip below it.
3. **Visual language** — dark-first "instrument panel": hairline rules and flat panels
   instead of rounded-xl cards with shadows; monospace with uppercase tracked micro-labels
   for every identifier, status and numeral. This promotes the best existing idiom in the
   codebase (the `.sr-*` Situation Room block in `styles.css`) to a system-wide rule.

Every colour, font stack and status/severity semantic already exists in
`web/src/styles.css` and `web/tailwind.config.ts`. **No new palette was invented.**

---

## About the design files

The two files in this bundle are **design references authored in HTML**. They are
prototypes that show intended look and behaviour. They are **not** production code and
must **not** be copied into the app.

The task is to **recreate them inside the existing `crates/rupu-cp/web` environment** —
React 18 + TypeScript + Vite + Tailwind, `react-router-dom` routes lazy-loaded in
`App.tsx`, tokens as CSS variables consumed through Tailwind classes — using that app's
established components and patterns.

| File | What it is |
|---|---|
| `Control Plane — Current.dc.html` | Faithful recreation of **today's** UI (sidebar shell + Dashboard, Projects, Findings). Use as the before/after baseline and to confirm nothing regresses. |
| `Control Plane — Redesign.dc.html` | The **target** design. Six screens: Overview, Activity, Projects, Security, Library, Fleet. Click the left rail to switch. |

Both open directly in a browser. Ignore their internal structure (a streaming-component
format used by the design tool); read them as visual specs.

## Fidelity

**High-fidelity.** Final colours, type, spacing, density and interaction states.
Recreate pixel-for-pixel — but **through the token layer, not the literal hexes**
(see *Design tokens* below).

---

## Rollout: the feature flag

Yes — a single flag is the right mechanism, and it belongs in `~/.rupu/config.toml`
alongside the existing `[ui]` block, which already owns theme and `live_view`:

```toml
[ui.cp]
shell = "v2"        # "v1" (default) | "v2"
```

Rationale for putting it under `[ui.cp]` rather than a build-time env var:

- `[ui]` is already the user-facing surface for UI preferences (`theme`,
  `live_view`), so this is consistent with the documented config vocabulary in
  `docs/configuration.md`.
- It is per-user and hot-switchable — no rebuild, no reinstall, and it can be flipped
  on a remote host you only reach over SSH.
- One flag, one code path to delete when v2 becomes the default.

### Wiring it (verify these against the current source before implementing)

1. **Config struct** — add the field in `crates/rupu-config` next to the existing
   `[ui]` fields, default `"v1"`. Add it to the `docs/configuration.md` reference and to
   `rupu config get/set` completion.
2. **Expose to the web app** — `crates/rupu-cp/src/api/config.rs` already serves config
   to the UI, and `crates/rupu-cp/src/api/host_info.rs` serves host-level facts. Return
   `ui.cp.shell` from whichever of those the UI already fetches on boot; do **not** add a
   second bootstrap request. If neither is fetched eagerly, inject it as a
   `<meta name="rupu-shell" content="v2">` in `crates/rupu-cp/src/embed.rs` when it
   serves `index.html`, which costs zero round-trips.
3. **Switch in the web app** — branch once, at the shell:

   ```tsx
   // web/src/App.tsx
   const Layout = shell === 'v2'
     ? React.lazy(() => import('./components/v2/Shell'))
     : React.lazy(() => import('./components/Layout'));
   ```

   Keep `components/Layout.tsx` untouched. New shell chrome lives in
   `web/src/components/v2/`. Page bodies are shared wherever the redesign did not change
   them; where it did, add a `v2/` sibling rather than editing the v1 page.
4. **Routes** — v2 introduces new paths (below). Register them unconditionally and have
   the old paths `<Navigate replace>` to the new ones when `shell === 'v2'`. Deep links
   in transcripts, findings and the command palette must keep working under both shells.
5. **Tests** — the existing page tests assert v1 markup. Add `v2/` test files; do not
   rewrite the v1 suites while the flag exists.

---

## Information architecture

### Route map

| v2 nav item | v2 route | Replaces (v1) |
|---|---|---|
| Overview | `/overview` | `/dashboard` |
| Activity | `/activity` | `/runs/agents`, `/runs/workflows`, `/runs/autoflows`, `/sessions`, `/events` |
| Projects | `/projects` | `/projects` (redesigned) |
| Security | `/security` (tabs: `findings`, `coverage`, `catalog`) | `/findings`, `/coverage`, `/coverage/templates` |
| Library | `/library` (tabs: `agents`, `workflows`, `autoflows`, `contracts`) | `/agents`, `/workflows`, `/autoflows` |
| Fleet | `/fleet` | `/hosts`, `/workers` |
| Usage | `/usage` | `/usage` (unchanged this pass) |
| Settings (footer) | `/settings` | `/settings` (unchanged this pass) |

Detail routes are **unchanged** and still open from these lists: `/runs/:id`,
`/transcript`, `/sessions/:id`, `/hosts/:id`, `/agents/:name`, `/workflows/:name`,
`/coverage/:target/*`, `/projects/:wsId/*`.

Sidebar **groups, collapsibles and the `SidebarGroup` localStorage state are gone** in v2.
Seven flat leaves plus Settings pinned to the bottom. `web/src/lib/sidebarNav.ts` gets a
v2 export; the v1 export stays.

### The load-bearing rule

**Library shows definitions and never lists their runs. Activity shows executions and
never lets you edit a definition.** A Library row's only links out are *"used by"* and
*"runs 30d"*, both of which navigate to Activity pre-filtered. This asymmetry *is* the
fix for the Runs/Build duplication — if a run list appears in Library the redesign has
failed.

### Command palette

`web/src/components/CommandPalette.tsx` keeps its entity search unchanged. Under v2,
update only the static `NAV_PAGES` array to the seven new destinations, and re-target the
`to` on entity results whose list page moved (runs → `/activity`, findings → `/security`,
workers → `/fleet`).

---

## Design tokens

### Critical instruction

The HTML mocks contain **literal hex values**. Those literals are the *dark-theme
resolutions of tokens that already exist*. Implement with the Tailwind token classes
(`bg-panel`, `text-ink-dim`, `border-border`, `text-status-awaiting`, `bg-sev-critical-bg`,
…) so light theme and any future theme keep working. **Do not hardcode a hex anywhere.**

| Mock literal (dark) | Token | Tailwind |
|---|---|---|
| `#0A0A0A` | `--c-bg` | `bg-bg` |
| `#141416` | `--c-panel` | `bg-panel` |
| `#1B1B1F` | `--c-surface` | `bg-surface` |
| `#232327` | `--c-surface-hover` | `bg-surface-hover` |
| `#26262A` | `--c-border` | `border-border` |
| `#F5F5F5` | `--c-ink` | `text-ink` |
| `#A1A1AA` | `--c-ink-dim` | `text-ink-dim` |
| `#71717A` | `--c-ink-mute` | `text-ink-mute` |
| `#7C3AED` | `--c-brand-500` / `-600` | `bg-brand-500` |
| `#A78BFA` | `--c-brand-700` | `text-brand-700` |
| `#211738` | `--c-brand-50` | `bg-brand-50` |
| `#60A5FA` | `--c-status-running` | `text-status-running` |
| `#4ADE80` | `--c-status-done` | `text-status-done` |
| `#F87171` | `--c-status-failed` | `text-status-failed` |
| `#FBBF24` | `--c-status-awaiting` | `text-status-awaiting` |
| `#22D3EE` | `--c-status-paused` | `text-status-paused` |
| `#A855F7` / `#2A1C38` | `--c-sev-critical` / `-bg` | `text-sev-critical` / `bg-sev-critical-bg` |
| `#F87171` / `#301818` | `--c-sev-high` / `-bg` | … |
| `#FB923C` / `#302012` | `--c-sev-medium` / `-bg` | … |
| `#FACC15` / `#2E2810` | `--c-sev-low` / `-bg` | … |
| `#94A3B8` / `#1E1F23` | `--c-sev-info` / `-bg` | … |

### Two new tokens are required

The design uses two neutrals with no existing token. Add them to both `:root` and
`[data-theme="dark"]` in `styles.css`, and to `tailwind.config.ts`:

| Purpose | Dark | Light (proposed) | Suggested token |
|---|---|---|---|
| Stronger hairline: dashed affordances, `/` separators, secondary-button borders, hover borders | `#3F3F46` | `#CBD5E1` | `--c-border-strong` |
| Pressed/hover fill for the neutral secondary button | `#2E2E33` | `#CBD5E1` | `--c-surface-active` |

The brand tile gradient is `brand-500 → brand-700`, matching `.sr-mark` in `styles.css`.
Note that in dark theme `--c-brand-700` resolves *lighter* (`#A78BFA`), so the mark
gradients up, not down. Use the variables, not the resolved values.

### Type

Stacks are unchanged: `fontFamily.sans` and `fontFamily.mono` from `tailwind.config.ts`.
The change is **which** is used where.

| Role | Family | Size | Weight | Tracking | Case |
|---|---|---|---|---|---|
| Page title | sans | 16px | 600 | -0.01em | — |
| Panel header / micro-label | mono | 9–10px | 700 | 0.12–0.14em | uppercase |
| Column header | mono | 9px | 700 | 0.12em | uppercase |
| Big metric | mono | 26px | 600 | -0.02em | tabular-nums |
| Card metric | mono | 15–19px | 600 | — | tabular-nums |
| Identifier (repo, run id, agent, file, host, CWE, concern) | mono | 10–11.5px | 400–600 | — | — |
| Body / prose | sans | 12–12.5px | 400 | — | — |
| Status pill / kind tag | mono | 9–10px | 600–700 | 0.08–0.1em | lowercase or uppercase |

Every numeral that sits in a column or a metric gets `font-variant-numeric: tabular-nums`.

### Geometry & density

4px base grid. Panel radius **7px** (was `rounded-xl`), inner card 6px, chip/pill 4–5px,
badge pill 999px. Panels are `1px solid border` on `bg-panel` with **no shadow** — the
`shadow-card` token is not used in v2. Table rows 8px vertical padding; controls 24–28px
tall; rail items 30px; top bar and rail header both 48px.

### Accessibility rule (non-negotiable)

`--c-ink-mute` (`#71717A`, ~3.7:1 on panel) is **the dimmest tier allowed to carry
text**. `--c-status-skipped` (`#52525B`) and `--c-border-strong` (`#3F3F46`) are *fill and
line* tokens — they must never be used as a text colour, at any size. An earlier draft of
this design made that mistake across ~57 data-bearing nodes; if a reviewer sees a
timestamp, run id or repo remote rendered dimmer than `ink-mute`, that's the bug.

All motion (`.rg-pulse-run`, `.rg-pulse-await`, `.beacon`/`sr-beacon`) must keep its
`prefers-reduced-motion` guard, as the existing `styles.css` blocks do.

---

## Shell

### Left rail — 204px, `bg-panel`, `border-r`

- **Header, 48px**: 24px brand tile (6px radius, `brand-500 → brand-700` gradient,
  `∞` glyph in mono 14px/300, `box-shadow: 0 0 0 1px brand-500/35`), the `rupu` wordmark
  (sans 13px/600), and `cp` right-aligned (mono 9px, 0.1em, uppercase, `ink-mute`).
  Reuse `components/Brand.tsx` with a new `variant="rail"`; do not fork the mark.
- **Nav, 7 leaves**, 30px tall, 5px radius, 15px lucide icon + sans 13px label.
  - Idle `text-ink-dim`; active `bg-surface` + `text-ink` + `box-shadow: inset 2px 0 0 brand-500`.
  - Right-aligned badge, mono 10px: **Overview** = attention count (amber pill),
    **Activity** = running count with a pulsing dot (`status-running`), **Projects** =
    project count (plain `ink-mute`), **Security** = critical findings (purple pill),
    **Fleet** = unhealthy hosts (`status-failed`). A badge renders only when > 0, and
    renders `—` never `0` when the count is unknown (same `Option` discipline as
    `FleetStrip.tsx`).
- **Settings** pinned to the bottom of the nav via `margin-top: auto`.
- **Footer strip**, `border-t`, 9px/12px padding: a status dot + mono 10px
  `"4 hosts · 1 down"`. The v1 `ThemeToggle` moves to the top bar.

### Top bar — 48px, `bg-panel`, `border-b`

Left → right: a mono 11px `scope` label; a **project scope** selector (26px, `bg-surface`,
5px radius, brand dot + `all projects` + chevron) that filters every page; a `/`
separator; a **range** selector (`7d / 30d / all`, replacing the per-page range control on
Dashboard); a **⌘K search field** (28px, `bg-bg`, flex 1, max-width 420px, magnifier +
placeholder `Jump to run, project, agent, finding…` + a `⌘K` keycap) that opens the
existing `CommandPalette`; then right-aligned, a **live pill** (mono 9px, 0.12em,
uppercase, `status-done` on a 10%-alpha fill with a 30%-alpha ring and a pulsing beacon)
and the **theme toggle** as a 26px icon button.

Scope and range are shell-level state, persisted to `localStorage` (a new key —
do not touch `rupu.sidebar.groups`), and read by every page. The `live` pill reflects the
real SSE connection state and must degrade honestly: `connecting` / `reconnecting` take
the `warn` tone, exactly as `.sr-live[data-state]` already does in `styles.css`.

---

## Screens

### 1. Overview — `/overview`

Replaces `/dashboard`. Body padding 16px, blocks stacked with 16px gaps.

#### 1a. Needs you (the headline block)

A `border` panel. Header row 36px with a left-to-right `brand-500` 10%→0 gradient wash,
a 2px × 14px `brand-500` tick, the title (mono 10px/700, 0.14em, uppercase), a mono 10px
subtitle `"4 items · oldest 1h04m"`, and a right-aligned `mute for 1h` link.

Then one row per item, `border-bottom` between, each with a **2px left border in its
tone**, 11px/14px padding, 12px gap:

| Item type | Left border / tag tone | Tag | Actions |
|---|---|---|---|
| Approval gate | `status-awaiting` | `gate` (pulsing, `rg-pulse-await`) | **Approve** · **Reject** · Diff |
| Failed run | `status-failed` | `failed` | **Transcript** · Retry |
| Critical finding | `sev-critical` | `critical` | **Review code** · Triage |

Row anatomy: the tag (mono 9px/700, 0.1em, uppercase, tinted fill, 4px radius) → a
two-line body: line 1 is the provenance breadcrumb in mono 11px
(`project` bold · `/` · `definition` in `brand-700` · `#runid` in `ink-mute` ·
`step <id>` in `ink-mute`), line 2 is the human sentence in sans 12.5px `text-ink` with
inline identifiers in mono — → a right-aligned age in mono 11px, tabular
(`status-awaiting` when the item is the oldest, else `ink-dim`) → the action buttons,
26px tall, 5px radius: affirmative = `status-done` at 14% fill / 35% ring, destructive =
`status-failed` at 12% / 35%, neutral = `bg-surface` + `border-border`.

**Sort order**: gates first (oldest first), then failures (newest first), then critical
and high findings (newest first). Cap at 6 rows with a `+N more` footer link into the
relevant filtered list. Empty state: collapse the whole panel to a single 36px row reading
`nothing needs you` in `ink-mute` — do not render an empty card.

**Data.** This is an aggregate no single endpoint serves today. Add one — e.g.
`GET /api/attention` in `crates/rupu-cp/src/api/` — and build it on the fan-out that
`api/dashboard.rs` + `host/dashboard_summary.rs` + `api/host_fanout.rs` already implement,
so it inherits their per-host partial-sum and `Option`/`null` discipline: paint from
whatever has resolved, never block on a hung host, never fabricate a `0`.

Approve/Reject must call the same server path as `rupu workflow approve|reject` (including
the `--gate <step-id>` case when several gates are parked on one run — check
`api/runs.rs`). On success, resolve the row optimistically with the `.sr-ev.resolved`
treatment already in `styles.css` (fade to 55% opacity, replace actions with a resolved
tag) rather than removing it instantly.

#### 1b. Instrument strip

One `border` panel, `display:flex`, six equal cells divided by `border-r` hairlines,
11px/16px padding. Each cell: a mono 9px/0.14em uppercase `ink-mute` label → a 26px mono
600 tabular value with a small mono 10.5px qualifier beside it → a 3px segmented bar → a
mono 10px breakdown line.

| Cell | Value | Qualifier | Bar segments | Breakdown |
|---|---|---|---|---|
| Active now | `active.running` | `longest <dur>` | running / awaiting / paused | `4 running · 2 gated · 1 paused` |
| Success rate | % | delta vs previous window | done / remainder | `96 of 112 terminal runs` |
| Failed · 30d | count in `status-failed` | `3 in last 24h` | — (22px sparkline instead) | — |
| Open findings | count + `+` when partial | `partial sum` | critical/high/medium/low | `1C 6H 13M 21L` |
| Spend · 30d | `$1,284` | % delta | input / output / cached | `62.4M tokens · 24% cached` |

Derive from the existing `getDashboard` payload (`ActiveCounts`, `ActiveLongest`,
`TerminalBucket[]`, `findings_open` + `findings_partial`) plus the usage endpoint for
spend. Keep the existing rules: `null` renders `—` not `0`, and a partial sum is marked.
`KeyPointTiles.tsx` is the reference for that logic — port the logic, replace the layout.

#### 1c. Two charts

Side by side, equal columns, 16px gap. Each: a 32px header (mono 9.5px/0.14em uppercase
title + a right-aligned inline legend of 6px swatch + mono 9.5px label) over a 164px plot.

Keep `recharts` and keep the existing components' semantics: **Outcomes** =
`TerminalTrend.tsx` (stacked completed / failed / rejected / cancelled), **Throughput** =
`ThroughputChart.tsx` (stacked manual / cron / event, colours locked to `TriggerChip`).
Retain `isAnimationActive={false}` and the `useThemeColors()` hook. The only changes are
chrome: grid `border`, axis labels mono 9.5px `status-skipped`-adjacent (use `ink-mute`),
`fillOpacity` 0.18–0.28, stroke 1.5, and the legend moves into the panel header so the
plot is unlabelled.

#### 1d. Live stream + Hosts

A `1.6fr / 1fr` grid.

**Live stream** — the compact form of `/events`. 32px header with a pulsing beacon and an
`open activity →` link. Rows: 8px/14px padding, `border-bottom` in `surface`, a **2px left
border in the event's accent**, then a fixed 38px mono 10px relative time → a mono 9px
uppercase tinted kind tag (`tool` brand / `done` green / `start` blue / severity / `cron`
neutral) → the project in mono 11px → a sans 12px single-line clause that truncates with
an ellipsis. Reuse the accent mapping and the `timeline-enter` / `is-fresh` stripe
animations from `styles.css`; this is a denser skin of `situationRoom/EventStream.tsx`,
not a new component.

**Hosts rail** — 32px header with a `fleet →` link, then one 6px-radius `bg-surface` card
per host: an 8px status dot (with a 3px alpha ring when live/faulted), the host name in
mono 11.5px/600, the transport in mono 9px uppercase, a right-aligned freshness value
(`live` in `status-done`, else the age in `ink-dim`, else the failure word in
`status-failed`); a 3px running/queued/free load bar; and a mono 10px
`2 running · 0 queued · 8 slots` line. A faulted host swaps to a `status-failed` border on
an `err-bg` fill and states the cause and its consequence
(`dial-home lost 14m ago · 2 runs orphaned`) instead of the load bar.

Freshness must stay **per host** — `HostFreshnessStrip.tsx`'s reasoning holds: one global
"live" pill would lie about the SSH and Bucket transports.

### 2. Activity — `/activity`

The single execution list. Header: title + a mono 11px subtitle
`every execution — agents, workflows, autoflows, sessions` + a right-aligned **live tail**
switch (26×15px pill, `status-done` when on).

**Filter row.** A segmented control (`bg-surface`, 1px border, 6px radius, 2px pad;
active segment `bg-panel` + `text-brand-700` + a 1px shadow) carrying
`all 412 · agents 186 · workflows 122 · autoflows 68 · sessions 36`; a 1px × 20px
divider; then independent status chips `gated 2 · running 4 · failed 7 · completed 96`
(26px, 5px radius; selected takes its status colour at 12% fill / 35% ring); then a
right-aligned dashed `+ save view` button. Filters are additive and reflected in the URL
query so a filtered list is linkable.

**Table.** Columns: `Status · Kind · Subject · Project · Host · Trigger · Dur · Cost ·
Started`. Header row on `bg-surface` with mono 9px uppercase labels. Rows 8px padding,
`border-bottom` in `surface`; a gated row takes a 4%-alpha `status-awaiting` row tint.

- **Status** — the existing `StatusPill`, restyled: mono 10px/600, 999px radius, colour at
  10% fill with a 30% inset ring, and `statusMotionClass()` from `lib/status.ts` still
  supplying `rg-pulse-run` / `rg-pulse-await`. `gated` is the label for
  `awaiting_approval` in v2.
- **Kind** — mono 10px, colour-coded: workflow `brand-700`, agent `ink-dim`, autoflow
  `sev-medium`, session `status-paused`.
- **Subject** — the one flexible column: the definition name in mono `ink`, then a
  ` · <detail>` clause in mono 11px `ink-mute` (or `status-failed` when it names the
  failure).
- **Dur** — takes the status colour while non-terminal, `ink-dim` when terminal.

**Table contract** (this is the pattern `SortableTable.tsx` §5.1 already describes, and it
must be honoured or the layout collapses): exactly **one** flexible column per table
carrying `width:100%` on its `<th>`; every other column is `width:1%` + `white-space:nowrap`;
the whole table sits in an `overflow-x:auto` wrapper *inside* the panel so the panel keeps
its radius and never grows a vertical scrollbar. Build this on `SortableTable` — extend it,
don't replace it; sorting, `rowHref`, the single-tab-stop row and `renderDetail` all still
apply.

### 3. Projects — `/projects`

The convergence point: Security and Activity state, per subject. Header + a segmented
`active 9 · all 14 · dormant 5`.

Columns: `Project · Branch · Now · Runs 30d · Success · Findings · Coverage · Spend 30d ·
Active`.

- **Project** (flexible) — a 14px lucide provider glyph (`Github` / `Gitlab` /
  `HardDrive` / `Server`, resolved by the existing `lib/projectProvider.ts`), the name in
  sans 600, then the remote in mono 11px `ink-mute`. The v1 30px glyph shrinks to 14px.
- **Branch** — mono 10px chip on `bg-surface`.
- **Now** — live state only, as coloured mono clauses: `2 run` `1 gate` `1 paused`.
- **Success / Findings / Coverage** — each a 44px × 3px micro-bar plus a mono 10.5px
  figure. Findings' bar is the four-segment severity ramp and its figure takes the colour
  of the **worst** severity present (use `severityRank()` from `lib/severity.ts`).
- **Spend / Active** — mono tabular, right-aligned.

A dormant project renders its name in `ink-dim` and every unreported cell as `—`. Keep
`UsageBarChart` on the page only if the scope selector is set to *all projects*; the
redesign drops it from the default view because the per-row Spend column now carries it.

### 4. Security — `/security`

Header: title + a segmented `findings · coverage · catalog` (these were three separate
routes).

**Summary strip** — one panel, three flex cells divided by hairlines: **Concern coverage**
(26px %, a `brand-500` progress bar, `31 of 42 concerns examined`); **Gaps** (26px count in
`status-awaiting`, then the named gaps in mono 10px); **Open findings by severity** —
four stacked figure/label pairs (19px mono 600 in the severity colour over a mono 9px
uppercase `ink-mute` label), with a `flex-shrink:0`, `nowrap` trailing block holding
`−8 vs last run` and `coverage run 31m ago`.

**Findings table.** `Sev · Summary · Location · CWE · Concern · Project · Age`, with a
**2px left border per row in the row's severity colour** — the ramp reads as a vertical
edge before you read a word. `Sev` is an abbreviated mono 9px badge
(`crit / high / med / low / info`) using the existing `sev-*` / `sev-*-bg` pill classes.
`Summary` is the flexible column. `Location` keeps `FindingsTable.tsx`'s deep link into
`/projects/:wsId/code?path=…&line=…`. `CWE` keeps its outbound link. Row expansion to
`FindingEvidence` is unchanged.

### 5. Library — `/library`

Header: title + a mono 11px subtitle `definitions, not executions — the files in .rupu/` +
a segmented `agents 12 · workflows 9 · autoflows 6 · contracts 4` + a primary
**New agent** button (which opens the existing `AgentBuilder` / `NewAgentModal`).

Agents tab columns: `Agent · Model · Permission · Tools · Used by · Scope · Runs 30d ·
Last run`.

- **Agent** (flexible) — name in mono 600, then ` — <one-line purpose>` in sans 11.5px
  `ink-mute`, pulled from the agent's description.
- **Model** — a 6px provider swatch (anthropic `brand-500`, openai `status-done`, gemini
  `status-running`, copilot `ink-dim`) + the model id in mono 10.5px. Reuse
  `pages/ProviderIcon.tsx`'s provider resolution.
- **Permission** — a mono 9.5px uppercase badge, and the tone is a safety signal:
  `read-only` `status-done` · `ask` `status-awaiting` · `bypass` `status-failed`. Given the
  README's "agents are code" warning, `bypass` must always read loud.
- **Used by** — `3 workflows` / `2 autoflows` in `brand-700`, linking to Activity filtered
  to those runs. **Not** an inline run list.
- **Scope** — `global` vs the owning project, distinguishing `~/.rupu/` from
  `<project>/.rupu/`.
- **Last run** — a status word in its status colour (`running`, `failed 8m`) or a relative
  time, or `never`.

Workflows / autoflows / contracts tabs follow the same shape with their own columns
(workflows: steps, trigger, gates, last run; autoflows: trigger source, claims, enabled).

### 6. Fleet — `/fleet`

Merges `/hosts` and `/workers`. Header: title + `4 hosts · 6 workers · 1 fault` + a
primary **Add host** button.

**Host cards** — `repeat(auto-fill, minmax(268px, 1fr))`, 12px gap. Each: an 8px status
dot (`pulse-run` while executing), the name in mono 13px/600, a bordered mono 9px uppercase
transport tag, a right-aligned freshness value; a 4px running/queued/free load bar; a
three-up `running / queued / slots` block (mono 9px uppercase labels over 15px mono 600
figures, coloured `status-running` / `status-awaiting` / `ink-dim`); and a `border-t`
footer in mono 10px carrying `3 projects · anthropic, openai · rupu 0.71.2`.

A faulted card takes a `status-failed` border plus `box-shadow: inset 3px 0 0
status-failed`, replaces the metric block with an `err-bg` panel stating cause and
consequence, and offers **Reconnect** and **Logs**. Unreported counts are `—`.

**Workers table** below: `State · Worker · Host · Current · Uptime`, with `State` as a dot
+ mono 10px word (`busy` running / `idle` mute / `lost` failed) and `Current` linking to
the run it is executing.

---

## Interactions & behaviour

- **Navigation** — rail click routes; badges update from live data. Active item = surface
  fill + 2px inset brand rule.
- **⌘K / Ctrl-K** — unchanged `CommandPalette` behaviour and hotkey (`lib/useHotkey.ts`).
- **Scope & range** — shell-level, applied to every page, persisted to `localStorage`,
  reflected in the URL where a page is linkable.
- **Approve / Reject** — inline on Overview and on a run's detail. Optimistic resolve with
  the `.sr-ev.resolved` fade; roll back and surface an `ErrorBanner` on failure.
- **Live tail** (Activity) — when on, new rows enter with `timeline-enter` + the
  `is-fresh` brand stripe (both already in `styles.css`) and the list holds scroll position
  unless the user is at the top.
- **Hover** — table rows `bg-bg/60` (as v1); rail items `bg-surface-hover`; neutral buttons
  raise their border to `border-strong`; cards raise their border only, never lift.
- **Loading** — panels paint their chrome immediately and fill per block as data resolves;
  never blank the page on one slow host. Reuse `Spinner` / `Skeleton`.
- **Errors** — a stale-data refresh failure shows a quiet inline note and keeps the last
  good data (v1 Dashboard already does this); a hard failure shows `ErrorBanner`.
- **Empty** — reuse `EmptyState`; the Needs-you panel collapses to one row rather than
  showing an empty card.
- **Responsive** — designed for ≥1280px. Below ~1150px tables scroll horizontally inside
  their panel; the instrument strip wraps to two rows; the Overview bottom grid stacks.
  The rail does not currently collapse — a future icon-only mode is the natural next step.
- **Reduced motion** — every animation guarded, as in `styles.css` today.

## State

Shell: `shell` flag (from config, immutable), `scope` (project id | all), `range`
(`7d`/`30d`/`all`), SSE connection state, theme (existing `ThemeProvider`).
Overview: attention items + optimistic resolution set, dashboard payload, per-host
freshness map, live event buffer.
Activity: kind filter, status filter set, live-tail on/off, saved views.
Security: active tab. Library: active tab. Projects: cohort filter. Per-table sort state
stays inside `SortableTable`.

## Assets

No new assets. Icons are `lucide-react` (already a dependency) at 14–15px, stroke 2:
`LayoutDashboard`, `Activity`, `FolderGit2`, `ShieldCheck`, `BookMarked`, `Server`,
`DollarSign`, `Settings`, `Search`, `Moon`, `ChevronDown`, plus `Github` / `Gitlab` /
`HardDrive`. The `∞` brand glyph stays a monospace character, as in `Brand.tsx`.

## Suggested sequence

1. Config flag + plumbing, with v2 rendering a stub shell. Verify v1 is untouched.
2. Shell: rail, top bar, scope/range state, route map + redirects, palette nav update.
3. Token additions (`border-strong`, `surface-active`) in both themes.
4. `SortableTable` v2 skin + the one-flexible-column contract; then Activity, whose table
   is the widest and will shake out the contract.
5. Overview: instrument strip → charts → live stream + hosts rail → the `/api/attention`
   endpoint and the Needs-you block last (it is the only piece needing new server work).
6. Security, then Fleet, then Projects, then Library.
7. Light-theme pass across all six screens — the design was authored dark-first, so this
   is where any remaining hardcoded hex will surface.

## Files in this bundle

- `Control Plane — Redesign.dc.html` — target design, six screens
- `Control Plane — Current.dc.html` — recreation of today's UI, for comparison
- `screenshots/` — the six screens as rendered, for quick reference:

| File | Screen |
|---|---|
| `01-overview-top.png` | Overview — Needs-you queue + instrument strip |
| `02-overview-charts-and-stream.png` | Overview — charts, live stream, hosts rail |
| `03-activity.png` | Activity — unified execution list |
| `04-projects.png` | Projects — with per-row success / findings / coverage bars |
| `05-security.png` | Security — findings tab + coverage summary strip |
| `06-library.png` | Library — agents tab |
| `07-fleet.png` | Fleet — host cards incl. the faulted card |
| `08-fleet-workers.png` | Fleet — workers table |

Screenshots were captured at a ~924px-wide viewport, which is **narrower than the design
target** (≥1280px): the tables show their horizontal-scroll state and the instrument strip
is tighter than intended. Open the HTML at full width for the real proportions.

## Open questions for the team

1. Should `/events` survive as a standalone fullscreen "Situation Room" wall display
   separate from Activity? The design folds it into Activity and the Overview stream, but
   a wall-mounted view is a genuinely different use case.
2. Does `Usage` stay a separate destination, or become a tab of Projects once every
   project row carries spend?
3. Is `contracts` the right fourth Library tab, or do contracts belong in Settings?
4. Should the Needs-you queue be per-scope (respecting the project selector) or always
   fleet-wide? The design shows fleet-wide.
