import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts';
import type { TurnUsagePoint } from '../transcript/turnSeries';
import { formatTokens } from '../../lib/usage';

const tooltipStyle: React.CSSProperties = {
  background: '#fff', border: '1px solid #e2e8f0', borderRadius: 6, fontSize: 11, padding: '6px 10px',
};

/** Per-turn token timeline (in/out/cached stacked) for the run-detail page. */
export default function RunUsageTimeline({ series }: { series: TurnUsagePoint[] }) {
  if (series.length === 0) {
    return <div className="text-xs text-ink-mute py-6 text-center">No per-turn usage yet</div>;
  }
  return (
    <div style={{ width: '100%', height: 120 }}>
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={series} margin={{ top: 4, right: 8, bottom: 0, left: 0 }}>
          <XAxis dataKey="turn" tick={{ fontSize: 10, fill: '#94a3b8' }} />
          <YAxis tick={{ fontSize: 10, fill: '#94a3b8' }} width={36}
            tickFormatter={(v) => formatTokens(typeof v === 'number' ? v : 0)} />
          <Tooltip contentStyle={tooltipStyle}
            formatter={(v, name) => [formatTokens(typeof v === 'number' ? v : 0), String(name)]}
            labelFormatter={(l) => `turn ${l}`} />
          <Area type="monotone" dataKey="tokens_in" name="in" stackId="1" stroke="#1860f2" fill="#1860f2" fillOpacity={0.18} />
          <Area type="monotone" dataKey="tokens_out" name="out" stackId="1" stroke="#22c55e" fill="#22c55e" fillOpacity={0.18} />
          <Area type="monotone" dataKey="tokens_cached" name="cached" stackId="1" stroke="#f59e0b" fill="#f59e0b" fillOpacity={0.18} />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}
