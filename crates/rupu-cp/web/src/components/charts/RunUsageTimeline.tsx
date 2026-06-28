import { Area, ComposedChart, Line, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import type { UsageTimelinePoint } from '../../lib/api';
import { formatTokens } from '../../lib/usage';
import { useThemeColors } from '../../lib/useThemeColors';

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
 *  them. With `separators`, a dashed line marks each step boundary. */
export default function RunUsageTimeline({
  series,
  separators = false,
}: {
  series: UsageTimelinePoint[];
  separators?: boolean;
}) {
  const colors = useThemeColors();
  const COLOR_IN = colors.status.running;
  const COLOR_OUT = colors.status.done;
  const COLOR_CACHED = colors.status.awaiting;
  const tooltipStyle: React.CSSProperties = {
    background: colors.panel,
    border: `1px solid ${colors.border}`,
    color: colors.ink,
    borderRadius: 6,
    fontSize: 11,
    padding: '6px 10px',
  };
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
          <XAxis dataKey="turn" tick={{ fontSize: 10, fill: colors.inkMute }} />
          {/* Left axis: input only (its own large scale). */}
          <YAxis yAxisId="in" tick={{ fontSize: 10, fill: COLOR_IN }} width={40}
            tickFormatter={(v) => formatTokenTick(typeof v === 'number' ? v : 0)} />
          {/* Right axis: output + cached, scaled to their own range. */}
          <YAxis yAxisId="oc" orientation="right" tick={{ fontSize: 10, fill: colors.inkMute }} width={40}
            tickFormatter={(v) => formatTokenTick(typeof v === 'number' ? v : 0)} />
          <Tooltip contentStyle={tooltipStyle}
            formatter={(v, name) => {
              const raw = typeof v === 'number' ? v : 0;
              return [formatTokens(raw), String(name)];
            }}
            labelFormatter={(l, payload) => {
              const p = payload?.[0]?.payload as UsageChartPoint | undefined;
              return p?.label ? `turn ${l} · ${p.label}` : `turn ${l}`;
            }} />
          {boundaries.map((b) => (
            <ReferenceLine key={`${b.label}-${b.turn}`} yAxisId="in" x={b.turn} stroke={colors.border} strokeDasharray="3 3"
              label={{ value: b.label, position: 'top', fontSize: 9, fill: colors.inkMute }} />
          ))}
          <Area yAxisId="in" type="monotone" dataKey="in" name="In" stroke={COLOR_IN} fill={COLOR_IN} fillOpacity={0.18} />
          <Line yAxisId="oc" type="monotone" dataKey="out" name="Out" stroke={COLOR_OUT} dot={false} strokeWidth={1.5} />
          <Line yAxisId="oc" type="monotone" dataKey="cached" name="Cached" stroke={COLOR_CACHED} dot={false} strokeWidth={1.5} />
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}
