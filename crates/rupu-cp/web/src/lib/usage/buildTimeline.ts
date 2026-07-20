// buildTimeline — the client-side pure function behind the interactive
// `/usage` spend graph. `GET /api/usage/runs` (Task U1) returns flat
// per-`(run × model)` `UsageRunRow`s; this buckets them by day/week and
// stacks them by a pivot dimension into the SAME `UsageTimelineBucket[]`
// shape `UsageTimelineStacked` already renders (fed today by
// `GET /api/usage/timeline`, which is model-only). Because the whole flat
// row list is held client-side, excluding a run or a pivot key is an
// in-memory filter — no refetch — which is the point: pulling a ~1000x-cost
// outlier out of the graph must be instant so the y-axis rescales live.
//
// Pure and I/O-free, like `src/lib/dashboard/mergeSummaries.ts` — no fetch,
// no `Date.now()`, co-located test.
//
// --- Component-shape note (read before changing the pivot->field mapping) ---
// `UsageTimelineStacked`'s `toChartData` derives each row's series key as
// `r.model || r.provider || r.agent || '—'` — it has no notion of
// `workflow`/`host_id`/`workspace_id` as series keys (it was built for the
// model-only `/api/usage/timeline`; see its file-header comment). For
// `pivot` values outside `{model, provider, agent}` (`workflow`/`host`/
// `project`) we therefore ALSO mirror the pivot key into the row's `model`
// field so the existing component still resolves distinct series — see
// `toBreakdownRow` below. This does not change the meaning of `model` for
// the `model`/`provider`/`agent` pivots (where the matching field alone is
// populated, mirroring the "only the active group_by field is non-empty"
// convention `UsageBreakdownRow` already documents).
//
// --- Grid note ---
// `toChartData` maps buckets 1:1 into `AreaChart` data with a categorical
// (string) x-axis — it does not require a continuous time domain, and
// zero-fills only WITHIN a bucket (one 0 per model absent from that
// bucket), never invents whole missing buckets. `buildTimeline` therefore
// does not gap-fill either: unlike `/api/usage/timeline`'s
// `build_timeline` (which fills every day across an explicit
// `[fill_start, fill_end]` window), this function has no window
// boundaries in its signature — only the rows themselves — so there is
// nothing correct to fill "up to". Buckets are emitted only for periods
// with at least one surviving row, sorted chronologically.

import type { UsageBreakdownRow, UsageRunRow, UsageTimelineBucket } from '../usage';
import type { Pivot } from '../api';

export type { Pivot } from '../api';

/** Client-side exclusion state for the interactive graph. A row is dropped
 *  when its `run_id` is excluded OR its pivot-key (the value of whichever
 *  field the active `pivot` selects) is excluded. */
export interface TimelineFilter {
  excludedRunIds: Set<string>;
  excludedKeys: Set<string>;
}

/** The pivot-key value for one row, per the active pivot dimension. */
function pivotKeyOf(row: UsageRunRow, pivot: Pivot): string {
  switch (pivot) {
    case 'model':
      return row.model;
    case 'provider':
      return row.provider;
    case 'agent':
      return row.agent;
    case 'workflow':
      return row.workflow_name;
    case 'host':
      return row.host_id;
    case 'project':
      return row.workspace_id;
  }
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : `${n}`;
}

function ymd(y: number, mZeroBased: number, d: number): string {
  return `${y}-${pad2(mZeroBased + 1)}-${pad2(d)}`;
}

/**
 * Map `started_at` (RFC-3339, Z-suffixed) to its bucket key, matching the
 * server's `bucket_key` (`rupu-cp/src/api/usage.rs`) exactly: `day` -> the
 * UTC calendar day; `week` -> that day's ISO-Monday. Both `YYYY-MM-DD`, so
 * they align with server-produced bucket keys if the two are ever mixed.
 */
function bucketKeyOf(startedAt: string, bucket: 'day' | 'week'): string {
  const dt = new Date(startedAt);
  const y = dt.getUTCFullYear();
  const m = dt.getUTCMonth();
  const d = dt.getUTCDate();
  if (bucket === 'day') return ymd(y, m, d);

  // ISO weekday: Monday=0 ... Sunday=6 (JS getUTCDay is Sunday=0..Saturday=6).
  const isoDow = (dt.getUTCDay() + 6) % 7;
  const monday = new Date(Date.UTC(y, m, d - isoDow));
  return ymd(monday.getUTCFullYear(), monday.getUTCMonth(), monday.getUTCDate());
}

interface Agg {
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  /** Sum of non-null `cost_usd` contributions only. */
  costSum: number;
  sawPriced: boolean;
  sawUnpriced: boolean;
  runIds: Set<string>;
}

function newAgg(): Agg {
  return {
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    total_tokens: 0,
    costSum: 0,
    sawPriced: false,
    sawUnpriced: false,
    runIds: new Set(),
  };
}

/**
 * Build one output `UsageBreakdownRow` for a pivot-key's aggregate. Sets
 * ONLY the field matching `pivot` to `key` (mirroring the "only the active
 * group_by field is non-empty" convention documented on `UsageBreakdownRow`
 * elsewhere) — with one deliberate exception: for `workflow`/`host`/
 * `project` pivots, `model` is ALSO set to `key` so `UsageTimelineStacked`'s
 * `model || provider || agent` series-key derivation still resolves a
 * distinct series per pivot key. See the file-header note.
 *
 * `cost_usd`: `null` only when NO contributing row was priced (mirrors
 * `UsageSummary.cost_usd`'s documented meaning); otherwise the sum of the
 * priced contributions, even if some rows in the group were unpriced.
 * `priced` is false whenever any contributor was unpriced.
 */
function toBreakdownRow(pivot: Pivot, key: string, agg: Agg): UsageBreakdownRow {
  const out: UsageBreakdownRow = {
    provider: '',
    model: '',
    agent: '',
    workflow: '',
    host_id: '',
    workspace_id: '',
    input_tokens: agg.input_tokens,
    output_tokens: agg.output_tokens,
    cached_tokens: agg.cached_tokens,
    total_tokens: agg.total_tokens,
    cost_usd: agg.sawPriced ? agg.costSum : null,
    priced: agg.sawPriced && !agg.sawUnpriced,
    runs: agg.runIds.size,
  };
  switch (pivot) {
    case 'model':
      out.model = key;
      break;
    case 'provider':
      out.provider = key;
      break;
    case 'agent':
      out.agent = key;
      break;
    case 'workflow':
      out.workflow = key;
      out.model = key;
      break;
    case 'host':
      out.host_id = key;
      out.model = key;
      break;
    case 'project':
      out.workspace_id = key;
      out.model = key;
      break;
  }
  return out;
}

/**
 * Bucket `rows` by day/week and stack them by `pivot`, skipping any row
 * excluded by `filter` (by `run_id` or by its pivot-key) — the whole point
 * being that toggling `filter` re-runs this synchronously with no refetch.
 * Output buckets are sparse (only periods with a surviving row) and sorted
 * chronologically — see the grid note above for why this doesn't gap-fill.
 */
export function buildTimeline(
  rows: UsageRunRow[],
  pivot: Pivot,
  filter: TimelineFilter,
  bucket: 'day' | 'week',
): UsageTimelineBucket[] {
  const byBucket = new Map<string, Map<string, Agg>>();

  for (const row of rows) {
    if (filter.excludedRunIds.has(row.run_id)) continue;
    const key = pivotKeyOf(row, pivot);
    if (filter.excludedKeys.has(key)) continue;

    const bKey = bucketKeyOf(row.started_at, bucket);
    let byKey = byBucket.get(bKey);
    if (!byKey) {
      byKey = new Map();
      byBucket.set(bKey, byKey);
    }
    let agg = byKey.get(key);
    if (!agg) {
      agg = newAgg();
      byKey.set(key, agg);
    }

    agg.input_tokens += row.input_tokens;
    agg.output_tokens += row.output_tokens;
    agg.cached_tokens += row.cached_tokens;
    agg.total_tokens += row.total_tokens;
    if (row.cost_usd == null) {
      agg.sawUnpriced = true;
    } else {
      agg.sawPriced = true;
      agg.costSum += row.cost_usd;
    }
    agg.runIds.add(row.run_id);
  }

  return [...byBucket.keys()].sort().map((bKey) => {
    const byKey = byBucket.get(bKey)!;
    const outRows = [...byKey.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, agg]) => toBreakdownRow(pivot, key, agg));
    return { bucket: bKey, rows: outRows };
  });
}
