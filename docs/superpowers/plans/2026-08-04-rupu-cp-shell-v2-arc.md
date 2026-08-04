# rupu CP Shell v2 — Arc Overview

**Spec:** `docs/redesign/README.md` (the design handoff; HTML mocks + screenshots in `docs/redesign/`).
This document is the arc-level map: locked decisions, the plan sequence, and the load-bearing
research facts each plan's author needs so they don't re-derive them.

## Locked decisions (matt, 2026-08-04)

1. **`/events` survives** as the standalone fullscreen Situation Room wall display under both
   shells. Activity absorbs the day-to-day list; the Overview live stream is the compact skin.
2. **Usage stays a separate rail destination** (`/usage`, unchanged this arc).
3. **`contracts` is the fourth Library tab** (agents · workflows · autoflows · contracts).
4. **The Overview Needs-you queue is always fleet-wide** — it ignores the top-bar project scope
   selector. A parked gate must never be hidden by a filter.

## Rollout mechanism

One flag: `[ui.cp] shell = "v1" | "v2"` in `~/.rupu/config.toml` (default `v1`). The server
injects `<meta name="rupu-shell" content="…">` into `index.html` at serve time (re-resolved from
disk per request, like `GET /api/config`, so `rupu config set ui.cp.shell v2` + browser refresh
switches shells without restarting `cp serve`). The web app branches once, at the shell:
`components/Layout.tsx` (v1) vs `components/v2/Shell.tsx`. v2 routes are registered
unconditionally; old paths `<Navigate replace>` to new ones only when `shell === 'v2'`.

> ⚠️ Compat note for docs: setting `[ui.cp]` in a config read by a **pre-arc rupu binary** fails
> `deny_unknown_fields` deserialization, and several call sites `.unwrap_or_default()` — the whole
> config silently vanishes on the old binary. The flag is opt-in, so only users who set it are
> exposed; `docs/configuration.md` should state the minimum version.

## Plan sequence (one PR each, stacked as needed)

| Plan | Scope | Status |
|---|---|---|
| **1 — Foundation & shell** (`2026-08-04-rupu-cp-shell-v2-plan-1-foundation.md`) | Config flag + `embed.rs` meta injection; `border-strong`/`surface-active` tokens; `lib/shell.ts`; `sidebarNavV2`; `Brand` rail variant; shell state (scope/range, localStorage); v2 `Shell.tsx` (rail, top bar, live pill, footer); `App.tsx` branch + v2 routes + redirects; interim composite pages (Activity/Security/Library/Fleet as tabbed wrappers over the existing page bodies); CommandPalette v2 nav + open-event. Deliverable: flip the flag → complete, capability-equal app under the new 7-leaf IA. | planned |
| **2 — Table contract & Activity** | `SortableTable` v2 skin (mono 9px headers, `overflow-x:auto` wrapper inside panel, `width:100%` subject `<th>`); the unified Activity execution list (segmented kind filter, additive status chips, URL-reflected filters, live tail, `gated` label); **server**: `workspace_id`/project on run list rows (`#[serde(default)]`, see flatten contract below); fix `TriggerChip` violet/sky theming (blocking: those colours are locked to the Throughput chart). | not written |
| **3 — Overview** | Instrument strip (port `KeyPointTiles` logic, new layout); chart chrome reskin (`TerminalTrend`/`ThroughputChart`, legend into header); Live stream (denser `EventStream` skin) + Hosts rail; **server**: `GET /api/attention` on the dashboard fan-out; Needs-you block with inline Approve/Reject + optimistic `.sr-ev.resolved` fade; rail badges (attention/running/projects/critical/unhealthy) fed from the same data. | not written |
| **4 — Security & Fleet** | `/security` tabs (findings/coverage/catalog) with summary strip + severity-edged findings table; `/fleet` host cards + workers table, faulted-card treatment. | not written |
| **5 — Projects & Library** | Projects table with micro-bars (success/findings/coverage) + dormant cohort; Library tabs incl. **contracts**, permission-badge safety tones, "used by → Activity" links (never inline run lists). | not written |
| **6 — Light theme & polish** | Light-theme pass across all six screens; a11y audit (`ink-mute` is the dimmest text tier — `status-skipped`/`border-strong` are fill/line only); reduced-motion audit; visual QA against the mocks; fix remaining non-token colours (`SectionHeader` purple, `lib/status.ts` light-only `hex`/`tint` where reachable from v2). | not written |

**The load-bearing IA rule** (from the spec, §"The load-bearing rule"): *Library shows definitions
and never lists their runs. Activity shows executions and never edits a definition.* A Library
row's only links out are "used by" and "runs 30d", both navigating to Activity pre-filtered. If a
run list appears in Library, the redesign has failed.

## Research facts (verified against source, 2026-08-04)

### Server (`crates/rupu-cp`, `crates/rupu-config`)

- `AppState.config: Arc<RwLock<rupu_config::Config>>` (`src/state.rs:18`); `GET /api/config`
  **re-resolves from disk per request** (`api/config.rs:79`) — precedent for per-request resolve
  in `embed.rs`.
- `UiConfig` is `crates/rupu-config/src/config.rs:75` — `#[serde(default, deny_unknown_fields)]`,
  every leaf `Option<String>`, consumer applies defaults. Nested `[ui.syntax]`/`[ui.palette]` are
  sibling structs in the same file — the pattern for `[ui.cp]`. Tests module at `config.rs:243`.
- `rupu config get/set` is naive `split('.')` over `toml::Value` (`rupu-cli/src/cmd/config.rs:77,90`)
  with **no key allowlist and no completion list** — `ui.cp.shell` needs no completer work.
- `embed.rs` (34 lines) serves `web/dist/` bytes verbatim via `RustEmbed`; **no injection mechanism
  exists** — both the exact-match arm and the SPA-fallback arm must inject; must tolerate the
  `build.rs` placeholder `index.html` (has `<head>`, no `#root`). It's the router **fallback**
  (`server.rs:93`), outside the bearer-token layer. Existing e2e tests: `crates/rupu-cp/tests/embed.rs`.
- Route registration: each `api/*.rs` module owns full `/api/...` paths; new module = `pub mod` in
  `api/mod.rs` + `.merge(...)` in `server.rs:49-76`.
- **Dashboard flatten contract** (`api/dashboard.rs:61-66`): `DashboardResponse` flattens
  `DashboardSummary`; `HttpHostConnector` parses the same body as a bare `DashboardSummary`. New
  top-level keys must not collide with summary field names; new summary fields need
  `#[serde(default)]` or older remotes drop off the dashboard.
- **Option discipline** (`api/dashboard.rs:222` `merge_dashboard_summaries`): sum only `Some`,
  latch `*_partial` on `None`, never fabricate 0; `cycles.clean` has poison semantics;
  `captured_at` = oldest across reporting hosts. `/api/attention` must copy this verbatim; reuse
  `host_fanout::fan_out_via`.
- Approve/reject: `POST /api/runs/:id/approve|reject[?host=…][&gate=<step_id>]`
  (`api/runs.rs:134,186`). Approve body optional; **reject requires a JSON body** (`{reason}`).
  409 on `NotAwaiting|Expired|AmbiguousGate|GateNotFound` (`AmbiguousGate` embeds candidates).
  **`gate` is not threaded through remote-host connectors** — per-gate approval is local-only today.
- `RunListRow` (`api/runs.rs:479`) has **no `kind` and no project/workspace field** — kind is
  implied by endpoint; project needs new server work for the Activity/Needs-you tables. The doc
  comment above it warns: `rupu-cli run list` and SSH `list_runs` emit these rows verbatim — a
  field omission once blanked the whole app.
- Workers list is **local-only, no fan-out** (`api/workers.rs`). `GET /api/hosts` probes (slow);
  `GET /api/hosts/registered` is the probe-free read.
- `GET /api/host/info` exists but is machine-to-machine; **nothing in the browser reads it**.

### Web app (`crates/rupu-cp/web`)

- **No boot-time fetch of anything** — `main.tsx` is `StrictMode > ThemeProvider > App`; the meta
  tag is the zero-round-trip flag carrier. Vite dev serves raw `index.html` (no meta) — dev
  override needed in `lib/shell.ts`.
- Tokens are space-separated RGB channels in `styles.css` `:root` (light) /
  `[data-theme="dark"]`; Tailwind maps them with `<alpha-value>` (`tailwind.config.ts`). Dark
  values match the mock literals exactly (e.g. `--c-bg` dark = `10 10 10` = `#0A0A0A`). Custom
  font sizes `meta/note/ui/lead` are registered in `lib/cn.ts`'s `extendTailwindMerge` —
  **new font-size tokens must be added there too** or twMerge misclassifies them as text colours.
- `.sr-live[data-state]`, `.sr-mark`, `.sr-beacon` in `styles.css:287-302` are **dead CSS with
  zero TSX consumers** — free for the v2 top-bar live pill and rail brand tile. There is **no bare
  `.beacon` class** (spec says "`.beacon`/`sr-beacon`"; only `sr-beacon` exists).
- Connection state pattern: `ConnectionState = 'connecting' | 'live' | 'reconnecting'` lives in
  `components/RunEventFeed.tsx:16`; `pages/Events.tsx:84` shows the subscribe/flip pattern
  ('live' only on first frame, 'reconnecting' on error). SSE helpers in `lib/api.ts`:
  `subscribeEvents` (2437), `subscribeRunLog` (2388), `subscribeTranscript` (2570) — no backoff,
  browser-native retry.
- `SortableTable` (`components/lists/SortableTable.tsx`): §5.1 contract partially implemented —
  `fit` = `w-[1%] whitespace-nowrap`, `subject` = `max-w-0` + truncate + title, `interactive`,
  single-tab-stop rows, `renderDetail`. **Missing:** `overflow-x:auto` wrapper (shell is
  `overflow-hidden`), `width:100%` on the subject `<th>`. Root is `rounded-xl shadow-card` — the
  v2 skin changes both.
- `Badge` `violet`/`sky` tones (used by `TriggerChip` cron/event) are **stock Tailwind colours
  that don't theme in dark mode**; `SectionHeader` `critical` hardcodes `bg-purple-500`;
  `lib/status.ts` `hex`/`tint` fields are light-only literals (affects charts/xyflow inline use).
- `lib/status.ts` is the single status source (`STATUS` map, `statusMotionClass()` →
  `rg-pulse-run`/`rg-pulse-await`, reduced-motion-guarded). `lib/severity.ts` `severityRank()`:
  higher = more severe (critical 4 … info 0). `lib/projectProvider.ts` resolves
  github/gitlab/remote/local from the remote URL.
- `SidebarGroup` owns `localStorage['rupu.sidebar.groups']` — v2 must not touch it.
  Theme key: `rupu.cp.theme` (also read by the `index.html` no-flash script).
- `CommandPalette` is fully self-contained (own ⌘K via `lib/useHotkey.ts`, fetch-on-open of ten
  endpoints); `NAV_PAGES` is a static array (missing `/usage` — pre-existing bug); entity `to`
  mappers live in `lib/paletteSources.ts` (`findingItems` → constant `/findings`,
  `workerItems` → constant `/workers` — the two needing v2 re-targeting).
- Dashboard data layer: `lib/dashboard/useDashboardData.ts` — per-host independent fetch (no
  `Promise.all`), SSE as invalidation signal only, stale-on-error keeps last good data,
  `MergedDashboard` carries `findings_partial`/`cycles_partial`/`fleet_partial`. `KeyPointTiles`
  is the reference for null→`—` rendering. `HostFreshnessStrip`: per-host freshness with 1s tick,
  `live` under 5s. Spend comes from `getUsage(presetWindow(range))` (`lib/usage.ts` types;
  `cost_usd: null` = unpriced, `priced: false` = partial).
- Run detail multi-gate: `RunRecord.awaiting?: AwaitingGate[]` (`{step_id, prompt, since,
  expires_at}`) with back-compat singular fields mirroring the first gate.
- `App.tsx`: `Layout` is a static import; every page individually lazy + Suspense-wrapped;
  **no 404 catch-all**. Route order matters: `/runs/agents|workflows|autoflows` before `/runs/:id`;
  `/agents/new` before `/agents/:name`.
- ui primitives (`components/ui/`, no barrel): Badge, Button (8 variants), Chip, EmptyState,
  ErrorBanner, FilterBar, FilterPills, HostStatusBadge, Input, SearchInput, Segmented, Select,
  Skeleton, Spinner. `TabBar`/`TabButton` at `components/TabBar.tsx`.

### Non-negotiables from the spec

- **No hardcoded hexes** — implement through the token layer; the two new tokens are
  `--c-border-strong` (dark `#3F3F46`, light `#CBD5E1`) and `--c-surface-active` (dark `#2E2E33`,
  light `#CBD5E1`), both **fill/line only, never text**.
- **`ink-mute` is the dimmest text tier.** An earlier design draft broke this ~57 times; any
  timestamp/run-id/repo dimmer than `ink-mute` is the bug.
- Every animation keeps its `prefers-reduced-motion` guard (existing guard inventory:
  `styles.css` lines 181, 239, 397, 567, 652, 845, 965, 1095, 1146, 1282).
- Geometry: 4px grid, panel radius 7px, no `shadow-card` in v2, rows 8px vertical padding,
  controls 24-28px, rail items 30px, top bar/rail header 48px.
- `null` renders `—`, never `0`; partial sums are marked.

### Pre-existing bugs observed (fix in-arc where a plan touches them, else spawn follow-ups)

- `NAV_PAGES` missing `/usage` (v1 palette can't reach Usage).
- `autoflowItems` builds `/workflows/${slug}` without `encodeURIComponent`.
- No 404 route — unknown paths render an empty `<main>`.
- `TriggerChip`/`Badge` violet+sky and `SectionHeader` purple don't theme (Plan 2 / Plan 6).
- `components/dashboard/TriageRibbon.tsx` is dead code (no importer).
