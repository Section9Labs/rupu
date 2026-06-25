// Project Runs tab body — paginated run list scoped to one project, with
// client-side status + trigger filter chips applied to the loaded rows.
//
// Ported from pages/ProjectRuns.tsx (fetch / load-more / MetricRow rendering),
// reshaped into a self-contained component keyed off the `wsId` prop.

import { useCallback, useEffect, useState } from 'react';
import { api, type RunListRow, type RunStatusStr } from '../../lib/api';
import { ListCard } from '../lists/ListCard';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import MetricRow from '../lists/MetricRow';
import UsageBarChart from '../charts/UsageBarChart';
import { durationBetween } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';
import { cn } from '../../lib/cn';

const PAGE = 20;

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

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
          <span className="text-[10px] font-semibold uppercase tracking-widest text-ink-mute w-14">
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
                  : 'bg-panel text-ink-dim border-border hover:bg-slate-100',
              )}
            >
              {f.label}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[10px] font-semibold uppercase tracking-widest text-ink-mute w-14">
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
                  : 'bg-panel text-ink-dim border-border hover:bg-slate-100',
              )}
            >
              {f.label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
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
          <ListCard>
            {filtered.map((r) => (
              <MetricRow
                key={r.id}
                to={`/runs/${encodeURIComponent(r.id)}`}
                header={
                  <>
                    <span className="text-sm font-medium text-ink truncate">{r.workflow_name}</span>
                    <span className="text-[11px] text-ink-mute font-mono">{shortId(r.id)}</span>
                    <TriggerChip trigger={r.trigger} />
                  </>
                }
                trailing={<StatusPill status={r.status} />}
                metrics={[
                  { label: 'in', value: formatTokens(r.usage.input_tokens) },
                  { label: 'out', value: formatTokens(r.usage.output_tokens) },
                  {
                    label: 'cached',
                    value: r.usage.cached_tokens ? formatTokens(r.usage.cached_tokens) : null,
                  },
                  { label: 'cost', value: formatCost(r.usage.cost_usd) },
                  {
                    label: 'duration',
                    value:
                      r.duration_ms != null
                        ? formatDuration(r.duration_ms)
                        : durationBetween(r.started_at, r.finished_at),
                  },
                  { label: 'turns', value: r.turns ? String(r.turns) : null },
                ]}
              />
            ))}
          </ListCard>
        </div>
      )}

      {runs !== null && filtered.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
          {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${filtered.length} —`}
        </div>
      )}
    </div>
  );
}
