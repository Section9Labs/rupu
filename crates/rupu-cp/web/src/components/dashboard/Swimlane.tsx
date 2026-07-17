// Swimlane — the live activity hero.
//
// The status tiles tell you HOW MANY; this tells you WHAT IS HAPPENING. A run
// executing 40x longer than its median is visually obvious here in a way no
// table makes it — that is why it earns the hero slot.
//
// Hand-rolled SVG: recharts has no Gantt. Bars DO NOT animate — they redraw on
// data. Local bars update sub-second via SSE while SSH bars step forward on the
// poll tick, and a smoothly-animating bar beside one jumping in 10s increments
// reads as broken.

import { useMemo, useState } from 'react';
import { assignLanes, type LaneKey } from '../../lib/dashboard/swimlane';
import { useThemeColors } from '../../lib/useThemeColors';
import type { ActiveRunBar } from '../../lib/api';

const ROW_H = 22;
const BAR_H = 10;

function colorFor(status: string, colors: ReturnType<typeof useThemeColors>): string {
  switch (status) {
    case 'awaiting_approval':
      return colors.get('status.awaiting');
    case 'paused':
      return colors.get('status.paused');
    case 'failed':
      return colors.get('status.failed');
    case 'pending':
      return colors.get('status.pending');
    default:
      return colors.get('status.running');
  }
}

export function Swimlane({
  bars,
  onSelect,
}: {
  bars: ActiveRunBar[];
  onSelect?: (runId: string) => void;
}) {
  const colors = useThemeColors();
  const [groupBy, setGroupBy] = useState<LaneKey>('workflow');

  // `now` is captured per render rather than ticked: bars redraw on data
  // arrival, not on a timer.
  const lanes = useMemo(() => assignLanes(bars, groupBy, Date.now()), [bars, groupBy]);

  if (bars.length === 0) {
    return (
      <div className="flex h-32 items-center justify-center text-sm text-[rgb(var(--c-ink-mute))]">
        Nothing running right now
      </div>
    );
  }

  return (
    <div>
      <div className="mb-2 flex items-center justify-end gap-1 text-xs">
        {(['workflow', 'host'] as LaneKey[]).map((k) => (
          <button
            key={k}
            onClick={() => setGroupBy(k)}
            className={`rounded px-2 py-0.5 ${
              groupBy === k
                ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
                : 'text-[rgb(var(--c-ink-mute))]'
            }`}
          >
            by {k}
          </button>
        ))}
      </div>
      <svg width="100%" height={lanes.length * ROW_H} role="img" aria-label="Active runs over time">
        {lanes.map((lane, i) => (
          <g key={lane.key} transform={`translate(0, ${i * ROW_H})`}>
            <text
              x={0}
              y={ROW_H / 2 + 4}
              fontSize={11}
              fill={colors.inkDim}
              className="select-none"
            >
              {lane.key}
            </text>
            {lane.bars.map((pb) => (
              <rect
                key={pb.bar.run_id}
                x={`${20 + pb.x0 * 78}%`}
                y={(ROW_H - BAR_H) / 2}
                width={`${Math.max(0.5, (pb.x1 - pb.x0) * 78)}%`}
                height={BAR_H}
                rx={2}
                fill={colorFor(pb.bar.status, colors)}
                onClick={() => onSelect?.(pb.bar.run_id)}
                style={{ cursor: onSelect ? 'pointer' : undefined }}
              >
                <title>{`${pb.bar.workflow_name} · ${pb.bar.status} · started ${new Date(
                  pb.bar.started_at,
                ).toLocaleTimeString()}`}</title>
              </rect>
            ))}
          </g>
        ))}
      </svg>
    </div>
  );
}
