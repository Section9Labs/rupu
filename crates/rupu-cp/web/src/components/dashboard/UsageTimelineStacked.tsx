// Usage timeline — a stacked area chart. X = time bucket; one stacked series
// per pivot key. Metric is either spend ($) or tokens, toggled by the parent.
//
// Stacks by whichever `pivot` dimension is active (default `'model'`,
// preserving this component's original behavior for callers that don't pass
// one). The series key comes from `pivotLabel` (`./modelColors.ts`) — the
// same helper `ModelBreakdownTable` already uses to read the one identity
// field a `UsageBreakdownRow` populates for the active `group_by` — so this
// chart genuinely varies by pivot instead of only resolving
// `model`/`provider`/`agent`. (Earlier, `GET /api/usage/timeline` had no
// `group_by` of its own and always grouped by model server-side, so this
// component only ever saw model-keyed rows; `buildTimeline` — the client-side
// aggregation behind the interactive `/usage` graph — now buckets+stacks
// arbitrary pivots itself, and used to work around this component's
// model-only assumption by mirroring the pivot key into `model`. That
// workaround is gone; this component resolves the real field directly.)
//
// `hosts` (optional) maps a `host` pivot's raw `host_id` keys to their
// friendly `name` for the legend/tooltip — same idiom as
// `ModelBreakdownTable`'s `hosts` prop. The raw `host_id` stays the actual
// stacking/color key; only the rendered label changes.
//
// Rendered by the shared living-chart engine (`components/ui/Chart`, pure SVG) —
// a stacked area that varies by pivot, with a dynamic per-pivot palette, a
// $/tokens y-axis, the marquee drag-select, and its own legend below. The pure
// `toChartData` transform is exported for unit testing.

import type { UsageTimelineBucket } from '../../lib/usage';
import { formatCost, formatTokens } from '../../lib/usage';
import { useThemeColors } from '../../lib/useThemeColors';
import type { Pivot } from '../../lib/api';
import { assignModelColors, pivotLabel } from './modelColors';
import { assignCategoricalColors } from '../usage/pivotColors';
import { Chart, type ChartSeries } from '../ui/Chart';
import { Skeleton } from '../ui/Skeleton';

export type UsageMetric = 'cost' | 'tokens';

/** One row of the chart: the bucket label plus a numeric value per pivot key. */
export interface ChartDatum {
  bucket: string;
  [key: string]: number | string;
}

export interface ChartData {
  /** Sorted, stable list of pivot keys (one stacked series each). */
  models: string[];
  data: ChartDatum[];
}

/**
 * Flatten timeline buckets into recharts rows. Collect every pivot key across
 * all buckets, then emit one datum per bucket with a value for *every* key (0
 * when absent — required so the stacked areas line up). For `cost`, unpriced
 * rows (`cost_usd == null`) contribute 0; for `tokens` every row contributes.
 * `pivot` (default `'model'`) selects which `UsageBreakdownRow` field is the
 * series key, via `pivotLabel`.
 */
export function toChartData(
  buckets: UsageTimelineBucket[],
  metric: UsageMetric,
  pivot: Pivot = 'model',
): ChartData {
  const models = [...new Set(buckets.flatMap((b) => b.rows.map((r) => pivotLabel(r, pivot))))].sort();

  const data: ChartDatum[] = buckets.map((b) => {
    const datum: ChartDatum = { bucket: b.bucket };
    for (const m of models) datum[m] = 0;
    for (const r of b.rows) {
      const key = pivotLabel(r, pivot);
      const value = metric === 'cost' ? r.cost_usd ?? 0 : r.total_tokens;
      datum[key] = (datum[key] as number) + value;
    }
    return datum;
  });

  return { models, data };
}

/** Short bucket tick: `2026-06-21` → `06-21`. */
function bucketTick(b: string): string {
  return b.length >= 10 ? b.slice(5) : b;
}

export default function UsageTimelineStacked({
  buckets,
  metric,
  pivot = 'model',
  hosts,
  onSelectRange,
  loading = false,
}: {
  buckets: UsageTimelineBucket[];
  metric: UsageMetric;
  /** Which `UsageBreakdownRow` field to stack by. Defaults to `'model'`,
   *  preserving this component's original model-only behavior for callers
   *  that don't pass one. */
  pivot?: Pivot;
  /** Per-host name refs (from `useUsageData`) — maps a `host` pivot's raw `host_id`
   *  series keys to their friendly `name` for the legend/tooltip. Optional;
   *  falls back to the raw id when absent or unmatched. Ignored for every
   *  other pivot. */
  hosts?: { host_id: string; name: string }[];
  /**
   * Marquee drag-select (Task W3) — called with the ordered `(startDay,
   * endDay)` day-bucket labels once a real drag (not a plain click)
   * finishes. Optional and INERT when omitted: the mouse handlers wired to
   * the chart below no-op immediately (see `useDragSelection`), so callers
   * that don't pass this prop (the dashboard's other consumers of this
   * component) see no behavior change at all.
   */
  onSelectRange?: (startDay: string, endDay: string) => void;
  /**
   * The caller hasn't heard back from its fetch yet — distinct from
   * "loaded and genuinely empty" (`buckets` is `[]` AFTER the fetch
   * resolved). Without this, an empty initial `buckets={[]}` while the
   * request is in flight rendered the exact same "No usage recorded yet"
   * copy as a real empty result, so a slow network read as "there's
   * nothing here" instead of "still loading". Optional, defaults to
   * `false` — every existing caller that doesn't track loading keeps its
   * current behavior verbatim.
   */
  loading?: boolean;
}) {
  const theme = useThemeColors();
  const { models, data } = toChartData(buckets, metric, pivot);

  if (loading) {
    return (
      <div className="h-[240px] flex flex-col justify-end gap-2 p-2" aria-busy="true" aria-label="Loading usage graph">
        <Skeleton className="h-[80%] w-full" />
        <div className="flex gap-3">
          <Skeleton className="h-3 w-16" />
          <Skeleton className="h-3 w-20" />
          <Skeleton className="h-3 w-12" />
        </div>
      </div>
    );
  }

  if (data.length === 0 || models.length === 0) {
    return (
      <div className="h-[240px] flex flex-col items-center justify-center text-center">
        <div className="w-10 h-10 rounded-full bg-surface mb-2" />
        <p className="text-xs text-ink-mute">No usage recorded yet</p>
      </div>
    );
  }

  // Model IDENTITY keeps its own dedicated palette (`assignModelColors`);
  // every other pivot has no identity of its own and uses the themed
  // categorical ramp — same split `ModelBreakdownTable` makes.
  const colors = pivot === 'model' ? assignModelColors(models) : assignCategoricalColors(models, theme);
  const fmt = (v: number) => (metric === 'cost' ? formatCost(v) : formatTokens(v));

  // Host pivot rows are keyed by raw `host_id` (via `pivotLabel`) — map to
  // the friendly `name` for display only; the raw id stays the stacking/color
  // key so dedup and the color assignment above are unaffected.
  const hostNameById = new Map((hosts ?? []).map((h) => [h.host_id, h.name]));
  const displayLabel = (key: string): string => (pivot === 'host' ? hostNameById.get(key) ?? key : key);

  // One stacked series per pivot key, bottom-first in the sorted `models`
  // order. `rawColor` carries the dynamic per-pivot palette into the engine
  // (the one place the shared Chart paints outside the theme tokens). The full
  // `bucket` is the drag-select key; the short `bucketTick` is only the x-label.
  const chartSeries: ChartSeries[] = models.map((m) => {
    const values = data.map((d) => (d[m] as number) ?? 0);
    return {
      key: m,
      label: displayLabel(m),
      values,
      rawColor: colors.get(m),
      legendValue: fmt(values.reduce((sum, v) => sum + v, 0)),
    };
  });
  const dates = data.map((d) => bucketTick(d.bucket));
  const selectKeys = data.map((d) => d.bucket);

  return (
    <Chart
      kind="area"
      showCard={false}
      dates={dates}
      selectKeys={selectKeys}
      series={chartSeries}
      yFormat={fmt}
      yTickFormat={metric === 'cost' ? costTick : fmt}
      onSelectRange={onSelectRange}
      height={220}
      ariaLabel={`Usage over time (${metric === 'cost' ? 'spend' : 'tokens'})`}
    />
  );
}

/** A cost tick: whole dollars on a round axis, cents only for sub-$10 steps. */
function costTick(v: number): string {
  if (v >= 1000) return `$${(v / 1000).toFixed(v % 1000 === 0 ? 0 : 1)}k`;
  if (v < 10 && v % 1 !== 0) return `$${v.toFixed(2)}`;
  return `$${Math.round(v)}`;
}
