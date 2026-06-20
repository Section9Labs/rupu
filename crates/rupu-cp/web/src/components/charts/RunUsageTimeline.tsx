import { Area, AreaChart, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import type { UsageTimelinePoint } from '../../lib/api';
import { formatTokens } from '../../lib/usage';

const tooltipStyle: React.CSSProperties = {
  background: '#fff', border: '1px solid #e2e8f0', borderRadius: 6, fontSize: 11, padding: '6px 10px',
};

/** Per-turn token timeline (in/out/cached stacked) across a run's steps (or a
 *  session's runs). With `separators`, a dashed line marks each step boundary. */
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
  // First turn of each new label group (skip index 0) — the step boundaries.
  const boundaries = separators
    ? series.filter((p, i) => i > 0 && p.label !== series[i - 1].label)
    : [];
  return (
    <div style={{ width: '100%', height: 140 }}>
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={series} margin={{ top: 12, right: 8, bottom: 0, left: 0 }}>
          <XAxis dataKey="turn" tick={{ fontSize: 10, fill: '#94a3b8' }} />
          <YAxis tick={{ fontSize: 10, fill: '#94a3b8' }} width={36}
            tickFormatter={(v) => formatTokens(typeof v === 'number' ? v : 0)} />
          <Tooltip contentStyle={tooltipStyle}
            formatter={(v, name) => [formatTokens(typeof v === 'number' ? v : 0), String(name)]}
            labelFormatter={(l, payload) => {
              const p = payload?.[0]?.payload as UsageTimelinePoint | undefined;
              return p?.label ? `turn ${l} · ${p.label}` : `turn ${l}`;
            }} />
          {boundaries.map((b) => (
            <ReferenceLine key={`${b.label}-${b.turn}`} x={b.turn} stroke="#cbd5e1" strokeDasharray="3 3"
              label={{ value: b.label, position: 'top', fontSize: 9, fill: '#94a3b8' }} />
          ))}
          <Area type="monotone" dataKey="tokens_in" name="in" stackId="1" stroke="#1860f2" fill="#1860f2" fillOpacity={0.18} />
          <Area type="monotone" dataKey="tokens_out" name="out" stackId="1" stroke="#22c55e" fill="#22c55e" fillOpacity={0.18} />
          <Area type="monotone" dataKey="tokens_cached" name="cached" stackId="1" stroke="#f59e0b" fill="#f59e0b" fillOpacity={0.18} />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}
