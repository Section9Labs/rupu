import type { UsageTimelinePoint } from '../../lib/api';
import { Chart, type ChartRefLine, type ChartSeries } from '../ui/Chart';

/** Chart datum — all three series positive. `in` is drawn on the LEFT axis (its
 *  own large scale); `out` and `cached` share a RIGHT axis scaled to their own
 *  (much smaller) range, so they stay legible instead of being flattened by input. */
export interface UsageChartPoint extends UsageTimelinePoint {
  in: number;
  out: number;
  cached: number;
}

/** Abbreviated token-count tick label: `2000 → "2k"`, `1_200_000 → "1.2M"`,
 *  `500 → "500"`, `0 → "0"`. */
export function formatTokenTick(v: number): string {
  const n = Math.abs(v);
  if (n >= 1_000_000_000) return `${trimZero(n / 1_000_000_000)}B`;
  if (n >= 1_000_000) return `${trimZero(n / 1_000_000)}M`;
  if (n >= 1_000) return `${trimZero(n / 1_000)}k`;
  return String(n);
}

function trimZero(x: number): string {
  // `2.0` → `2`, `1.5` → `1.5` (one-decimal, no trailing `.0`).
  return x.toFixed(1).replace(/\.0$/, '');
}

/** Map a timeline point to its chart datum. All three series stay positive; the
 *  left/right axis split (not negation) is what keeps them readable. */
export function toChartPoint(p: UsageTimelinePoint): UsageChartPoint {
  return { ...p, in: p.tokens_in, out: p.tokens_out, cached: p.tokens_cached };
}

/** Per-turn token timeline across a run's steps (or a session's runs). Input is an
 *  area on the LEFT axis; output and cached are lines on a RIGHT axis scaled to
 *  their own (smaller) range — so output/cached are legible even when input dwarfs
 *  them. With `separators`, a dashed line marks each step boundary. Rendered by
 *  the shared living-chart engine (`kind="composed"`, pure SVG). */
export default function RunUsageTimeline({
  series,
  separators = false,
}: {
  series: UsageTimelinePoint[];
  separators?: boolean;
}) {
  if (series.length === 0) {
    return <div className="text-xs text-ink-mute py-6 text-center">No per-turn usage yet</div>;
  }
  const points = series.map(toChartPoint);
  const dates = points.map((p) => String(p.turn));
  // Richer tooltip title than the bare turn number: "turn 3 · review".
  const tooltipLabels = points.map((p) => (p.label ? `turn ${p.turn} · ${p.label}` : `turn ${p.turn}`));
  // `in` is the hero area on the left axis; `out`/`cached` ride the right axis
  // so they stay legible even when input dwarfs them.
  const chartSeries: ChartSeries[] = [
    { key: 'in', label: 'In', values: points.map((p) => p.in), color: 'status.running', mark: 'area', axis: 'left' },
    { key: 'out', label: 'Out', values: points.map((p) => p.out), color: 'status.done', mark: 'line', axis: 'right' },
    { key: 'cached', label: 'Cached', values: points.map((p) => p.cached), color: 'status.awaiting', mark: 'line', axis: 'right' },
  ];
  // First turn of each new label group (skip index 0) — the step boundaries.
  const refLines: ChartRefLine[] = separators
    ? points.reduce<ChartRefLine[]>((acc, p, i) => {
        if (i > 0 && p.label !== points[i - 1].label) acc.push({ index: i, label: p.label });
        return acc;
      }, [])
    : [];
  return (
    <Chart
      kind="composed"
      showCard={false}
      dates={dates}
      series={chartSeries}
      yFormat={formatTokenTick}
      yFormatRight={formatTokenTick}
      refLines={refLines}
      tooltipLabels={tooltipLabels}
      ariaLabel="Per-turn token usage"
    />
  );
}
