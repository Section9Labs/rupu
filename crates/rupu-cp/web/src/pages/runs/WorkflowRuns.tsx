// Workflow run-stream page — execution history for workflow runs.
// Grouped by lifecycle (Active / Completed / Failed-Rejected).
// Polls every 5 s. Each row links to the live Run detail graph.

import { useCallback, useEffect, useState } from 'react';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type RunListRow, type RunStatusStr } from '../../lib/api';
import { StatusPill } from '../../components/StatusPill';
import MetricRow from '../../components/lists/MetricRow';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { ListCard } from '../../components/lists/ListCard';
import { SectionHeader, type SectionTone } from '../../components/lists/SectionHeader';
import { cn } from '../../lib/cn';
import { durationBetween } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;

const ACTIVE: RunStatusStr[] = ['running', 'pending', 'awaiting_approval'];
const TERMINAL_OK: RunStatusStr[] = ['completed'];
const TERMINAL_BAD: RunStatusStr[] = ['failed', 'rejected'];

type TriggerFilter = 'all' | 'manual' | 'cron' | 'event';

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

const TRIGGER_CHIP_CLS: Record<string, string> = {
  manual: 'bg-slate-100 text-slate-600 ring-slate-200',
  cron:   'bg-violet-50 text-violet-700 ring-violet-200',
  event:  'bg-sky-50 text-sky-700 ring-sky-200',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CHIP_CLS[trigger] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-[10px] font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {trigger}
    </span>
  );
}

export default function WorkflowRuns() {
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [filter, setFilter] = useState<TriggerFilter>('all');

  // Page-0 fetch (mount + 5 s refresh) — resets pagination.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const data = await api.getWorkflowRuns({ limit: PAGE });
      setRuns(data);
      setHasMore(data.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load runs');
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

  const loadMore = async () => {
    const current = runs ?? [];
    const next = await api.getWorkflowRuns({ offset: current.length, limit: PAGE });
    if (next.length === 0) { setHasMore(false); return; }
    setRuns([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  const filtered = (runs ?? []).filter((r) => filter === 'all' || r.trigger === filter);
  const active = filtered.filter((r) => ACTIVE.includes(r.status));
  const done   = filtered.filter((r) => TERMINAL_OK.includes(r.status));
  const bad    = filtered.filter((r) => TERMINAL_BAD.includes(r.status));

  const FILTERS: TriggerFilter[] = ['all', 'manual', 'cron', 'event'];

  return (
    <div className="p-8 max-w-5xl">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Workflow Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Workflow executions across this control plane.</p>
        </div>
        <button
          onClick={() => void refresh()}
          className="inline-flex items-center gap-1.5 text-xs font-medium px-3 py-1.5 rounded-md border border-border bg-panel text-ink hover:bg-slate-100"
        >
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </button>
      </header>

      {/* Trigger filter chips */}
      <div className="flex items-center gap-2 mb-5">
        {FILTERS.map((f) => (
          <button
            key={f}
            onClick={() => setFilter(f)}
            className={cn(
              'text-xs font-medium px-3 py-1 rounded-full border transition-colors',
              filter === f
                ? 'bg-brand-600 text-white border-brand-600'
                : 'bg-panel text-ink-dim border-border hover:bg-slate-100',
            )}
          >
            {f === 'all' ? 'All' : f.charAt(0).toUpperCase() + f.slice(1)}
          </button>
        ))}
      </div>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {runs === null ? (
        <div className="text-sm text-ink-dim">Loading runs…</div>
      ) : filtered.length === 0 ? (
        <WorkflowRunsEmpty hasRuns={runs.length > 0} />
      ) : (
        <div className="space-y-6">
          {runs.length > 0 && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
              <UsageBarChart bars={runs.map((r) => ({
                id: r.id, label: r.workflow_name, to: `/runs/${encodeURIComponent(r.id)}`,
                input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
                cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
              }))} />
            </div>
          )}
          <WorkflowRunSection tone="progress" label="Active"            runs={active} />
          <WorkflowRunSection tone="good"     label="Completed"         runs={done}   />
          <WorkflowRunSection tone="bad"      label="Failed / Rejected" runs={bad}    />
          {runs.length > 0 && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              {loading ? 'loading more…' : hasMore ? 'scroll for more' : `— end of ${runs.length} —`}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function WorkflowRunSection({
  tone,
  label,
  runs,
}: {
  tone: SectionTone;
  label: string;
  runs: RunListRow[];
}) {
  if (runs.length === 0) return null;
  return (
    <section>
      <SectionHeader tone={tone} label={label} count={runs.length} />
      <ListCard>
        {runs.map((r) => (
          <WorkflowRunRow key={r.id} run={r} />
        ))}
      </ListCard>
    </section>
  );
}

function WorkflowRunRow({ run }: { run: RunListRow }) {
  return (
    <MetricRow
      to={`/runs/${encodeURIComponent(run.id)}`}
      header={<>
        <span className="text-sm font-medium text-ink truncate">{run.workflow_name}</span>
        <span className="text-[11px] text-ink-mute font-mono">{shortId(run.id)}</span>
        <TriggerChip trigger={run.trigger} />
      </>}
      trailing={<StatusPill status={run.status} />}
      metrics={[
        { label: 'in', value: formatTokens(run.usage.input_tokens) },
        { label: 'out', value: formatTokens(run.usage.output_tokens) },
        { label: 'cached', value: run.usage.cached_tokens ? formatTokens(run.usage.cached_tokens) : null },
        { label: 'cost', value: formatCost(run.usage.cost_usd) },
        { label: 'duration', value: run.duration_ms != null ? formatDuration(run.duration_ms) : durationBetween(run.started_at, run.finished_at) },
        { label: 'turns', value: run.turns ? String(run.turns) : null },
      ]}
    />
  );
}

function WorkflowRunsEmpty({ hasRuns }: { hasRuns: boolean }) {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">
        {hasRuns ? 'No runs match this filter' : 'No workflow runs yet'}
      </h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        {hasRuns
          ? 'Try selecting a different trigger filter above.'
          : 'Workflow runs will appear here once you dispatch one from the CLI, the desktop app, or a scheduled trigger.'}
      </p>
    </div>
  );
}
