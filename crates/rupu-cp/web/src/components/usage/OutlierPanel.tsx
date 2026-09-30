// OutlierPanel — runs that cost far more than their workflow normally does.
//
// Baseline is per-workflow and median-based (`rupu-cp/src/api/usage_outliers.rs`):
// an absolute threshold would flag an expensive-by-design workflow forever and
// never flag a cheap one that regressed 10x. Unpriced runs never appear here —
// `cost_usd: None` on the wire means "unknown", not "free", and the backend
// excludes them from both the baseline and the results.
//
// Standalone agent runs and session turns are outlier candidates too, baselined
// per agent: they carry `workflow_name: ""` plus `kind` (`"agent"`/`"session"`)
// and `agent`, so the name column falls back to the agent and a small kind tag
// says what sort of row it is. Only workflow runs live in the run store, so
// the name links by kind: workflow -> `/runs/:id`, session -> its session page,
// agent -> its transcript view (the same `/transcript?path=` route
// `AgentRuns` uses); a row without its target renders as plain text rather
// than a link that would 404.
//
// `excludedRunIds`/`onToggleRun` (Task U3, the interactive `/usage` page) add
// a per-row exclude checkbox that toggles the run's `run_id` in the caller's
// `TimelineFilter.excludedRunIds` — this is how a real ~1000x-cost outlier
// gets pulled out of the spend graph so the axis rescales. Both optional;
// omitting `onToggleRun` renders the panel exactly as before (no checkbox).

import { Link } from 'react-router-dom';
import type { OutlierRun } from '../../lib/api';

export type { OutlierRun };

/** Where an outlier row's name links, or `null` when the server didn't send
 *  the target for its kind (older API) — then it renders as plain text. */
function outlierHref(o: OutlierRun): string | null {
  switch (o.kind ?? 'workflow') {
    case 'session':
      return o.session_id ? `/sessions/${encodeURIComponent(o.session_id)}` : null;
    case 'agent':
      // The outliers endpoint is local-only, so no `&host=`; outliers are
      // completed history, so `live=0`.
      return o.transcript_path
        ? `/transcript?path=${encodeURIComponent(o.transcript_path)}&live=0`
        : null;
    default:
      return `/runs/${o.run_id}`;
  }
}

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
      <div className="p-4 text-sm text-ink-mute">
        No cost outliers in this window
      </div>
    );
  }
  return (
    <ul className="divide-y divide-border">
      {outliers.map((o) => {
        const excluded = !!excludedRunIds?.has(o.run_id);
        // `workflow_name` is `""` for standalone/session outliers — name the
        // agent instead. `kind` is absent on older servers (a workflow run).
        const name = o.workflow_name || o.agent || o.run_id;
        const kind = o.kind ?? 'workflow';
        const href = outlierHref(o);
        const nameClass = `font-medium text-ink ${excluded ? 'line-through opacity-50' : ''}`;
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
            {href ? (
              <Link to={href} className={nameClass}>
                {name}
              </Link>
            ) : (
              <span className={nameClass}>{name}</span>
            )}
            {kind !== 'workflow' && (
              <span
                className={`rounded border border-border px-1 py-px text-[10px] uppercase tracking-wide text-ink-mute ${excluded ? 'opacity-50' : ''}`}
                title={kind === 'session' ? 'A session turn' : 'A standalone agent run'}
              >
                {kind}
              </span>
            )}
            <span className={`text-xs text-ink-mute ${excluded ? 'opacity-50' : ''}`}>
              {o.run_id}
            </span>
            <span className={`ml-auto tabular-nums text-ink ${excluded ? 'opacity-50' : ''}`}>
              ${o.cost_usd.toFixed(2)}
            </span>
            <span
              className={`tabular-nums text-status-failed ${excluded ? 'opacity-50' : ''}`}
            >
              {o.ratio.toFixed(1)}× baseline (${o.baseline_usd.toFixed(2)})
            </span>
          </li>
        );
      })}
    </ul>
  );
}
