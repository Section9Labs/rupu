// ActivityFeed — the run feed, grouped by autoflow cycle.
//
// The problem: a chatty autoflow emitting twelve runs consumed the entire
// Recent Runs list (hard-capped at 10 rows server-side), and the rows carried
// no trigger so they could not even be told apart from operator-launched ones.
//
// One row per cycle, expandable. Manual runs always individual. Inside an
// expanded cycle, clean runs fold behind a `+N clean` pill: hidden, never lost.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { buildFeed, isCycleInteresting, foldCleanRuns } from '../../lib/dashboard/feed';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import type { CycleRollup, DashboardRecentRun } from '../../lib/api';

function CycleRow({ cycle }: { cycle: CycleRollup }) {
  const [open, setOpen] = useState(() => isCycleInteresting(cycle));
  // Clean runs fold behind a pill; `showClean` un-folds them. Hidden, never
  // lost — the count is always visible and always clickable.
  const [showClean, setShowClean] = useState(false);
  // ran/failed are null when the host does not report the breakdown (SSH).
  // Show what we know; never render a computed 0 from unknown inputs.
  const ok =
    cycle.ran !== null && cycle.failed !== null ? Math.max(0, cycle.ran - cycle.failed) : null;
  const { shown, cleanCount } = foldCleanRuns(cycle.runs);
  const visible = showClean ? cycle.runs : shown;

  return (
    <li className="border-b border-[rgb(var(--c-border))] px-3 py-2 last:border-0">
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-2 text-left text-sm"
      >
        <span className="text-[rgb(var(--c-ink-mute))]">{open ? '▾' : '▸'}</span>
        <span className="font-medium text-[rgb(var(--c-ink))]">
          {cycle.worker_name ?? cycle.cycle_id}
        </span>
        <span className="text-[rgb(var(--c-ink-dim))]">
          {/* Fall back to the runs list when the host omits the breakdown —
              it is the one count we always have. */}
          {cycle.ran ?? cycle.runs.length} runs
          {ok !== null && <> · {ok} ok</>}
          {cycle.failed !== null && cycle.failed > 0 && (
            <span className="text-[rgb(var(--c-status-failed))]">, {cycle.failed} failed</span>
          )}
          {cycle.skipped !== null && cycle.skipped > 0 && <span>, {cycle.skipped} skipped</span>}
        </span>
        <span className="ml-auto flex items-center gap-2">
          <TriggerChip trigger="cron" />
          {cycle.host_id && (
            <span className="text-xs text-[rgb(var(--c-ink-mute))]">{cycle.host_id}</span>
          )}
        </span>
      </button>
      {open && (
        // Plain divs, NOT a nested <ul>/<li>: the expanded run detail is not
        // itself another row in the feed list — the feed's "one row per
        // cycle" contract is about the outer <li> above. A nested list here
        // would inflate `getAllByRole('listitem')` counts for something that
        // is detail, not a feed entry.
        <div className="mt-2 space-y-1 pl-6">
          {visible.map((run) => (
            <div key={run.run_id} className="flex items-center gap-2">
              <Link
                to={`/runs/${run.run_id}`}
                className="text-xs text-[rgb(var(--c-ink-dim))] hover:text-[rgb(var(--c-ink))]"
              >
                {run.run_id}
              </Link>
              {/* StatusPill's prop is RunStatusStr, which does not include
                  'unknown' — an unresolved run gets a plain label rather than
                  a pill lying about a status we do not have. */}
              {run.status === 'unknown' ? (
                <span className="text-xs text-[rgb(var(--c-ink-mute))]">unresolved</span>
              ) : (
                <StatusPill status={run.status} />
              )}
            </div>
          ))}
          {cleanCount > 0 && !showClean && (
            <div>
              <button
                onClick={() => setShowClean(true)}
                className="rounded-full bg-[rgb(var(--c-surface))] px-2 py-0.5 text-xs text-[rgb(var(--c-ink-mute))] hover:text-[rgb(var(--c-ink))]"
              >
                +{cleanCount} clean
              </button>
            </div>
          )}
        </div>
      )}
    </li>
  );
}

export function ActivityFeed({
  cycles,
  recentManual,
}: {
  cycles: CycleRollup[];
  recentManual: DashboardRecentRun[];
}) {
  const rows = buildFeed(cycles, recentManual);

  if (rows.length === 0) {
    return (
      <div className="p-6 text-center text-sm text-[rgb(var(--c-ink-mute))]">No activity yet</div>
    );
  }

  return (
    <ul className="divide-y divide-[rgb(var(--c-border))]">
      {rows.map((row) =>
        row.kind === 'cycle' ? (
          <CycleRow key={row.cycle.cycle_id} cycle={row.cycle} />
        ) : (
          <li key={row.run.id} className="px-3 py-2">
            <Link to={`/runs/${row.run.id}`} className="flex items-center gap-2 text-sm">
              <span className="font-medium text-[rgb(var(--c-ink))]">{row.run.workflow_name}</span>
              <span className="text-xs text-[rgb(var(--c-ink-mute))]">{row.run.id}</span>
              <span className="ml-auto flex items-center gap-2">
                <TriggerChip trigger={row.run.trigger} />
                {row.run.host_id && (
                  <span className="text-xs text-[rgb(var(--c-ink-mute))]">{row.run.host_id}</span>
                )}
              </span>
            </Link>
          </li>
        ),
      )}
    </ul>
  );
}
