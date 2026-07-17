// OutlierPanel — runs that cost far more than their workflow normally does.
//
// Baseline is per-workflow and median-based (`rupu-cp/src/api/usage_outliers.rs`):
// an absolute threshold would flag an expensive-by-design workflow forever and
// never flag a cheap one that regressed 10x. Unpriced runs never appear here —
// `cost_usd: None` on the wire means "unknown", not "free", and the backend
// excludes them from both the baseline and the results.

import { Link } from 'react-router-dom';
import type { OutlierRun } from '../../lib/api';

export type { OutlierRun };

export function OutlierPanel({ outliers }: { outliers: OutlierRun[] }) {
  if (outliers.length === 0) {
    return (
      <div className="p-4 text-sm text-[rgb(var(--c-ink-mute))]">
        No cost outliers in this window
      </div>
    );
  }
  return (
    <ul className="divide-y divide-[rgb(var(--c-border))]">
      {outliers.map((o) => (
        <li key={o.run_id} className="flex items-center gap-3 px-3 py-2 text-sm">
          <Link to={`/runs/${o.run_id}`} className="font-medium text-[rgb(var(--c-ink))]">
            {o.workflow_name}
          </Link>
          <span className="text-xs text-[rgb(var(--c-ink-mute))]">{o.run_id}</span>
          <span className="ml-auto tabular-nums text-[rgb(var(--c-ink))]">
            ${o.cost_usd.toFixed(2)}
          </span>
          <span className="tabular-nums text-[rgb(var(--c-status-failed))]">
            {o.ratio.toFixed(1)}× baseline (${o.baseline_usd.toFixed(2)})
          </span>
        </li>
      ))}
    </ul>
  );
}
