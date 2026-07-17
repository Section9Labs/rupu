# Dashboard Redesign — Plan 1: Ops Page

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-frame `/dashboard` from spend-forward to operations-first: a live swimlane hero, split run status, a cycle-grouped activity feed, a weighted attention row, and a per-host freshness strip — live via SSE invalidation where liveness actually exists, and visibly honest where it does not.

**Architecture:** The server stays the single source of truth for every number. SSE is an *invalidation signal*, not a data channel: event arrival marks a slice dirty and triggers a refetch of the server-computed aggregate, so the Rust aggregation is never mirrored in TypeScript. All grouping/layout logic lives in pure, unit-tested functions separate from the components.

**Tech Stack:** React 18.3 + TypeScript, Vite 5.4, Tailwind 3.4, Recharts 3.8 (already a dep), Vitest. **No new dependencies.**

**Spec:** `docs/superpowers/specs/2026-07-16-rupu-cp-dashboard-redesign-design.md`
**Depends on:** Plan 2 (`docs/superpowers/plans/2026-07-16-rupu-cp-dashboard-plan-2-data-foundation.md`) — specifically the `GET /api/dashboard?range=&host=` contract.

## Global Constraints

- **No new frontend dependencies.** `recharts`, `@xyflow/react`, `@dagrejs/dagre`, `lucide-react`, `clsx`, `tailwind-merge` are already present. The swimlane is hand-rolled SVG — Recharts has no Gantt.
- **All chart/inline colors come from `useThemeColors()`** (`src/lib/useThemeColors.ts`). It exposes `status.running`, `status.awaiting`, `status.paused`, `status.pending`, `status.completed`, `status.failed`, `status.cancelled`, `status.rejected`, plus `panel` / `border` / `ink` / `inkDim` / `inkMute` and `alpha(key, a)`. **Never hardcode a hex or rgb literal** — inline consumers must go through the hook so the palette stays single-sourced in `src/styles.css`.
- **`Unsupported` / offline hosts never render as `0`.** A host that cannot report is not a host with no runs.
- **Bars do not animate.** They redraw on data. Liveness is per-transport; a smoothly-animating local bar beside an SSH bar that jumps every 10s reads as broken.
- **`make cp-web` before `make release`** — the SPA is embedded from `web/dist/` via RustEmbed. Never chain `make release` after git ops in one command.
- **Runtime validation before merge.** Per CLAUDE.md, `npm run build` + passing tests ≠ rendering cleanliness. The page is opened in a browser and checked before the PR merges.
- Dev loop: `npm run dev` in `crates/rupu-cp/web/` (Vite on :5173, proxies `/api` → 127.0.0.1:7878). Run `rupu cp serve` alongside it.

---

### Task 1: API types + client for the new dashboard contract

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`
- Test: `crates/rupu-cp/web/src/lib/api.test.ts` — **already exists** (added on main); ADD to it, do not create or overwrite it

**Interfaces:**
- Consumes: Plan 2 Task 7's `GET /api/dashboard?range=&host=` response
- Produces: `DashboardResponse`, `HostFreshness`, `ActiveCounts`, `TerminalBucket`, `ActiveRunBar`, `CycleRollup`, `RecentRun`, `DashboardRange`, `api.getDashboard(range)`, `api.subscribeEvents(...)` (existing) — **every later task in this plan depends on these names.**

- [ ] **Step 1: Write the failing test**

**`crates/rupu-cp/web/src/lib/api.test.ts` already exists on main — append this test to it. Do NOT overwrite the file.**

```ts
import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('getDashboard', () => {
  it('passes the range through as a query param', async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        hosts: [],
        active: { running: 0, awaiting_approval: 0, paused: 0, pending: 0 },
        terminal_buckets: [],
        active_runs: [],
        cycles: [],
        recent_manual: [],
        findings_open: 0,
      }),
    });
    vi.stubGlobal('fetch', fetchMock);

    await api.getDashboard('7d');

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('range=7d'),
      expect.anything(),
    );
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/api.test.ts`
Expected: FAIL — `getDashboard` takes no arguments, so `range=7d` never appears in the URL.

- [ ] **Step 3: Replace the types and client method**

In `crates/rupu-cp/web/src/lib/api.ts`, replace the existing `DashboardResponse` type (and its `RecentRun` / `RunsSummary` helpers) with:

```ts
/** The dashboard's time window. Mirrors the segmented control. */
export type DashboardRange = '7d' | '30d' | 'all';

/**
 * One host's reporting state, for the freshness strip.
 *
 * `state` is three-valued on purpose. `offline` and `unavailable` must never be
 * rendered as zeroed counts — a host that cannot report is not a host with no
 * runs.
 */
export interface HostFreshness {
  host_id: string;
  name: string;
  transport_kind: string;
  state: 'ok' | 'offline' | 'unavailable';
  /** RFC-3339. Present only when `state === 'ok'`. */
  captured_at: string | null;
  /** Cause when `state !== 'ok'`, e.g. "needs rupu >= 0.49". */
  reason: string | null;
}

/** Live, non-terminal run counts — "is anything stuck right now". */
export interface ActiveCounts {
  running: number;
  awaiting_approval: number;
  paused: number;
  pending: number;
}

/** One day-bucket of terminal outcomes, for the trend area. */
export interface TerminalBucket {
  ts: string;
  completed: number;
  failed: number;
  rejected: number;
  cancelled: number;
}

/** One bar in the live swimlane. */
export interface ActiveRunBar {
  run_id: string;
  workflow_name: string;
  status: RunStatusStr;
  started_at: string;
  trigger: 'manual' | 'cron' | 'event';
  /** `null` for manual runs; set when the run belongs to an autoflow cycle. */
  cycle_id: string | null;
  host_id?: string;
}

/**
 * One run inside a cycle. Carries `status`, not just an id, because the
 * `+N clean` pill needs to know what folds. The server joins it — the client
 * must not fetch a run per id to expand one cycle.
 */
export interface CycleRun {
  run_id: string;
  /** `'unknown'` when the host could not resolve the run. */
  status: RunStatusStr | 'unknown';
}

/** One autoflow cycle, collapsed. The activity feed's primary row. */
export interface CycleRollup {
  cycle_id: string;
  worker_name: string | null;
  started_at: string;
  finished_at: string | null;
  /** `null` = this host does not report the breakdown (SSH — the CLI's autoflow
   *  history has no ran/skipped/failed fields). NOT zero: unknown is not "none". */
  ran: number | null;
  skipped: number | null;
  failed: number | null;
  runs: CycleRun[];
  host_id?: string;
}

/** A manual-trigger run. Never grouped. */
export interface DashboardRecentRun {
  id: string;
  workflow_name: string;
  status: RunStatusStr;
  started_at: string;
  finished_at: string | null;
  trigger: 'manual' | 'cron' | 'event';
  host_id?: string;
}

export interface DashboardResponse {
  hosts: HostFreshness[];
  /**
   * True when at least one host that DID report (`state === 'ok'`) omitted its
   * open-findings count. When true, `findings_open` is a partial sum across only
   * the hosts that reported it — **the UI must not present it as a fleet total**.
   */
  findings_partial: boolean;
  active: ActiveCounts;
  terminal_buckets: TerminalBucket[];
  active_runs: ActiveRunBar[];
  cycles: CycleRollup[];
  recent_manual: DashboardRecentRun[];
  /** `null` when no reporting host supplied a count. NOT zero. */
  findings_open: number | null;
  /**
   * RFC-3339. The OLDEST `captured_at` among reporting hosts — the honest
   * staleness bound for the merged aggregate. Present at the TOP level because
   * the server `#[serde(flatten)]`s a `DashboardSummary` into this response;
   * `HttpHostConnector` re-parses this same body as a bare `DashboardSummary`.
   */
  captured_at: string;
}
```

Replace the client method (currently `api.ts:1170`):

```ts
  getDashboard(range: DashboardRange = '30d'): Promise<DashboardResponse> {
    return request<DashboardResponse>(`/api/dashboard?range=${range}`);
  },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/api.test.ts`
Expected: PASS.

Run: `npx tsc --noEmit`
Expected: errors in `Dashboard.tsx` — it still consumes the old shape. That is expected; Task 6 rewrites it. Do not patch `Dashboard.tsx` here.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/lib/api.test.ts
git commit -m "feat(cp-web): dashboard API types for the multi-host contract

HostFreshness.state is three-valued so offline/unavailable hosts can never
be rendered as zeroed counts."
```

---

### Task 2: Cycle grouping + clean-folding (pure functions)

**Files:**
- Create: `crates/rupu-cp/web/src/lib/dashboard/feed.ts`
- Test: `crates/rupu-cp/web/src/lib/dashboard/feed.test.ts`

**Interfaces:**
- Consumes: `CycleRollup`, `DashboardRecentRun`, `ActiveRunBar` (Task 1)
- Produces: `buildFeed(cycles, recentManual): FeedRow[]`, `type FeedRow = CycleFeedRow | ManualFeedRow`, `isCycleInteresting(c): boolean`, `foldCleanRuns(runs): { shown, cleanCount }`

**Design note:** pure and I/O-free so grouping is testable without rendering, following the Okesu precedent where `cluster.ts` / `scale.ts` / `filter.ts` each have a co-located `.test.ts`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/lib/dashboard/feed.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { buildFeed, isCycleInteresting, foldCleanRuns } from './feed';
import type { CycleRollup, DashboardRecentRun } from '../api';

const cycle = (over: Partial<CycleRollup> = {}): CycleRollup => ({
  cycle_id: 'cyc_1',
  worker_name: 'nightly-review',
  started_at: '2026-07-16T03:00:00Z',
  finished_at: '2026-07-16T03:12:00Z',
  ran: 12,
  skipped: 0,
  failed: 0,
  runs: [
    { run_id: 'r1', status: 'completed' },
    { run_id: 'r2', status: 'completed' },
  ],
  ...over,
});

const manual = (over: Partial<DashboardRecentRun> = {}): DashboardRecentRun => ({
  id: 'run_m1',
  workflow_name: 'adhoc',
  status: 'completed',
  started_at: '2026-07-16T09:00:00Z',
  finished_at: '2026-07-16T09:01:00Z',
  trigger: 'manual',
  ...over,
});

describe('buildFeed', () => {
  it('emits one row per cycle, not one per run', () => {
    const rows = buildFeed(
      [
        cycle({
          runs: ['r1', 'r2', 'r3', 'r4'].map((run_id) => ({ run_id, status: 'completed' as const })),
        }),
      ],
      [],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe('cycle');
  });

  it('never groups manual runs', () => {
    const rows = buildFeed([], [manual({ id: 'a' }), manual({ id: 'b' })]);
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => r.kind === 'manual')).toBe(true);
  });

  it('sorts cycles and manual runs together, newest first', () => {
    const rows = buildFeed(
      [cycle({ started_at: '2026-07-16T03:00:00Z' })],
      [manual({ started_at: '2026-07-16T09:00:00Z' })],
    );
    expect(rows[0].kind).toBe('manual'); // 09:00 is newer than 03:00
  });
});

describe('isCycleInteresting', () => {
  it('is interesting when any run failed', () => {
    expect(isCycleInteresting(cycle({ failed: 2 }))).toBe(true);
  });

  it('a fully clean cycle is not interesting', () => {
    expect(isCycleInteresting(cycle({ ran: 12, failed: 0, skipped: 0 }))).toBe(false);
  });

  it('an unfinished cycle is interesting — it is still live work', () => {
    expect(isCycleInteresting(cycle({ finished_at: null, failed: 0 }))).toBe(true);
  });
});

describe('foldCleanRuns', () => {
  it('folds clean runs away and reports the count', () => {
    const runs = [
      { run_id: 'r1', status: 'completed' as const },
      { run_id: 'r2', status: 'completed' as const },
      { run_id: 'r3', status: 'failed' as const },
    ];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown.map((r) => r.run_id)).toEqual(['r3']);
    expect(cleanCount).toBe(2);
  });

  it('folds nothing when every run failed', () => {
    const runs = [{ run_id: 'r1', status: 'failed' as const }];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown).toHaveLength(1);
    expect(cleanCount).toBe(0);
  });

  it('keeps awaiting_approval visible — it is blocked on the operator', () => {
    const runs = [{ run_id: 'r1', status: 'awaiting_approval' as const }];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown).toHaveLength(1);
    expect(cleanCount).toBe(0);
  });

  it('never folds an unresolved run — unknown is not clean', () => {
    const runs = [{ run_id: 'r1', status: 'unknown' as const }];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown).toHaveLength(1);
    expect(cleanCount).toBe(0);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/dashboard/feed.test.ts`
Expected: FAIL — `Cannot find module './feed'`

- [ ] **Step 3: Implement**

Create `crates/rupu-cp/web/src/lib/dashboard/feed.ts`:

```ts
// feed — cycle grouping for the activity feed.
//
// The problem this solves: a chatty autoflow emitting twelve runs consumed the
// entire Recent Runs list, and the rows could not even be told apart from
// operator-launched ones. Grouping is by CYCLE, not by outcome: a cycle failing
// *as a cycle* is a real event, and outcome-grouping scatters that across rows.
//
// Pure and I/O-free so the grouping is testable without rendering.

import type { CycleRollup, DashboardRecentRun, RunStatusStr } from '../api';

export interface CycleFeedRow {
  kind: 'cycle';
  sortKey: string;
  cycle: CycleRollup;
}

export interface ManualFeedRow {
  kind: 'manual';
  sortKey: string;
  run: DashboardRecentRun;
}

export type FeedRow = CycleFeedRow | ManualFeedRow;

/**
 * Build the activity feed: one row per autoflow cycle, one row per manual run.
 *
 * Manual runs are NEVER grouped — they are the operator's own actions and each
 * one is an event they care about individually.
 */
export function buildFeed(
  cycles: CycleRollup[],
  recentManual: DashboardRecentRun[],
): FeedRow[] {
  const rows: FeedRow[] = [
    ...cycles.map((cycle): CycleFeedRow => ({
      kind: 'cycle',
      sortKey: cycle.started_at,
      cycle,
    })),
    ...recentManual.map((run): ManualFeedRow => ({
      kind: 'manual',
      sortKey: run.started_at,
      run,
    })),
  ];
  // RFC-3339 sorts correctly lexicographically. (This is why the Rust side
  // refuses non-RFC-3339 timestamps — see plan 2 task 1.)
  rows.sort((a, b) => (a.sortKey < b.sortKey ? 1 : a.sortKey > b.sortKey ? -1 : 0));
  return rows;
}

/**
 * Should this cycle be expanded by default?
 *
 * A clean, finished cycle is noise — that is the whole autoflow-flooding
 * problem. Anything unfinished or containing failures is signal.
 */
export function isCycleInteresting(c: CycleRollup): boolean {
  // `failed` is null when the host omits the breakdown — unknown is not "no
  // failures", but it is also not evidence of one. Fall through to the
  // finished/unfinished check rather than guessing either way.
  if (c.failed !== null && c.failed > 0) return true;
  if (c.finished_at === null) return true; // still running
  return false;
}

/**
 * The statuses that fold away as "clean". An ALLOW-list, not a deny-list:
 * anything we do not positively recognize as clean stays visible. `unknown`
 * (a run the host could not resolve) therefore never folds — hiding a run we
 * know nothing about is exactly the wrong default.
 */
const CLEAN_STATUSES: ReadonlySet<string> = new Set(['completed']);

/**
 * Fold clean runs behind a `+N clean` pill: hidden, never lost.
 *
 * Lifted from Okesu's `isInterestingTick` idiom. `awaiting_approval` and
 * `paused` deliberately never fold — they are blocked on the operator.
 */
export function foldCleanRuns<T extends { run_id: string; status: string }>(
  runs: T[],
): { shown: T[]; cleanCount: number } {
  const shown = runs.filter((r) => !CLEAN_STATUSES.has(r.status));
  return { shown, cleanCount: runs.length - shown.length };
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/dashboard/feed.test.ts`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/dashboard/feed.ts crates/rupu-cp/web/src/lib/dashboard/feed.test.ts
git commit -m "feat(cp-web): cycle grouping + clean-folding for the activity feed

Group by cycle, not by outcome: a cycle failing AS a cycle is a real event
that outcome-grouping would scatter. Manual runs never group.

awaiting_approval and paused never fold — they are blocked on the operator."
```

---

### Task 3: Swimlane layout (pure functions)

**Files:**
- Create: `crates/rupu-cp/web/src/lib/dashboard/swimlane.ts`
- Test: `crates/rupu-cp/web/src/lib/dashboard/swimlane.test.ts`

**Interfaces:**
- Consumes: `ActiveRunBar` (Task 1)
- Produces: `autoFitRange(bars, now): { start: number; end: number }`, `assignLanes(bars, groupBy): Lane[]`, `type Lane = { key: string; bars: PositionedBar[] }`, `type PositionedBar = { bar: ActiveRunBar; x0: number; x1: number }` (x values are 0..1 fractions of the fitted range)

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/lib/dashboard/swimlane.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { autoFitRange, assignLanes } from './swimlane';
import type { ActiveRunBar } from '../api';

const NOW = Date.parse('2026-07-16T12:00:00Z');

const bar = (over: Partial<ActiveRunBar> = {}): ActiveRunBar => ({
  run_id: 'r1',
  workflow_name: 'wf',
  status: 'running',
  started_at: '2026-07-16T11:55:00Z',
  trigger: 'manual',
  cycle_id: null,
  ...over,
});

describe('autoFitRange', () => {
  it('fits to the 5th/95th percentile, not min/max', () => {
    // Nineteen ~5-minute runs and one 6-hour outlier. Fitting to min/max would
    // crush the cluster into ~1% of the width.
    const bars = [
      ...Array.from({ length: 19 }, (_, i) =>
        bar({ run_id: `r${i}`, started_at: '2026-07-16T11:55:00Z' }),
      ),
      bar({ run_id: 'outlier', started_at: '2026-07-16T06:00:00Z' }),
    ];
    const { start, end } = autoFitRange(bars, NOW);
    const spanMinutes = (end - start) / 60_000;
    expect(spanMinutes).toBeLessThan(120);
  });

  it('always ends at now — the right edge is the present', () => {
    const { end } = autoFitRange([bar()], NOW);
    expect(end).toBe(NOW);
  });

  it('handles an empty bar list without producing NaN', () => {
    const { start, end } = autoFitRange([], NOW);
    expect(Number.isFinite(start)).toBe(true);
    expect(Number.isFinite(end)).toBe(true);
    expect(end).toBeGreaterThan(start);
  });
});

describe('assignLanes', () => {
  it('groups by workflow', () => {
    const lanes = assignLanes(
      [bar({ workflow_name: 'a' }), bar({ workflow_name: 'b' }), bar({ workflow_name: 'a' })],
      'workflow',
      NOW,
    );
    expect(lanes).toHaveLength(2);
    expect(lanes.find((l) => l.key === 'a')!.bars).toHaveLength(2);
  });

  it('groups by host', () => {
    const lanes = assignLanes(
      [bar({ host_id: 'local' }), bar({ host_id: 'builder-01' })],
      'host',
      NOW,
    );
    expect(lanes.map((l) => l.key).sort()).toEqual(['builder-01', 'local']);
  });

  it('positions bars as 0..1 fractions of the fitted range', () => {
    const lanes = assignLanes([bar()], 'workflow', NOW);
    const b = lanes[0].bars[0];
    expect(b.x0).toBeGreaterThanOrEqual(0);
    expect(b.x1).toBeLessThanOrEqual(1);
    expect(b.x1).toBeGreaterThan(b.x0);
  });

  it('clamps a bar that started before the fitted window to x0 = 0', () => {
    // The outlier is fitted out of the window but must still be drawn —
    // clipped to the left edge, not dropped or given a negative x.
    const bars = [
      ...Array.from({ length: 19 }, (_, i) => bar({ run_id: `r${i}` })),
      bar({ run_id: 'outlier', started_at: '2026-07-16T06:00:00Z' }),
    ];
    const lanes = assignLanes(bars, 'workflow', NOW);
    const outlier = lanes.flatMap((l) => l.bars).find((b) => b.bar.run_id === 'outlier')!;
    expect(outlier.x0).toBe(0);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/dashboard/swimlane.test.ts`
Expected: FAIL — `Cannot find module './swimlane'`

- [ ] **Step 3: Implement**

Create `crates/rupu-cp/web/src/lib/dashboard/swimlane.ts`:

```ts
// swimlane — layout math for the live activity hero.
//
// Pure and I/O-free: this is the code that decides whether the hero reads
// correctly, so it is unit-tested rather than eyeballed.
//
// Recharts has no Gantt, so the view is hand-rolled SVG (same call Okesu made
// for its war-room case timeline). This module owns the math; the component
// owns the paint.

import type { ActiveRunBar } from '../api';

export type LaneKey = 'workflow' | 'host';

export interface PositionedBar {
  bar: ActiveRunBar;
  /** Fraction 0..1 of the fitted range. Clamped — never negative. */
  x0: number;
  x1: number;
}

export interface Lane {
  key: string;
  bars: PositionedBar[];
}

/** Floor for the fitted window, so a handful of 3-second runs still read. */
const MIN_SPAN_MS = 60_000;

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.floor(p * (sorted.length - 1))));
  return sorted[idx];
}

/**
 * Fit the x-axis to the 5th percentile start, not the earliest start.
 *
 * Fitting to min/max lets a single 6-hour run crush every other bar into ~2% of
 * the width. Lifted from Okesu's `timeline/scale.ts:autoFitRange`. Bars outside
 * the fitted window are clamped by `assignLanes`, not dropped.
 */
export function autoFitRange(bars: ActiveRunBar[], now: number): { start: number; end: number } {
  if (bars.length === 0) {
    return { start: now - MIN_SPAN_MS * 15, end: now };
  }
  const starts = bars.map((b) => Date.parse(b.started_at)).sort((a, b) => a - b);
  const p5 = percentile(starts, 0.05);
  const span = Math.max(MIN_SPAN_MS, now - p5);
  return { start: now - span, end: now };
}

/**
 * Bucket bars into lanes and position them within the fitted range.
 *
 * Active runs have no end time — they are still running — so every bar's right
 * edge is `now`. That is not a placeholder: the bar genuinely extends to the
 * present.
 */
export function assignLanes(
  bars: ActiveRunBar[],
  groupBy: LaneKey,
  now: number,
): Lane[] {
  const { start, end } = autoFitRange(bars, now);
  const span = Math.max(1, end - start);

  const laneOf = (b: ActiveRunBar): string =>
    groupBy === 'host' ? (b.host_id ?? 'local') : b.workflow_name;

  const byLane = new Map<string, PositionedBar[]>();
  for (const b of bars) {
    const t0 = Date.parse(b.started_at);
    // Clamp rather than drop: a bar fitted out of the window is still real work
    // and must be visible, clipped to the left edge.
    const x0 = Math.min(1, Math.max(0, (t0 - start) / span));
    const key = laneOf(b);
    const arr = byLane.get(key) ?? [];
    arr.push({ bar: b, x0, x1: 1 });
    byLane.set(key, arr);
  }

  return [...byLane.entries()]
    .map(([key, laneBars]) => ({
      key,
      bars: laneBars.sort((a, b) => a.x0 - b.x0),
    }))
    .sort((a, b) => a.key.localeCompare(b.key));
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/dashboard/swimlane.test.ts`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/dashboard/swimlane.ts crates/rupu-cp/web/src/lib/dashboard/swimlane.test.ts
git commit -m "feat(cp-web): swimlane layout math

Percentile auto-fit (5th, not min) so one 6-hour run cannot crush every other
bar into 2% of the width. Out-of-window bars clamp to the left edge rather
than being dropped — they are still real work."
```

---

### Task 4: SSE invalidation hook with burst coalescing

**Files:**
- Create: `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.ts`
- Test: `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.test.ts`

**Interfaces:**
- Consumes: `api.getDashboard(range)`, `api.subscribeEvents(onEvent, opts, onError)` (both Task 1 / existing)
- Produces: `useDashboardData(range): { data, error, loading, refresh }`, and the exported pure helper `coalesce(fn, ms): { trigger, cancel }`

**Why invalidation, not deltas:** every number on this page is a server-computed aggregate; the stream carries step-level events. Applying deltas client-side would mean reimplementing the Rust aggregation in TypeScript and keeping them in agreement forever. They would drift, and a dashboard quietly showing wrong counts is worse than one that is 10s stale. So: arrival marks dirty → refetch. The server stays the only thing doing arithmetic.

**Why coalescing is load-bearing:** an autoflow cycle firing twelve runs produces a burst of events. Naive invalidation means twelve refetches. This is the piece that decides whether the page feels fast or hammers the server.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { coalesce } from './useDashboardData';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('coalesce', () => {
  it('collapses a burst into ONE call', () => {
    const fn = vi.fn();
    const { trigger } = coalesce(fn, 250);
    // An autoflow cycle firing 12 runs.
    for (let i = 0; i < 12; i++) trigger();
    vi.advanceTimersByTime(250);
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it('allows a second call after the window elapses', () => {
    const fn = vi.fn();
    const { trigger } = coalesce(fn, 250);
    trigger();
    vi.advanceTimersByTime(250);
    trigger();
    vi.advanceTimersByTime(250);
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it('does not fire before the window elapses', () => {
    const fn = vi.fn();
    const { trigger } = coalesce(fn, 250);
    trigger();
    vi.advanceTimersByTime(100);
    expect(fn).not.toHaveBeenCalled();
  });

  it('cancel prevents a pending call', () => {
    const fn = vi.fn();
    const { trigger, cancel } = coalesce(fn, 250);
    trigger();
    cancel();
    vi.advanceTimersByTime(500);
    expect(fn).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/dashboard/useDashboardData.test.ts`
Expected: FAIL — `Cannot find module './useDashboardData'`

- [ ] **Step 3: Implement**

Create `crates/rupu-cp/web/src/lib/dashboard/useDashboardData.ts`:

```ts
// useDashboardData — live dashboard state.
//
// SSE is an INVALIDATION SIGNAL, not a data channel. Every number on the
// dashboard is a server-computed aggregate; the event stream carries step-level
// events. Applying step deltas to aggregates client-side would mean
// reimplementing the Rust aggregation in TypeScript and keeping the two in
// agreement forever — they would drift, and a dashboard quietly showing WRONG
// counts is worse than one that is 10s stale.
//
// So: event arrival marks dirty, and we refetch the aggregate. The server stays
// the single source of truth for every number.
//
// Note the stream is LOCAL-ONLY: /api/events/stream requires ?run= for any
// remote host, and there is no cross-host firehose. Remote hosts therefore
// refresh on the reconciling poll, which is why per-host freshness is rendered
// rather than one global "live" pill (spec §5.4).

import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type DashboardRange, type DashboardResponse } from '../api';

/** Burst window. An autoflow cycle firing 12 runs must cost ONE refetch. */
const COALESCE_MS = 250;

/**
 * Reconciling poll. Runs regardless of SSE so a dropped connection degrades to
 * the old behavior instead of freezing.
 */
const RECONCILE_MS = 60_000;

/**
 * Collapse a burst of triggers into a single trailing call.
 *
 * Exported for testing — this is the piece that decides whether the page feels
 * fast or hammers the server.
 */
export function coalesce(fn: () => void, ms: number): { trigger: () => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return {
    trigger() {
      if (timer !== null) return; // a call is already pending — fold into it
      timer = setTimeout(() => {
        timer = null;
        fn();
      }, ms);
    },
    cancel() {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    },
  };
}

export function useDashboardData(range: DashboardRange) {
  const [data, setData] = useState<DashboardResponse | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);

  // Held in a ref so the SSE subscription and poll never re-subscribe just
  // because `range` changed identity.
  const rangeRef = useRef(range);
  rangeRef.current = range;

  const refresh = useCallback(async () => {
    try {
      const d = await api.getDashboard(rangeRef.current);
      setData(d);
      setError(null);
    } catch (e) {
      // Keep stale data on a transient error rather than flashing an error
      // state — a 10s-old number beats an empty page.
      setError(e instanceof Error ? e : new Error(String(e)));
    } finally {
      setLoading(false);
    }
  }, []);

  // Refetch immediately whenever the range changes.
  useEffect(() => {
    setLoading(true);
    void refresh();
  }, [range, refresh]);

  useEffect(() => {
    const { trigger, cancel } = coalesce(() => void refresh(), COALESCE_MS);

    // Payloads are deliberately ignored — arrival is the whole signal.
    const unsubscribe = api.subscribeEvents(() => trigger());

    const poll = setInterval(() => {
      // A dashboard in a background tab does no work.
      if (document.visibilityState === 'visible') void refresh();
    }, RECONCILE_MS);

    // Refetch on tab focus so returning to a backgrounded tab is never stale.
    const onVisible = () => {
      if (document.visibilityState === 'visible') trigger();
    };
    document.addEventListener('visibilitychange', onVisible);

    return () => {
      cancel();
      unsubscribe();
      clearInterval(poll);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [refresh]);

  return { data, error, loading, refresh };
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/dashboard/useDashboardData.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/dashboard/useDashboardData.ts crates/rupu-cp/web/src/lib/dashboard/useDashboardData.test.ts
git commit -m "feat(cp-web): SSE invalidation hook with burst coalescing

SSE marks dirty; the server recomputes. Payloads are ignored on purpose --
mirroring the Rust aggregation in TS would drift, and wrong counts beat stale
ones only in the sense that both are bad and drift is worse.

An autoflow cycle firing 12 runs costs ONE refetch, not twelve.
Reconciling 60s poll gated on tab visibility."
```

---

### Task 5: Presentational components

**Files:**
- Create: `crates/rupu-cp/web/src/components/dashboard/HostFreshnessStrip.tsx`
- Create: `crates/rupu-cp/web/src/components/dashboard/AttentionRow.tsx`
- Create: `crates/rupu-cp/web/src/components/dashboard/ActiveStatusTiles.tsx`
- Create: `crates/rupu-cp/web/src/components/dashboard/TerminalTrend.tsx`
- Create: `crates/rupu-cp/web/src/components/dashboard/Swimlane.tsx`
- Create: `crates/rupu-cp/web/src/components/dashboard/ActivityFeed.tsx`
- Test: `crates/rupu-cp/web/src/components/dashboard/HostFreshnessStrip.test.tsx`
- Test: `crates/rupu-cp/web/src/components/dashboard/ActivityFeed.test.tsx`

**Interfaces:**
- Consumes: Tasks 1–3 (`DashboardResponse` & friends, `buildFeed`, `assignLanes`), `useThemeColors()`, existing `TriggerChip`, `StatusPill`
- Produces: the six components above, consumed by Task 6's page.

**Colors:** every fill/stroke comes from `useThemeColors()`. The active tiles' segmented bars and `TerminalTrend`'s areas **must** read the same `status.*` tokens so the eye ties the live count to the history without a legend.

- [ ] **Step 1: Write the failing test for the freshness strip**

Create `crates/rupu-cp/web/src/components/dashboard/HostFreshnessStrip.test.tsx`:

```tsx
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { HostFreshnessStrip } from './HostFreshnessStrip';
import type { HostFreshness } from '../../lib/api';

const host = (over: Partial<HostFreshness> = {}): HostFreshness => ({
  host_id: 'local',
  name: 'local',
  transport_kind: 'local',
  state: 'ok',
  captured_at: new Date().toISOString(),
  reason: null,
  ...over,
});

describe('HostFreshnessStrip', () => {
  it('renders a fresh host as live', () => {
    render(<HostFreshnessStrip hosts={[host()]} />);
    expect(screen.getByText(/live/i)).toBeInTheDocument();
  });

  it('renders an unavailable host with its reason, NOT as zero', () => {
    render(
      <HostFreshnessStrip
        hosts={[
          host({
            host_id: 'builder-01',
            name: 'builder-01',
            state: 'unavailable',
            captured_at: null,
            reason: 'needs rupu >= 0.49',
          }),
        ]}
      />,
    );
    expect(screen.getByText(/unavailable/i)).toBeInTheDocument();
    expect(screen.getByTitle(/needs rupu/i)).toBeInTheDocument();
    expect(screen.queryByText('0')).not.toBeInTheDocument();
  });

  it('renders a stale host with its age rather than claiming live', () => {
    const thirtySecondsAgo = new Date(Date.now() - 30_000).toISOString();
    render(
      <HostFreshnessStrip hosts={[host({ host_id: 'b', name: 'b', captured_at: thirtySecondsAgo })]} />,
    );
    expect(screen.queryByText(/live/i)).not.toBeInTheDocument();
    expect(screen.getByText(/30s/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/dashboard/HostFreshnessStrip.test.tsx`
Expected: FAIL — `Cannot find module './HostFreshnessStrip'`

- [ ] **Step 3: Implement `HostFreshnessStrip`**

```tsx
// HostFreshnessStrip — per-host truth about how current this data is.
//
// One global "live" pill would lie about the SSH host. Liveness is
// per-transport (spec §5.4): local and HTTP hosts are sub-second via SSE, SSH
// and Bucket are poll-bounded, Tunnel/Bucket may not report at all. So each
// host carries its own freshness.
//
// This is also the host-status view rupu lacks entirely today.

import { useEffect, useState } from 'react';
import type { HostFreshness } from '../../lib/api';

/** Under this, a host reads as "live" rather than showing an age. */
const LIVE_THRESHOLD_MS = 5_000;

function age(capturedAt: string, now: number): string {
  const ms = now - Date.parse(capturedAt);
  if (ms < LIVE_THRESHOLD_MS) return 'live';
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  return `${Math.round(ms / 3_600_000)}h`;
}

export function HostFreshnessStrip({ hosts }: { hosts: HostFreshness[] }) {
  // Ticks so ages advance between refetches.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
      {hosts.map((h) => {
        const label =
          h.state === 'ok' && h.captured_at ? age(h.captured_at, now) : h.state;
        const isLive = label === 'live';
        const tone =
          h.state === 'ok'
            ? isLive
              ? 'bg-[rgb(var(--c-status-running))]'
              : 'bg-[rgb(var(--c-status-pending))]'
            : 'bg-[rgb(var(--c-status-failed))]';
        return (
          <span
            key={h.host_id}
            className="inline-flex items-center gap-1.5 text-[rgb(var(--c-ink-dim))]"
            title={h.reason ?? `${h.transport_kind} host`}
          >
            <span className={`h-1.5 w-1.5 rounded-full ${tone}`} aria-hidden />
            <span className="font-medium text-[rgb(var(--c-ink))]">{h.name}</span>
            <span className="text-[rgb(var(--c-ink-mute))]">·</span>
            <span>{label}</span>
          </span>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/dashboard/HostFreshnessStrip.test.tsx`
Expected: PASS (3 tests).

- [ ] **Step 5: Implement `ActiveStatusTiles`**

```tsx
// ActiveStatusTiles — the live half of the status split.
//
// Replaces the old donut's job of answering "is anything stuck right now". The
// donut could not: it rendered by_status seeded with all 8 variants at zero, so
// it was an 8-slice pie that was ~95% completed — a ratio answering "what
// fraction of all runs ever succeeded", which nobody asks.
//
// AwaitingApproval and Paused get weight because they are the ONLY states where
// the system is blocked on the operator.

import type { ActiveCounts } from '../../lib/api';

interface Tile {
  key: keyof ActiveCounts;
  label: string;
  cssVar: string;
  /** Blocked on the operator — rendered with weight. */
  needsYou: boolean;
}

const TILES: Tile[] = [
  { key: 'running', label: 'Running', cssVar: '--c-status-running', needsYou: false },
  { key: 'awaiting_approval', label: 'Awaiting you', cssVar: '--c-status-awaiting', needsYou: true },
  { key: 'paused', label: 'Paused', cssVar: '--c-status-paused', needsYou: true },
  { key: 'pending', label: 'Pending', cssVar: '--c-status-pending', needsYou: false },
];

export function ActiveStatusTiles({ active }: { active: ActiveCounts }) {
  const total = TILES.reduce((s, t) => s + active[t.key], 0);

  return (
    <div className="grid grid-cols-4 gap-3">
      {TILES.map((t) => {
        const value = active[t.key];
        const pct = total > 0 ? (value / total) * 100 : 0;
        return (
          <div
            key={t.key}
            className={`rounded-lg border p-3 ${
              t.needsYou && value > 0
                ? 'border-[rgb(var(--c-status-awaiting))] bg-[rgb(var(--c-surface))]'
                : 'border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))]'
            }`}
          >
            <div className="text-xs text-[rgb(var(--c-ink-dim))]">{t.label}</div>
            <div
              className={`mt-1 tabular-nums ${
                t.needsYou && value > 0 ? 'text-3xl font-semibold' : 'text-2xl'
              } text-[rgb(var(--c-ink))]`}
            >
              {value}
            </div>
            {/* Segmented bar, color-locked to TerminalTrend's palette so the
                eye ties the live count to the history without a legend. */}
            <div className="mt-2 h-[1.5px] w-full bg-[rgb(var(--c-border))]">
              <div
                className="h-full"
                style={{ width: `${pct}%`, backgroundColor: `rgb(var(${t.cssVar}))` }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 6: Implement `TerminalTrend`**

```tsx
// TerminalTrend — the outcome half of the status split.
//
// The 8 RunStatus variants split along a seam already in the Rust:
// is_terminal() is Completed|Failed|Rejected|Cancelled and deliberately
// EXCLUDES Paused (a paused run expects a resume). That exclusion is the design
// saying there are two populations. Active states are transient and belong in
// live counts; terminal outcomes need a time axis so failure trend is a slope.

import { ResponsiveContainer, AreaChart, Area, XAxis, YAxis, Tooltip, CartesianGrid } from 'recharts';
import { useThemeColors } from '../../lib/useThemeColors';
import type { TerminalBucket } from '../../lib/api';

export function TerminalTrend({ buckets }: { buckets: TerminalBucket[] }) {
  const colors = useThemeColors();

  const data = buckets.map((b) => ({
    ts: new Date(b.ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }),
    completed: b.completed,
    failed: b.failed,
    rejected: b.rejected,
    cancelled: b.cancelled,
  }));

  // Same tokens ActiveStatusTiles' segmented bars use.
  const series = [
    { key: 'completed', color: colors.get('status.completed') },
    { key: 'failed', color: colors.get('status.failed') },
    { key: 'rejected', color: colors.get('status.rejected') },
    { key: 'cancelled', color: colors.get('status.cancelled') },
  ];

  return (
    <div className="h-48 w-full">
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: -20 }}>
          <CartesianGrid stroke={colors.border} vertical={false} />
          <XAxis dataKey="ts" stroke={colors.inkMute} fontSize={11} tickLine={false} />
          <YAxis stroke={colors.inkMute} fontSize={11} tickLine={false} allowDecimals={false} />
          <Tooltip
            contentStyle={{
              background: colors.panel,
              border: `1px solid ${colors.border}`,
              borderRadius: 8,
              color: colors.ink,
            }}
          />
          {series.map((s) => (
            <Area
              key={s.key}
              type="monotone"
              dataKey={s.key}
              stackId="1"
              stroke={s.color}
              fill={s.color}
              fillOpacity={0.25}
              // No animation: liveness is per-transport, and an animating chart
              // implies a smoothness the SSH hosts do not have.
              isAnimationActive={false}
            />
          ))}
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}
```

- [ ] **Step 7: Implement `Swimlane`**

```tsx
// Swimlane — the live activity hero.
//
// The status tiles tell you HOW MANY; this tells you WHAT IS HAPPENING. A run
// executing 40x longer than its median is visually obvious here in a way no
// table makes it — that is why it earns the hero slot.
//
// Hand-rolled SVG: recharts has no Gantt. Bars DO NOT animate — they redraw on
// data. Local bars update sub-second via SSE while SSH bars step forward on the
// poll tick, and a smoothly-animating bar beside one jumping in 10s increments
// reads as broken.

import { useMemo, useState } from 'react';
import { assignLanes, type LaneKey } from '../../lib/dashboard/swimlane';
import { useThemeColors } from '../../lib/useThemeColors';
import type { ActiveRunBar } from '../../lib/api';

const ROW_H = 22;
const BAR_H = 10;

function colorFor(status: string, colors: ReturnType<typeof useThemeColors>): string {
  switch (status) {
    case 'awaiting_approval':
      return colors.get('status.awaiting');
    case 'paused':
      return colors.get('status.paused');
    case 'failed':
      return colors.get('status.failed');
    case 'pending':
      return colors.get('status.pending');
    default:
      return colors.get('status.running');
  }
}

export function Swimlane({
  bars,
  onSelect,
}: {
  bars: ActiveRunBar[];
  onSelect?: (runId: string) => void;
}) {
  const colors = useThemeColors();
  const [groupBy, setGroupBy] = useState<LaneKey>('workflow');

  // `now` is captured per render rather than ticked: bars redraw on data
  // arrival, not on a timer.
  const lanes = useMemo(() => assignLanes(bars, groupBy, Date.now()), [bars, groupBy]);

  if (bars.length === 0) {
    return (
      <div className="flex h-32 items-center justify-center text-sm text-[rgb(var(--c-ink-mute))]">
        Nothing running right now
      </div>
    );
  }

  return (
    <div>
      <div className="mb-2 flex items-center justify-end gap-1 text-xs">
        {(['workflow', 'host'] as LaneKey[]).map((k) => (
          <button
            key={k}
            onClick={() => setGroupBy(k)}
            className={`rounded px-2 py-0.5 ${
              groupBy === k
                ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
                : 'text-[rgb(var(--c-ink-mute))]'
            }`}
          >
            by {k}
          </button>
        ))}
      </div>
      <svg width="100%" height={lanes.length * ROW_H} role="img" aria-label="Active runs over time">
        {lanes.map((lane, i) => (
          <g key={lane.key} transform={`translate(0, ${i * ROW_H})`}>
            <text
              x={0}
              y={ROW_H / 2 + 4}
              fontSize={11}
              fill={colors.inkDim}
              className="select-none"
            >
              {lane.key}
            </text>
            {lane.bars.map((pb) => (
              <rect
                key={pb.bar.run_id}
                x={`${20 + pb.x0 * 78}%`}
                y={(ROW_H - BAR_H) / 2}
                width={`${Math.max(0.5, (pb.x1 - pb.x0) * 78)}%`}
                height={BAR_H}
                rx={2}
                fill={colorFor(pb.bar.status, colors)}
                onClick={() => onSelect?.(pb.bar.run_id)}
                style={{ cursor: onSelect ? 'pointer' : undefined }}
              >
                <title>{`${pb.bar.workflow_name} · ${pb.bar.status} · started ${new Date(
                  pb.bar.started_at,
                ).toLocaleTimeString()}`}</title>
              </rect>
            ))}
          </g>
        ))}
      </svg>
    </div>
  );
}
```

- [ ] **Step 8: Write the failing test for `ActivityFeed`**

Create `crates/rupu-cp/web/src/components/dashboard/ActivityFeed.test.tsx`:

```tsx
import { describe, it, expect } from 'vitest';
// fireEvent, not user-event: @testing-library/user-event is NOT a dependency
// and this plan adds none.
import { render, screen, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { ActivityFeed } from './ActivityFeed';
import type { CycleRollup, DashboardRecentRun } from '../../lib/api';

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

const cycle: CycleRollup = {
  cycle_id: 'cyc_1',
  worker_name: 'nightly-review',
  started_at: '2026-07-16T03:00:00Z',
  finished_at: '2026-07-16T03:12:00Z',
  ran: 12,
  skipped: 0,
  failed: 2,
  runs: [
    { run_id: 'r_ok_1', status: 'completed' },
    { run_id: 'r_ok_2', status: 'completed' },
    { run_id: 'r_bad', status: 'failed' },
  ],
};

const manualRun: DashboardRecentRun = {
  id: 'run_m1',
  workflow_name: 'adhoc',
  status: 'completed',
  started_at: '2026-07-16T09:00:00Z',
  finished_at: '2026-07-16T09:01:00Z',
  trigger: 'manual',
};

describe('ActivityFeed', () => {
  it('collapses a 12-run cycle into ONE row', () => {
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    expect(screen.getByText(/nightly-review/)).toBeInTheDocument();
    // The whole point: 12 runs, one row.
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
  });

  it('shows the cycle outcome tally', () => {
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    expect(screen.getByText(/2 failed/)).toBeInTheDocument();
  });

  it('renders manual runs individually, never grouped', () => {
    wrap(<ActivityFeed cycles={[]} recentManual={[manualRun, { ...manualRun, id: 'run_m2' }]} />);
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
  });

  it('folds clean runs behind a pill and shows the failure', () => {
    // The cycle has failures, so it auto-expands. Its two clean runs must fold
    // away; the failed one must stay visible.
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    expect(screen.getByText('r_bad')).toBeInTheDocument();
    expect(screen.queryByText('r_ok_1')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /\+2 clean/ })).toBeInTheDocument();
  });

  it('restores the clean runs when the pill is clicked — hidden, never lost', () => {
    wrap(<ActivityFeed cycles={[cycle]} recentManual={[]} />);
    fireEvent.click(screen.getByRole('button', { name: /\+2 clean/ }));
    expect(screen.getByText('r_ok_1')).toBeInTheDocument();
  });
});
```

- [ ] **Step 9: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/dashboard/ActivityFeed.test.tsx`
Expected: FAIL — `Cannot find module './ActivityFeed'`

- [ ] **Step 10: Implement `ActivityFeed` and `AttentionRow`**

```tsx
// ActivityFeed — the run feed, grouped by autoflow cycle.
//
// The problem: a chatty autoflow emitting twelve runs consumed the entire
// Recent Runs list (hard-capped at 10 rows server-side), and the rows carried
// no trigger so they could not even be told apart from operator-launched ones.
//
// One row per cycle, expandable. Manual runs always individual. Inside an
// expanded cycle, clean runs fold behind a `+N clean` pill: hidden, never lost.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { buildFeed, isCycleInteresting, foldCleanRuns } from '../../lib/dashboard/feed';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import type { CycleRollup, DashboardRecentRun } from '../../lib/api';

function CycleRow({ cycle }: { cycle: CycleRollup }) {
  const [open, setOpen] = useState(() => isCycleInteresting(cycle));
  // Clean runs fold behind a pill; `showClean` un-folds them. Hidden, never
  // lost — the count is always visible and always clickable.
  const [showClean, setShowClean] = useState(false);
  // ran/failed are null when the host does not report the breakdown (SSH).
  // Show what we know; never render a computed 0 from unknown inputs.
  const ok =
    cycle.ran !== null && cycle.failed !== null ? Math.max(0, cycle.ran - cycle.failed) : null;
  const { shown, cleanCount } = foldCleanRuns(cycle.runs);
  const visible = showClean ? cycle.runs : shown;

  return (
    <li className="border-b border-[rgb(var(--c-border))] px-3 py-2 last:border-0">
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 text-left text-sm"
      >
        <span className="text-[rgb(var(--c-ink-mute))]">{open ? '▾' : '▸'}</span>
        <span className="font-medium text-[rgb(var(--c-ink))]">
          {cycle.worker_name ?? cycle.cycle_id}
        </span>
        <span className="text-[rgb(var(--c-ink-dim))]">
          {/* Fall back to the runs list when the host omits the breakdown —
              it is the one count we always have. */}
          {cycle.ran ?? cycle.runs.length} runs
          {ok !== null && <> · {ok} ok</>}
          {cycle.failed !== null && cycle.failed > 0 && (
            <span className="text-[rgb(var(--c-status-failed))]">, {cycle.failed} failed</span>
          )}
          {cycle.skipped !== null && cycle.skipped > 0 && <span>, {cycle.skipped} skipped</span>}
        </span>
        <span className="ml-auto flex items-center gap-2">
          <TriggerChip trigger="cron" />
          {cycle.host_id && (
            <span className="text-xs text-[rgb(var(--c-ink-mute))]">{cycle.host_id}</span>
          )}
        </span>
      </button>
      {open && (
        <ul className="mt-2 space-y-1 pl-6">
          {visible.map((run) => (
            <li key={run.run_id} className="flex items-center gap-2">
              <Link
                to={`/runs/${run.run_id}`}
                className="text-xs text-[rgb(var(--c-ink-dim))] hover:text-[rgb(var(--c-ink))]"
              >
                {run.run_id}
              </Link>
              {/* StatusPill's prop is RunStatusStr, which does not include
                  'unknown' — an unresolved run gets a plain label rather than
                  a pill lying about a status we do not have. */}
              {run.status === 'unknown' ? (
                <span className="text-xs text-[rgb(var(--c-ink-mute))]">unresolved</span>
              ) : (
                <StatusPill status={run.status} />
              )}
            </li>
          ))}
          {cleanCount > 0 && !showClean && (
            <li>
              <button
                onClick={() => setShowClean(true)}
                className="rounded-full bg-[rgb(var(--c-surface))] px-2 py-0.5 text-xs text-[rgb(var(--c-ink-mute))] hover:text-[rgb(var(--c-ink))]"
              >
                +{cleanCount} clean
              </button>
            </li>
          )}
        </ul>
      )}
    </li>
  );
}

export function ActivityFeed({
  cycles,
  recentManual,
}: {
  cycles: CycleRollup[];
  recentManual: DashboardRecentRun[];
}) {
  const rows = buildFeed(cycles, recentManual);

  if (rows.length === 0) {
    return (
      <div className="p-6 text-center text-sm text-[rgb(var(--c-ink-mute))]">No activity yet</div>
    );
  }

  return (
    <ul className="divide-y divide-[rgb(var(--c-border))]">
      {rows.map((row) =>
        row.kind === 'cycle' ? (
          <CycleRow key={row.cycle.cycle_id} cycle={row.cycle} />
        ) : (
          <li key={row.run.id} className="px-3 py-2">
            <Link to={`/runs/${row.run.id}`} className="flex items-center gap-2 text-sm">
              <span className="font-medium text-[rgb(var(--c-ink))]">{row.run.workflow_name}</span>
              <span className="text-xs text-[rgb(var(--c-ink-mute))]">{row.run.id}</span>
              <span className="ml-auto flex items-center gap-2">
                <TriggerChip trigger={row.run.trigger} />
                {row.run.host_id && (
                  <span className="text-xs text-[rgb(var(--c-ink-mute))]">{row.run.host_id}</span>
                )}
              </span>
            </Link>
          </li>
        ),
      )}
    </ul>
  );
}
```

```tsx
// AttentionRow — the triage ribbon, weighted.
//
// Was four equal chips. But AwaitingApproval and Paused are the only states
// where the system is blocked ON THE OPERATOR; open findings is a backlog, not
// an interrupt. Equal weight was the bug.

import { Link } from 'react-router-dom';
import type { ActiveCounts } from '../../lib/api';

export function AttentionRow({
  active,
  failedInWindow,
  findingsOpen,
  findingsPartial,
}: {
  active: ActiveCounts;
  failedInWindow: number;
  /** `null` = nobody reported. Render "—", never "0". */
  findingsOpen: number | null;
  /** True = the number below is a partial sum. Mark it; never imply completeness. */
  findingsPartial: boolean;
}) {
  const blocked = active.awaiting_approval + active.paused;

  return (
    <div className="flex flex-wrap items-stretch gap-3">
      <Link
        to="/runs?lifecycle=active&status=awaiting_approval"
        className={`flex-1 rounded-lg border px-4 py-3 ${
          blocked > 0
            ? 'border-[rgb(var(--c-status-awaiting))] bg-[rgb(var(--c-surface))]'
            : 'border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))]'
        }`}
      >
        <div className="text-xs text-[rgb(var(--c-ink-dim))]">Blocked on you</div>
        <div className="text-2xl font-semibold tabular-nums text-[rgb(var(--c-ink))]">
          {blocked}
        </div>
      </Link>
      <Link
        to="/runs?lifecycle=failed"
        className={`flex-1 rounded-lg border px-4 py-3 ${
          failedInWindow > 0
            ? 'border-[rgb(var(--c-status-failed))]'
            : 'border-[rgb(var(--c-border))]'
        } bg-[rgb(var(--c-panel))]`}
      >
        <div className="text-xs text-[rgb(var(--c-ink-dim))]">Failed</div>
        <div className="text-2xl font-semibold tabular-nums text-[rgb(var(--c-ink))]">
          {failedInWindow}
        </div>
      </Link>
      {/* Demoted: a backlog, not an interrupt. */}
      <Link
        to="/findings"
        className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] px-4 py-3"
      >
        <div className="text-xs text-[rgb(var(--c-ink-dim))]">
          Open findings{findingsPartial && <span title="Some reporting hosts do not supply a findings count — this is a partial sum, not a fleet total."> (partial)</span>}
        </div>
        <div className="text-base tabular-nums text-[rgb(var(--c-ink-dim))]">
          {/* `null` means nobody reported. "—" not "0": unknown is not none. */}
          {findingsOpen === null ? '—' : `${findingsOpen}${findingsPartial ? '+' : ''}`}
        </div>
      </Link>
    </div>
  );
}
```

- [ ] **Step 11: Run all component tests**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/dashboard/`
Expected: PASS.

**Note:** if `TriggerChip`'s prop is not named `trigger`, read `src/components/TriggerChip.tsx` and match its actual signature rather than changing the component.

- [ ] **Step 12: Commit**

```bash
git add crates/rupu-cp/web/src/components/dashboard/
git commit -m "feat(cp-web): ops dashboard components

- HostFreshnessStrip: per-host truth; unavailable renders with a reason, never
  as zero
- ActiveStatusTiles: live counts; awaiting/paused weighted (blocked on you)
- TerminalTrend: outcome trend, isAnimationActive=false
- Swimlane: hand-rolled SVG hero; bars redraw on data, never animate
- ActivityFeed: one row per cycle, manual runs never grouped
- AttentionRow: weighted, findings demoted to a backlog

All colors via useThemeColors tokens; tiles and trend share status.* so the
eye links them without a legend."
```

---

### Task 6: Rewrite the page

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Dashboard.tsx` (full rewrite)
- Test: `crates/rupu-cp/web/src/pages/Dashboard.test.tsx` (create)

**Interfaces:**
- Consumes: everything from Tasks 1–5

**Removed:** the spend hero, `UsageTimelineStacked`, `ModelBreakdownTable`, the status donut, `POLL_MS`, and the `RecentRuns` `SortableTable`. Spend becomes a compact tile linking to `/usage` (Plan 3). **Do not delete `UsageTimelineStacked.tsx` / `ModelBreakdownTable.tsx`** — Plan 3's page reuses them.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Dashboard from './Dashboard';
import { api } from '../lib/api';

afterEach(() => vi.restoreAllMocks());

const payload = {
  hosts: [
    {
      host_id: 'local',
      name: 'local',
      transport_kind: 'local',
      state: 'ok' as const,
      captured_at: new Date().toISOString(),
      reason: null,
    },
  ],
  active: { running: 2, awaiting_approval: 1, paused: 0, pending: 0 },
  terminal_buckets: [],
  active_runs: [],
  cycles: [],
  recent_manual: [],
  findings_open: 3,
};

describe('Dashboard', () => {
  it('renders the freshness strip and active counts', async () => {
    vi.spyOn(api, 'getDashboard').mockResolvedValue(payload);
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
    expect(screen.getByText('Running')).toBeInTheDocument();
  });

  it('subscribes to the event stream for invalidation', async () => {
    vi.spyOn(api, 'getDashboard').mockResolvedValue(payload);
    const sub = vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(sub).toHaveBeenCalled());
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/Dashboard.test.tsx`
Expected: FAIL — the current page calls `getDashboard()` with no range and renders the spend hero.

- [ ] **Step 3: Rewrite `Dashboard.tsx`**

```tsx
// Dashboard — operations-first.
//
// Was spend-forward: the largest element on the page was cost and tokens. But a
// dashboard you leave open in a tab is an ops monitor; spend is something you
// review deliberately, on a cadence. Spend now lives at /usage with room to
// answer attribution and anomaly questions (plan 3).
//
// Composition (spec §5.1): freshness strip → attention row → swimlane hero →
// split status → activity feed.

import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useDashboardData } from '../lib/dashboard/useDashboardData';
import { HostFreshnessStrip } from '../components/dashboard/HostFreshnessStrip';
import { AttentionRow } from '../components/dashboard/AttentionRow';
import { ActiveStatusTiles } from '../components/dashboard/ActiveStatusTiles';
import { TerminalTrend } from '../components/dashboard/TerminalTrend';
import { Swimlane } from '../components/dashboard/Swimlane';
import { ActivityFeed } from '../components/dashboard/ActivityFeed';
import type { DashboardRange } from '../lib/api';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))]">
      <h2 className="border-b border-[rgb(var(--c-border))] px-3 py-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
        {title}
      </h2>
      <div className="p-3">{children}</div>
    </section>
  );
}

export default function Dashboard() {
  const [range, setRange] = useState<DashboardRange>('30d');
  const { data, error, loading } = useDashboardData(range);

  const failedInWindow = useMemo(
    () => (data?.terminal_buckets ?? []).reduce((s, b) => s + b.failed, 0),
    [data],
  );

  if (loading && !data) {
    return <div className="p-6 text-sm text-[rgb(var(--c-ink-mute))]">Loading…</div>;
  }
  if (!data) {
    return (
      <div className="p-6 text-sm text-[rgb(var(--c-status-failed))]">
        Could not load dashboard: {error?.message}
      </div>
    );
  }

  return (
    <div className="space-y-4 p-4">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold text-[rgb(var(--c-ink))]">Dashboard</h1>
          <div className="mt-1">
            <HostFreshnessStrip hosts={data.hosts} />
          </div>
        </div>
        <div className="flex items-center gap-2">
          {/* Stale data is kept on a transient error rather than flashing an
              error state; surface it quietly instead. */}
          {error && (
            <span className="text-xs text-[rgb(var(--c-status-failed))]" title={error.message}>
              refresh failed — showing last good data
            </span>
          )}
          <div className="flex rounded-md border border-[rgb(var(--c-border))]">
            {RANGES.map((r) => (
              <button
                key={r}
                onClick={() => setRange(r)}
                className={`px-2 py-1 text-xs ${
                  range === r
                    ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
                    : 'text-[rgb(var(--c-ink-mute))]'
                }`}
              >
                {r}
              </button>
            ))}
          </div>
          <Link
            to="/usage"
            className="rounded-md border border-[rgb(var(--c-border))] px-3 py-1 text-xs text-[rgb(var(--c-ink-dim))] hover:text-[rgb(var(--c-ink))]"
          >
            Spend →
          </Link>
        </div>
      </header>

      <AttentionRow
        active={data.active}
        failedInWindow={failedInWindow}
        findingsOpen={data.findings_open}
        findingsPartial={data.findings_partial}
      />

      <Panel title="Live activity">
        <Swimlane bars={data.active_runs} />
      </Panel>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Panel title="Active now">
          <ActiveStatusTiles active={data.active} />
        </Panel>
        <Panel title="Outcomes over time">
          <TerminalTrend buckets={data.terminal_buckets} />
        </Panel>
      </div>

      <Panel title="Activity">
        <ActivityFeed cycles={data.cycles} recentManual={data.recent_manual} />
      </Panel>
    </div>
  );
}
```

- [ ] **Step 4: Run tests + typecheck + build**

Run: `cd crates/rupu-cp/web && npx vitest run`
Expected: PASS (all suites).

Run: `npx tsc --noEmit`
Expected: clean.

Run: `npm run build`
Expected: SUCCESS.

- [ ] **Step 5: Runtime validation in a browser — REQUIRED**

Per CLAUDE.md, build + test cleanliness is not rendering cleanliness. This step is not optional.

```bash
# terminal 1
cargo run -p rupu-cli -- cp serve
# terminal 2
cd crates/rupu-cp/web && npm run dev
```

Open `http://127.0.0.1:5173/dashboard` and confirm:
- [ ] The freshness strip shows `local` and it reads `live` (not a stale age).
- [ ] Launching a run makes it appear in the swimlane **without a manual refresh** (SSE invalidation works).
- [ ] An autoflow cycle renders as **one** row, not N.
- [ ] A cycle with failures is expanded by default; a clean finished cycle is collapsed.
- [ ] Toggling `by workflow` / `by host` re-lanes the swimlane.
- [ ] The range control refetches.
- [ ] Light **and** dark themes both read correctly (toggle the theme).
- [ ] Stopping `cp serve` leaves the last-good data on screen with the quiet "refresh failed" note, rather than an empty page.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Dashboard.tsx crates/rupu-cp/web/src/pages/Dashboard.test.tsx
git commit -m "feat(cp-web): rewrite Dashboard as an ops surface

Swimlane hero, split status, cycle-grouped feed, weighted attention row,
per-host freshness. Spend demotes to a link to /usage.

Drops the 15s poll for SSE invalidation + a 60s reconciling poll gated on tab
visibility. Drops the status donut: it rendered an 8-slice pie that was ~95%
completed, answering a question nobody asks.

UsageTimelineStacked / ModelBreakdownTable are intentionally left in place --
plan 3's spend page reuses them."
```

---

## Plan 1 Definition of Done

- [ ] `npx vitest run` passes; `npx tsc --noEmit` clean; `npm run build` succeeds.
- [ ] No new entries in `package.json` dependencies.
- [ ] No hardcoded color literals — every fill/stroke resolves through `useThemeColors()` or a `--c-*` token.
- [ ] An autoflow cycle renders as one row; manual runs render individually.
- [ ] A new run appears in the swimlane without a manual refresh.
- [ ] A 12-run burst triggers exactly one refetch (covered by `useDashboardData.test.ts`).
- [ ] An unavailable/offline host renders with a reason, never as `0`.
- [ ] Browser-validated in **both** light and dark themes (Task 6 Step 5).
- [ ] `make cp-web` run before any release build.
