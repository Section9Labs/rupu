import { Area, ComposedChart, Line, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import type { UsageTimelinePoint } from '../../lib/api';
import { formatTokens } from '../../lib/usage';

const tooltipStyle: React.CSSProperties = {
  background: '#fff', border: '1px solid #e2e8f0', borderRadius: 6, fontSize: 11, padding: '6px 10px',
};

const COLOR_IN = '#1860f2';
const COLOR_OUT = '#22c55e';
const COLOR_CACHED = '#f59e0b';

/** Chart datum for the diverging timeline. `out` is NEGATED for display (renders
 *  below the zero baseline); the original positive `tokens_out` is kept for the
 *  tooltip. `in` and `cached` stay positive. */
export interface UsageChartPoint extends UsageTimelinePoint {
  in: number;
  out: number;
  cached: number;
}

/** Abbreviated ABSOLUTE magnitude tick label (sign stripped): `-1500 → "1.5k"`,
 *  `2000 → "2k"`, `500 → "500"`, `0 → "0"`. */
export function formatAbsTick(v: number): string {
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

/** Map a timeline point to its chart datum, negating `out` for the diverging
 *  layout while keeping `in`/`cached` positive (and the originals for tooltips). */
export function toChartPoint(p: UsageTimelinePoint): UsageChartPoint {
  return { ...p, in: p.tokens_in, out: -p.tokens_out, cached: p.tokens_cached };
}

/** Per-turn token timeline across a run's steps (or a session's runs), drawn as a
 *  diverging chart: input above the zero baseline, output mirrored below it (same
 *  left axis, magnitudes compare directly), and cached as a line on its own right
 *  axis. With `separators`, a dashed line marks each step boundary. */
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
  const data = series.map(toChartPoint);
  // First turn of each new label group (skip index 0) — the step boundaries.
  const boundaries = separators
    ? series.filter((p, i) => i > 0 && p.label !== series[i - 1].label)
    : [];
  return (
    <div style={{ width: '100%', height: 140 }}>
      <ResponsiveContainer width="100%" height="100%">
        <ComposedChart data={data} margin={{ top: 12, right: 8, bottom: 0, left: 0 }}>
          <XAxis dataKey="turn" tick={{ fontSize: 10, fill: '#94a3b8' }} />
          <YAxis yAxisId="io" tick={{ fontSize: 10, fill: '#94a3b8' }} width={36}
            tickFormatter={(v) => formatAbsTick(typeof v === 'number' ? v : 0)} />
          <YAxis yAxisId="cache" orientation="right" tick={{ fontSize: 10, fill: COLOR_CACHED }} width={36}
            tickFormatter={(v) => formatAbsTick(typeof v === 'number' ? v : 0)} />
          <Tooltip contentStyle={tooltipStyle}
            formatter={(v, name) => {
              const raw = typeof v === 'number' ? v : 0;
              // `out` is negated for display — show its true magnitude.
              return [formatTokens(Math.abs(raw)), String(name)];
            }}
            labelFormatter={(l, payload) => {
              const p = payload?.[0]?.payload as UsageChartPoint | undefined;
              return p?.label ? `turn ${l} · ${p.label}` : `turn ${l}`;
            }} />
          <ReferenceLine yAxisId="io" y={0} stroke="#94a3b8" />
          {boundaries.map((b) => (
            <ReferenceLine key={`${b.label}-${b.turn}`} yAxisId="io" x={b.turn} stroke="#cbd5e1" strokeDasharray="3 3"
              label={{ value: b.label, position: 'top', fontSize: 9, fill: '#94a3b8' }} />
          ))}
          <Area yAxisId="io" type="monotone" dataKey="in" name="In" stroke={COLOR_IN} fill={COLOR_IN} fillOpacity={0.18} />
          <Area yAxisId="io" type="monotone" dataKey="out" name="Out" stroke={COLOR_OUT} fill={COLOR_OUT} fillOpacity={0.18} />
          <Line yAxisId="cache" type="monotone" dataKey="cached" name="Cached" stroke={COLOR_CACHED} dot={false} strokeWidth={1.5} />
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}
