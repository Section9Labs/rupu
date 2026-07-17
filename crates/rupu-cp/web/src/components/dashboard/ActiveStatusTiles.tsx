// ActiveStatusTiles — the live half of the status split.
//
// Replaces the old donut's job of answering "is anything stuck right now". The
// donut could not: it rendered by_status seeded with all 8 variants at zero, so
// it was an 8-slice pie that was ~95% completed — a ratio answering "what
// fraction of all runs ever succeeded", which nobody asks.
//
// AwaitingApproval and Paused get weight because they are the ONLY states where
// the system is blocked on the operator.

import type { ActiveCounts } from '../../lib/api';

interface Tile {
  key: keyof ActiveCounts;
  label: string;
  cssVar: string;
  /** Blocked on the operator — rendered with weight. */
  needsYou: boolean;
}

const TILES: Tile[] = [
  { key: 'running', label: 'Running', cssVar: '--c-status-running', needsYou: false },
  { key: 'awaiting_approval', label: 'Awaiting you', cssVar: '--c-status-awaiting', needsYou: true },
  { key: 'paused', label: 'Paused', cssVar: '--c-status-paused', needsYou: true },
  { key: 'pending', label: 'Pending', cssVar: '--c-status-pending', needsYou: false },
];

export function ActiveStatusTiles({ active }: { active: ActiveCounts }) {
  const total = TILES.reduce((s, t) => s + active[t.key], 0);

  return (
    <div className="grid grid-cols-4 gap-3">
      {TILES.map((t) => {
        const value = active[t.key];
        const pct = total > 0 ? (value / total) * 100 : 0;
        return (
          <div
            key={t.key}
            className={`rounded-lg border p-3 ${
              t.needsYou && value > 0
                ? 'border-[rgb(var(--c-status-awaiting))] bg-[rgb(var(--c-surface))]'
                : 'border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))]'
            }`}
          >
            <div className="text-xs text-[rgb(var(--c-ink-dim))]">{t.label}</div>
            <div
              className={`mt-1 tabular-nums ${
                t.needsYou && value > 0 ? 'text-3xl font-semibold' : 'text-2xl'
              } text-[rgb(var(--c-ink))]`}
            >
              {value}
            </div>
            {/* Segmented bar, color-locked to TerminalTrend's palette so the
                eye ties the live count to the history without a legend. */}
            <div className="mt-2 h-[1.5px] w-full bg-[rgb(var(--c-border))]">
              <div
                className="h-full"
                style={{ width: `${pct}%`, backgroundColor: `rgb(var(${t.cssVar}))` }}
              />
            </div>
          </div>
        );
      })}
    </div>
  );
}
