// OutlierPanel — runs that cost far more than their workflow normally does.
//
// Baseline is per-workflow and median-based (`rupu-cp/src/api/usage_outliers.rs`):
// an absolute threshold would flag an expensive-by-design workflow forever and
// never flag a cheap one that regressed 10x. Unpriced runs never appear here —
// `cost_usd: None` on the wire means "unknown", not "free", and the backend
// excludes them from both the baseline and the results.
//
// `excludedRunIds`/`onToggleRun` (Task U3, the interactive `/usage` page) add
// a per-row exclude checkbox that toggles the run's `run_id` in the caller's
// `TimelineFilter.excludedRunIds` — this is how a real ~1000x-cost outlier
// gets pulled out of the spend graph so the axis rescales. Both optional;
// omitting `onToggleRun` renders the panel exactly as before (no checkbox).

import { Link } from 'react-router-dom';
import type { OutlierRun } from '../../lib/api';

export type { OutlierRun };

export function OutlierPanel({
  outliers,
  excludedRunIds,
  onToggleRun,
}: {
  outliers: OutlierRun[];
  /** `run_id`s currently excluded from the graph. Only read when `onToggleRun` is set. */
  excludedRunIds?: Set<string>;
  /** Called with a row's `run_id` when its exclude toggle is clicked. Omit to render read-only. */
  onToggleRun?: (runId: string) => void;
}) {
  if (outliers.length === 0) {
    return (
      <div className="p-4 text-sm text-[rgb(var(--c-ink-mute))]">
        No cost outliers in this window
      </div>
    );
  }
  return (
    <ul className="divide-y divide-[rgb(var(--c-border))]">
      {outliers.map((o) => {
        const excluded = !!excludedRunIds?.has(o.run_id);
        return (
          <li key={o.run_id} className="flex items-center gap-3 px-3 py-2 text-sm">
            {onToggleRun && (
              <input
                type="checkbox"
                aria-label={o.run_id}
                checked={!excluded}
                onChange={() => onToggleRun(o.run_id)}
              />
            )}
            <Link
              to={`/runs/${o.run_id}`}
              className={`font-medium text-[rgb(var(--c-ink))] ${excluded ? 'line-through opacity-50' : ''}`}
            >
              {o.workflow_name}
            </Link>
            <span className={`text-xs text-[rgb(var(--c-ink-mute))] ${excluded ? 'opacity-50' : ''}`}>
              {o.run_id}
            </span>
            <span className={`ml-auto tabular-nums text-[rgb(var(--c-ink))] ${excluded ? 'opacity-50' : ''}`}>
              ${o.cost_usd.toFixed(2)}
            </span>
            <span
              className={`tabular-nums text-[rgb(var(--c-status-failed))] ${excluded ? 'opacity-50' : ''}`}
            >
              {o.ratio.toFixed(1)}× baseline (${o.baseline_usd.toFixed(2)})
            </span>
          </li>
        );
      })}
    </ul>
  );
}
