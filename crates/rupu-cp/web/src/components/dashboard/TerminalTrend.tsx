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
