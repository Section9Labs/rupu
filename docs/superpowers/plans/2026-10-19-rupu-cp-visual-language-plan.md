# rupu CP visual language — implementation plan

**Spec:** `docs/superpowers/specs/2026-10-19-rupu-cp-visual-language-design.md`
**Visual contract (mocks):** `docs/superpowers/specs/assets/cp-visual-language/{visual-language,dashboard-redesign,charts-v2}.html`
**Scope of THIS plan/PR:** the shared components + the Dashboard rebuilt on them. The rest of the app's screens are a follow-up.

Pure frontend. CP Tailwind tokens only — no new raw hex, static class strings, no `any`. `npx vitest run`, `npx tsc -b --noEmit`, `npm run build` all green; verified in a real browser (light + dark) against the running CP.

## Grounding (what already exists — build on it, don't fork)

- **Tokens** (`src/styles.css` `:root` light / `[data-theme="dark"]` dark, surfaced in `tailwind.config.ts`): `bg/panel/surface/surface-hover/border/border-strong`, `ink/ink-dim/ink-mute`, `brand-{50..700}`, `status-{running,completed,done,failed,awaiting,paused,pending,skipped,cancelled,rejected}`, `sev-{critical,high,medium,low,info}(-bg)`, `ok/err/warn/info(-bg)`. The mock's `--ok→status-completed`, `--fail→status-failed`, `--run→status-running`, `--warn→status-awaiting`, `--brand→brand-500`.
- **Primitives** (`src/components/ui/`): `Button` (variants primary/secondary/ghost/danger/…), `Chip` (generic ring pill), `Skeleton`, `EmptyState`, `Dot`, `Spinner`, `Badge`, `Segmented`, `Input`. `StatusPill` already renders the exact tint+text+ring+dot treatment the spec's chip wants.
- **status map** `src/lib/status.ts` (`STATUS[key].pillClass/dotClass/label`), **severity** `src/lib/severity.ts`, **theme colors for inline SVG** `src/lib/useThemeColors.ts` (`get('status.completed')` → `rgb(...)`), **duration** `src/lib/duration.ts`.
- **Dashboard data**: `src/pages/Dashboard.tsx` + `src/lib/dashboard/useDashboardData.ts` (per-host fan-out → merged `DashboardSummary`: `active{running,awaiting_approval,paused,pending}`, `active_longest`, `terminal_buckets[]`, `throughput_buckets[]`, `cycles`, `findings_open`, `fleet`, `+ findings_partial/cycles_partial/fleet_partial`). Freshness strip, customer scope, partial-sum rules already handled.
- **Needs-you data sources** (existing endpoints): `api.getWorkflowRuns({host, q, limit})` (query supports `status:awaiting_approval`, `status:failed`), `api.getRegisteredHosts()`. Mutations: `api.approveRun(id, mode?, host?, gate?)`, `api.rejectRun(id, reason, host?, gate?)`, `api.resumeRun(id, host?)`.
- **Dev**: `npm run dev` → :5173, Vite proxies `/api` → `http://127.0.0.1:7878`. Reduced motion via `matchMedia('(prefers-reduced-motion: reduce)')`; CP already has keyframes + reduced-motion guards in `styles.css`.

## Design decisions (recorded)

1. **Button hierarchy.** The spec's primary is a *neutral high-contrast solid* (`bg-ink text-bg`), not brand purple. The app-wide `primary`=brand is the anti-pattern the spec names. To stay Dashboard-first and not restyle every screen in this PR, add a **`solid`** Button variant (`bg-ink text-bg hover:bg-ink/90 dark:…`) = the spec's primary; keep `primary`(brand) untouched for unmigrated screens. New components use `solid`. App-wide swap is the follow-up. (Theme-correct: light = near-black button/white text; dark = near-white button/dark text.)
2. **Chip.** Build ONE `Chip` (new, well-typed) with a `tone` covering status / severity / origin / neutral and `dot` only for live status — the spec's single pill. Reuse `STATUS`/`severity` token classes inside it. Leave `StatusPill` in place; migrate callers later.
3. **Living Chart.** Build `Chart` = the stacked-area **living tile** engine, a faithful React port of charts-v2.html's CHART 1/CHART 2: thin stacked areas that rise from the baseline on mount (`riser` scaleY), recessive gridlines + real baseline, round y-ticks above the max, rounded data-ends, a direct-labelled peak, a breathing now-pulse on the latest point, an interruptible hover crosshair + tooltip, and a `prefers-reduced-motion` fallback (no draw-in, no pulse, static). Colors are tokens (`status-completed`/`status-failed` for outcomes; trigger palette for throughput). Pure-SVG, no recharts. The two-lane hero "seismograph" is NOT in the Dashboard contract (`dashboard-redesign.html` uses two tiles) — the engine is built lane-capable but the hero is a later showcase.
   - **Trigger colors**: follow charts-v2 (manual=info/blue, cron=warn/amber, event=brand/purple) — the approved, colorblind-validated set. (Note: differs from today's `ThroughputChart`/`TriggerChip`; those migrate in the follow-up. Within this PR the living chart + its legend are self-consistent.)
4. **Chart math is a pure, tested module** (`src/lib/chart/area.ts`): data → scaled points, stacked area paths, nice y-max/ticks, peak index. Keeps the SVG component thin and the numbers unit-tested.
5. **Needs-you** is a self-contained hook `useNeedsYou(customer)` mirroring `useDashboardData`'s per-host fan-out: per registered host, fetch `getWorkflowRuns({host, q:'status:awaiting_approval'})` + `getWorkflowRuns({host, q:'status:failed', limit})`, merge, sort (awaiting by oldest wait, failed by most-recent), cap. Inline actions call the existing mutations with optimistic `pending` state (idle→pending→removed on refetch / failed on error), never trusting the POST alone.

## Tasks

Each task: write/adjust tests, implement, `npx vitest run <file>` green, keep `tsc` clean.

### Phase A — shared components (`src/components/ui/`)
1. **Button `solid` variant** — add variant + test; neutral high-contrast, theme-correct, focus ring preserved.
2. **Chip** — `Chip.tsx` (new unified): `tone` (status keys | `high`/`medium`/`low` | `origin-global`/`origin-project` | `neutral`) + `dot?`; token classes only; test each tone + dot presence.
3. **PageHeader** — `PageHeader.tsx`: optional `back` (label+to), `title` (one size ~`text-lg`), optional one-line `subtitle`, right `actions` zone. Test: renders back link, title, subtitle, actions; no subtitle → no empty node.
4. **KpiStrip** — `KpiStrip.tsx`: one divided panel, `items[]` = `{label, value, meta?, attention?}`; attention value in `status-failed`; tabular-nums; no sparkline. Test: renders items, attention class, meta.
5. **SummaryStrip** — `SummaryStrip.tsx`: key/value cells, hairline dividers, tabular-nums, `sub` (quiet second value). Test: renders kv, sub, em-dash for null.
6. **Field** — `Field.tsx`: label + inline origin `Chip` + optional policy-lock note + help line + control (children). Test: label/help/chip/children render; locked note.
7. **Skeleton-in-layout helpers** — small `SkeletonRow`/list skeleton used by Dashboard blocks (reuse `ui/Skeleton`). (Lightweight; may inline in Dashboard.)

### Phase B — living Chart
8. **`src/lib/chart/area.ts`** (pure) + test: `niceMax`, `scalePoints`, `stackedAreaPaths`, `linePath`, `peakIndex`, `roundedEnd`. Snapshot-ish numeric tests.
9. **`src/components/ui/Chart.tsx`** (+ `Chart.test.tsx`): consumes `area.ts`; renders grid/baseline/ticks, stacked areas + lines, peak label, now-pulse, crosshair group + tooltip, hit rect with pointer handlers; `riser`/pulse/crosshair motion via CSS classes in `styles.css` guarded by reduced-motion. Test: renders N series paths, y-ticks, peak label, tooltip content on pointer move (jsdom: assert structure + that handlers update tooltip), reduced-motion branch.
10. **`styles.css`** additions: `.cp-chart-riser`, `.cp-chart-now`, `.cp-chart-cross`, keyframes + `@media (prefers-reduced-motion: reduce)` guard. Token vars only.

### Phase C — Needs-you
11. **`src/lib/dashboard/useNeedsYou.ts`** (+ test): per-host fan-out, merge/sort/cap, `pending`-action state + `approve/reject/resume` wrappers. Test with a fake `api`.
12. **`src/components/dashboard/NeedsYou.tsx`** (+ test): the hero list — `Chip` status + mono codename + meta + inline action buttons (`solid` Approve/Resume, `secondary` Reject/Transcript). Rows link to run detail. Pending/disabled states. Empty → `EmptyState` keeping the frame ("Nothing needs you").

### Phase D — Dashboard rebuild
13. **`src/components/dashboard/FleetHostCards.tsx`** (+ test): the host cards grid from `useDashboardData`'s `hosts` (status `Chip`, mono name, state line, freshness age). Replaces the dotted `HostFreshnessStrip` line on the dashboard (keep the component for other callers).
14. **`src/pages/Dashboard.tsx`** rebuild per `dashboard-redesign.html`: `PageHeader` (title "Overview", subtitle scope+freshness, range `Segmented` + "Usage →" secondary) → **Needs you** hero → **Last 30 days** `KpiStrip` → **Fleet** host cards → **Trends** two living `Chart`s → **Inventory** `SummaryStrip`. Skeletons in the real layout; keep all existing data/scope/partial wiring (`useDashboardData`, `ScopeChip`, banners). Update `Dashboard.test.tsx`.

### Phase E — verify + ship
15. `npx tsc -b --noEmit`, `npx vitest run`, `npm run build` — all green.
16. **Browser**: `npm run dev`, open :5173 against the running CP; check light + dark, the living charts' motion + crosshair + reduced-motion, needs-you actions, phone width. Screenshot for matt. (matt reviews visuals himself before PR.)
17. Code-review pass on the branch, then PR against main.

## Non-goals (this PR)
- Migrating other screens / the global `primary`→`solid` swap / `StatusPill`/`TriggerChip`/`ThroughputChart` retirement — follow-up.
- The hero seismograph on the dashboard (two tiles are the dashboard contract).
- Any backend/API/data-model change.
