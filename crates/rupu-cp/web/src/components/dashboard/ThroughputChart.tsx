// ThroughputChart — runs started per bucket, stacked by trigger (spec §5.5).
//
// Replaces the cycle-grouped activity feed. The feed answered "autoflow is
// taking over the runs" by folding the noise into rows; this answers it as a
// SHAPE — "cron is 90% of today's volume" reads at a glance, with nothing to
// scan or expand.
//
// Colors are locked to `TriggerChip`'s palette (manual=neutral, cron=violet,
// event=sky) so this chart and any trigger chip elsewhere agree WITHOUT a
// legend. `brand.600` is an exact channel match for Tailwind's `violet-700`
// (the chip's cron text color) — see the inline note below. There is no
// dedicated "sky" token in the palette; `info` (blue) is the closest existing
// token and is kept distinct from `status.running` so this chart's "event"
// band is never confused with the live "Running" count elsewhere.

import { ResponsiveContainer, AreaChart, Area, XAxis, YAxis, Tooltip, CartesianGrid } from 'recharts';
import { useThemeColors } from '../../lib/useThemeColors';
import type { ThroughputBucket } from '../../lib/api';

export function ThroughputChart({ buckets }: { buckets: ThroughputBucket[] }) {
  const colors = useThemeColors();

  if (buckets.length === 0) {
    return (
      <div className="flex h-48 items-center justify-center text-sm text-[rgb(var(--c-ink-mute))]">
        No runs in this range
      </div>
    );
  }

  const data = buckets.map((b) => ({
    ts: new Date(b.ts).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }),
    manual: b.manual,
    cron: b.cron,
    event: b.event,
  }));

  const series = [
    // neutral — matches the TriggerChip "manual" tone (bg-surface text-ink).
    { key: 'manual', color: colors.inkDim },
    // violet — `brand.600` = channels `109 40 217`, an exact match for
    // Tailwind's `violet-700` (the chip's cron text color).
    { key: 'cron', color: colors.get('brand.600') },
    // sky (closest available token — see module doc comment).
    { key: 'event', color: colors.info },
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
              // Consistent with TerminalTrend: no animation, liveness is
              // per-transport.
              isAnimationActive={false}
            />
          ))}
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}

export default ThroughputChart;
