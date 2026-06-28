// Project Runs tab body — paginated run list scoped to one project, with
// client-side status + trigger filter chips applied to the loaded rows.
//
// Ported from pages/ProjectRuns.tsx (fetch / load-more rendering), reshaped
// into a self-contained component keyed off the `wsId` prop. Rows render via
// the shared SortableTable.

import { useCallback, useEffect, useState } from 'react';
import { api, type RunListRow, type RunStatusStr } from '../../lib/api';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import SortableTable, { type Column } from '../lists/SortableTable';
import UsageBarChart from '../charts/UsageBarChart';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { shortId } from '../../lib/shortId';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';
import { cn } from '../../lib/cn';

const PAGE = 20;

// --- Filter definitions -----------------------------------------------------

type StatusFilter = 'all' | 'running' | 'completed' | 'failed';
type TriggerFilter = 'all' | 'manual' | 'event' | 'cron';

const STATUS_FILTERS: { id: StatusFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'running', label: 'Running' },
  { id: 'completed', label: 'Completed' },
  { id: 'failed', label: 'Failed' },
];

const TRIGGER_FILTERS: { id: TriggerFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'manual', label: 'Manual' },
  { id: 'event', label: 'Event' },
  { id: 'cron', label: 'Cron' },
];

// Status grouping mirrors the run-status enum in StatusPill:
//   running   → running | pending | awaiting_approval
//   completed → completed
//   failed    → failed | rejected
const STATUS_GROUP: Record<Exclude<StatusFilter, 'all'>, ReadonlySet<RunStatusStr>> = {
  running: new Set<RunStatusStr>(['running', 'pending', 'awaiting_approval']),
  completed: new Set<RunStatusStr>(['completed']),
  failed: new Set<RunStatusStr>(['failed', 'rejected']),
};

function matchesStatus(status: RunStatusStr, filter: StatusFilter): boolean {
  return filter === 'all' || STATUS_GROUP[filter].has(status);
}

/** Run duration in ms — explicit duration_ms, else derived from start/finish. */
function runDurationMs(run: RunListRow): number | null {
  if (run.duration_ms != null) return run.duration_ms;
  const start = Date.parse(run.started_at);
  if (Number.isNaN(start)) return null;
  const end = run.finished_at ? Date.parse(run.finished_at) : Date.now();
  if (Number.isNaN(end)) return null;
  return Math.max(0, end - start);
}

const RUN_COLUMNS: Column<RunListRow>[] = [
  {
    key: 'workflow',
    header: 'Workflow',
    sortable: true,
    sortValue: (r) => r.workflow_name,
    render: (r) => <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>,
  },
  {
    key: 'run',
    header: 'Run',
    render: (r) => <span className="text-note text-ink-mute font-mono">{shortId(r.id)}</span>,
  },
  {
    key: 'trigger',
    header: 'Trigger',
    sortable: true,
    sortValue: (r) => r.trigger,
    render: (r) => <TriggerChip trigger={r.trigger} />,
  },
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusPill status={r.status} />,
  },
  {
    key: 'in',
    header: 'In',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (r) => r.usage.input_tokens,
    render: (r) => <span className="text-ink-dim">{formatTokens(r.usage.input_tokens)}</span>,
  },
  {
    key: 'out',
    header: 'Out',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (r) => r.usage.output_tokens,
    render: (r) => <span className="text-ink-dim">{formatTokens(r.usage.output_tokens)}</span>,
  },
  {
    key: 'cached',
    header: 'Cached',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (r) => r.usage.cached_tokens,
    render: (r) =>
      r.usage.cached_tokens ? (
        <span className="text-ink-dim">{formatTokens(r.usage.cached_tokens)}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'cost',
    header: 'Cost',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (r) => r.usage.cost_usd,
    render: (r) => <span className="text-ink font-medium">{formatCost(r.usage.cost_usd)}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (r) => runDurationMs(r),
    render: (r) => (
      <span className="text-ink-dim">
        {r.duration_ms != null
          ? formatDuration(r.duration_ms)
          : durationBetween(r.started_at, r.finished_at)}
      </span>
    ),
  },
  {
    key: 'turns',
    header: 'Turns',
    align: 'right',
    width: 'w-16',
    sortable: true,
    sortValue: (r) => r.turns,
    render: (r) => <span className="text-ink">{r.turns ? String(r.turns) : '—'}</span>,
  },
  {
    key: 'started',
    header: 'Started',
    align: 'right',
    width: 'w-28',
    sortable: true,
    sortValue: (r) => (r.started_at ? Date.parse(r.started_at) : null),
    render: (r) => <span className="text-ink-mute">{relativeTime(r.started_at)}</span>,
  },
];

export default function ProjectRunsTab({ wsId }: { wsId: string }) {
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [triggerFilter, setTriggerFilter] = useState<TriggerFilter>('all');

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setRuns(null);
    setError(null);
    api
      .getProjectRuns(wsId, { limit: PAGE })
      .then((pageData) => {
        if (cancelled) return;
        setRuns(pageData);
        setHasMore(pageData.length >= PAGE);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load runs');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const loadMore = useCallback(async () => {
    if (!wsId) return;
    const current = runs ?? [];
    const next = await api.getProjectRuns(wsId, { offset: current.length, limit: PAGE });
    if (next.length === 0) {
      setHasMore(false);
      return;
    }
    setRuns([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  }, [wsId, runs]);

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Clicking the already-active chip returns it to "All".
  const toggleStatus = (id: StatusFilter) =>
    setStatusFilter((cur) => (cur === id ? 'all' : id));
  const toggleTrigger = (id: TriggerFilter) =>
    setTriggerFilter((cur) => (cur === id ? 'all' : id));

  const filtered = (runs ?? []).filter(
    (r) =>
      matchesStatus(r.status, statusFilter) &&
      (triggerFilter === 'all' || r.trigger === triggerFilter),
  );

  return (
    <div className="space-y-4">
      {/* Filter chip rows */}
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <span className="text-meta font-semibold uppercase tracking-widest text-ink-mute w-14">
            Status
          </span>
          {STATUS_FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              onClick={() => (f.id === 'all' ? setStatusFilter('all') : toggleStatus(f.id))}
              className={cn(
                'text-xs font-medium px-3 py-1 rounded-full border transition-colors',
                statusFilter === f.id
                  ? 'bg-brand-600 text-white border-brand-600'
                  : 'bg-panel text-ink-dim border-border hover:bg-surface-hover',
              )}
            >
              {f.label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <span className="text-meta font-semibold uppercase tracking-widest text-ink-mute w-14">
            Trigger
          </span>
          {TRIGGER_FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              onClick={() => (f.id === 'all' ? setTriggerFilter('all') : toggleTrigger(f.id))}
              className={cn(
                'text-xs font-medium px-3 py-1 rounded-full border transition-colors',
                triggerFilter === f.id
                  ? 'bg-brand-600 text-white border-brand-600'
                  : 'bg-panel text-ink-dim border-border hover:bg-surface-hover',
              )}
            >
              {f.label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {runs === null && !error && <div className="text-sm text-ink-dim">Loading runs…</div>}

      {runs !== null && filtered.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex items-center justify-center">
          <p className="text-sm text-ink-mute">
            {runs.length === 0 ? 'No runs for this project yet' : 'No runs match this filter'}
          </p>
        </div>
      )}

      {runs !== null && filtered.length > 0 && (
        <div className="space-y-4">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
            <UsageBarChart
              bars={filtered.map((r) => ({
                id: r.id,
                label: r.workflow_name,
                to: `/runs/${encodeURIComponent(r.id)}`,
                input_tokens: r.usage.input_tokens,
                output_tokens: r.usage.output_tokens,
                cached_tokens: r.usage.cached_tokens,
                cost_usd: r.usage.cost_usd,
              }))}
            />
          </div>
          <SortableTable<RunListRow>
            columns={RUN_COLUMNS}
            rows={filtered}
            rowKey={(r) => r.id}
            rowHref={(r) => `/runs/${encodeURIComponent(r.id)}`}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
        </div>
      )}

      {runs !== null && filtered.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
          {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${filtered.length} —`}
        </div>
      )}
    </div>
  );
}
