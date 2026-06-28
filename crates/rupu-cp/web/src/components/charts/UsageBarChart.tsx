import { Bar, BarChart, Cell, ResponsiveContainer, Tooltip, XAxis } from 'recharts';
import { useNavigate } from 'react-router-dom';
import { formatTokens, formatCost } from '../../lib/usage';
import { useThemeColors } from '../../lib/useThemeColors';

export interface UsageBar {
  id: string;
  label: string;
  input_tokens: number;
  output_tokens: number;
  cached_tokens: number;
  cost_usd: number | null;
  to?: string;
}

/** Per-run stacked token bars (in/out/cached) summarising the loaded list. */
export default function UsageBarChart({ bars }: { bars: UsageBar[] }) {
  const navigate = useNavigate();
  const colors = useThemeColors();
  const tooltipStyle: React.CSSProperties = {
    background: colors.panel,
    border: `1px solid ${colors.border}`,
    color: colors.ink,
    borderRadius: 6,
    fontSize: 11,
    padding: '6px 10px',
  };
  const total = bars.reduce((a, b) => a + b.input_tokens + b.output_tokens + b.cached_tokens, 0);
  if (bars.length === 0 || total === 0) {
    return <div className="text-xs text-ink-mute py-6 text-center">No token usage in this list yet</div>;
  }
  return (
    <div style={{ width: '100%', height: 96 }}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={bars} margin={{ top: 4, right: 4, bottom: 0, left: 0 }} barCategoryGap={2}>
          <XAxis dataKey="label" hide />
          <Tooltip
            contentStyle={tooltipStyle}
            formatter={(v, name) => [formatTokens(typeof v === 'number' ? v : 0), String(name)]}
            labelFormatter={(_l, payload) => {
              const p = payload?.[0]?.payload as UsageBar | undefined;
              return p ? `${p.label} · ${formatCost(p.cost_usd)}` : '';
            }}
          />
          <Bar dataKey="input_tokens" name="in" stackId="t" fill={colors.status.running}
            onClick={(d) => { const b = (d as { payload?: UsageBar })?.payload; if (b?.to) navigate(b.to); }}>
            {bars.map((b) => <Cell key={b.id} cursor={b.to ? 'pointer' : 'default'} />)}
          </Bar>
          <Bar dataKey="output_tokens" name="out" stackId="t" fill={colors.status.done} />
          <Bar dataKey="cached_tokens" name="cached" stackId="t" fill={colors.status.awaiting} />
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}
