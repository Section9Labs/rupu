// Per-model usage timeline — a stacked area chart. X = time bucket; one stacked
// series per model. Metric is either spend ($) or tokens, toggled by the parent.
//
// recharts is the only heavy import here; it is force-split into the `charts`
// rollup chunk (see vite.config manualChunks), so it never lands in the main
// bundle. The pure `toChartData` transform is exported for unit testing.

import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import type { UsageTimelineBucket } from '../../lib/usage';
import { formatCost, formatTokens } from '../../lib/usage';
import { assignModelColors } from './modelColors';

export type UsageMetric = 'cost' | 'tokens';

/** One row of the chart: the bucket label plus a numeric value per model key. */
export interface ChartDatum {
  bucket: string;
  [model: string]: number | string;
}

export interface ChartData {
  /** Sorted, stable list of model keys (one stacked series each). */
  models: string[];
  data: ChartDatum[];
}

/**
 * Flatten timeline buckets into recharts rows. Collect every model across all
 * buckets, then emit one datum per bucket with a value for *every* model (0 when
 * absent — required so the stacked areas line up). For `cost`, unpriced rows
 * (`cost_usd == null`) contribute 0; for `tokens` every row contributes.
 */
export function toChartData(buckets: UsageTimelineBucket[], metric: UsageMetric): ChartData {
  const models = [
    ...new Set(buckets.flatMap((b) => b.rows.map((r) => r.model || r.provider || r.agent || '—'))),
  ].sort();

  const data: ChartDatum[] = buckets.map((b) => {
    const datum: ChartDatum = { bucket: b.bucket };
    for (const m of models) datum[m] = 0;
    for (const r of b.rows) {
      const key = r.model || r.provider || r.agent || '—';
      const value = metric === 'cost' ? r.cost_usd ?? 0 : r.total_tokens;
      datum[key] = (datum[key] as number) + value;
    }
    return datum;
  });

  return { models, data };
}

const tooltipStyle: React.CSSProperties = {
  background: '#fff', border: '1px solid #e2e8f0', borderRadius: 6, fontSize: 11, padding: '6px 10px',
};

/** Short bucket tick: `2026-06-21` → `06-21`. */
function bucketTick(b: string): string {
  return b.length >= 10 ? b.slice(5) : b;
}

export default function UsageTimelineStacked({
  buckets,
  metric,
}: {
  buckets: UsageTimelineBucket[];
  metric: UsageMetric;
}) {
  const { models, data } = toChartData(buckets, metric);

  if (data.length === 0 || models.length === 0) {
    return (
      <div className="h-[240px] flex flex-col items-center justify-center text-center">
        <div className="w-10 h-10 rounded-full bg-slate-100 mb-2" />
        <p className="text-xs text-ink-mute">No usage recorded yet</p>
      </div>
    );
  }

  const colors = assignModelColors(models);
  const fmt = (v: number) => (metric === 'cost' ? formatCost(v) : formatTokens(v));

  return (
    <div>
      <div style={{ width: '100%', height: 240 }}>
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={data} margin={{ top: 8, right: 8, bottom: 0, left: 0 }}>
            <XAxis dataKey="bucket" tickFormatter={bucketTick} tick={{ fontSize: 10, fill: '#94a3b8' }} />
            <YAxis
              width={48}
              tick={{ fontSize: 10, fill: '#94a3b8' }}
              tickFormatter={(v) => fmt(typeof v === 'number' ? v : 0)}
            />
            <Tooltip
              contentStyle={tooltipStyle}
              formatter={(v, name) => [fmt(typeof v === 'number' ? v : 0), String(name)]}
            />
            {models.map((m) => (
              <Area
                key={m}
                type="monotone"
                stackId="u"
                dataKey={m}
                name={m}
                stroke={colors.get(m)}
                fill={colors.get(m)}
                fillOpacity={0.5}
                strokeWidth={1}
              />
            ))}
          </AreaChart>
        </ResponsiveContainer>
      </div>
      {/* Legend — models → colors (stable). */}
      <ul className="mt-3 flex flex-wrap gap-x-4 gap-y-1.5">
        {models.map((m) => (
          <li key={m} className="flex items-center gap-1.5 text-[11px]">
            <span className="w-2.5 h-2.5 rounded-sm shrink-0" style={{ background: colors.get(m) }} />
            <span className="text-ink-dim truncate max-w-[160px]">{m}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
