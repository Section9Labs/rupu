// Workflow run-stream page — execution history for workflow runs.
// Three tabs (Running / Completed / Failed-Rejected) with independent fetches.
// Running polls every 5 s (unpaginated); Completed/Failed paginate (no poll) —
// keeping paginated history off the poll loop avoids the scroll-reset flicker.
//
// Archived toggle: when enabled, switches the fetch to /api/runs/archived and
// shows Restore + Delete per-row actions; when disabled, shows Archive + Delete.

import { useCallback, useEffect, useState } from 'react';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type HostView, type RunListRow } from '../../lib/api';
import { StatusPill } from '../../components/StatusPill';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { Button } from '../../components/ui/Button';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { shortId } from '../../lib/shortId';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;
/** Sentinel select value meaning "fetch all hosts" (fan-out / no ?host= param). */
const ALL_HOSTS = '__all__';

type Tab = 'active' | 'completed' | 'failed';

const TABS: { id: Tab; label: string }[] = [
  { id: 'active', label: 'Running' },
  { id: 'completed', label: 'Completed' },
  { id: 'failed', label: 'Failed / Rejected' },
];

type TriggerFilter = 'all' | 'manual' | 'cron' | 'event';

const FILTERS: TriggerFilter[] = ['all', 'manual', 'cron', 'event'];

const TRIGGER_CHIP_CLS: Record<string, string> = {
  manual: 'bg-surface text-ink ring-border',
  cron:   'bg-violet-50 text-violet-700 ring-violet-200',
  event:  'bg-sky-50 text-sky-700 ring-sky-200',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CHIP_CLS[trigger] ?? 'bg-surface text-ink ring-border';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {trigger}
    </span>
  );
}

/** Build the detail link for a run, including ?host= for remote runs. */
function runHref(r: RunListRow): string {
  const hid = r.host_id;
  if (hid && hid !== 'local') {
    return `/runs/${encodeURIComponent(r.id)}?host=${encodeURIComponent(hid)}`;
  }
  return `/runs/${encodeURIComponent(r.id)}`;
}

export default function WorkflowRuns() {
  const [tab, setTab] = useState<Tab>('active');
  const [archived, setArchived] = useState(false);
  const [runs, setRuns] = useState<RunListRow[] | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [filter, setFilter] = useState<TriggerFilter>('all');
  // Default to 'local' → fast server-side path; ALL_HOSTS → fan-out.
  const [hostFilter, setHostFilter] = useState<string>('local');
  const [hosts, setHosts] = useState<HostView[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getHosts().then((hs) => { if (!cancelled) setHosts(hs); }).catch(() => { if (!cancelled) setHosts([]); });
    return () => { cancelled = true; };
  }, []);

  // Page-0 fetch (and the 5 s poll on the active tab). Reset on tab/host/archived change.
  // Active: fetch ALL in one call (unpaginated) → poll never resets a scrolled
  // list. Completed/Failed: page-0 only; loadMore appends.
  // Archived: single fetch from /api/runs/archived (no lifecycle tabs, no poll).
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      if (archived) {
        const data = await api.getArchivedRuns('workflow');
        setRuns(data);
        setHasMore(false);
      } else {
        const limit = tab === 'active' ? 200 : PAGE;
        const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
        const data = await api.getWorkflowRuns({ lifecycle: tab, limit, host });
        setRuns(data);
        setHasMore(tab !== 'active' && data.length >= PAGE);
      }
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load runs');
    } finally {
      setRefreshing(false);
    }
  }, [tab, hostFilter, archived]);

  useEffect(() => {
    setRuns(null); // show loading on tab/host/archived switch
    void refresh();
    if (!archived && tab === 'active') {
      const t = window.setInterval(() => void refresh(), 5000);
      return () => window.clearInterval(t);
    }
    return () => {};
  }, [tab, hostFilter, refresh, archived]);

  const loadMore = async () => {
    if (archived || tab === 'active') return; // archived + active are unpaginated
    const current = runs ?? [];
    const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
    const next = await api.getWorkflowRuns({ lifecycle: tab, offset: current.length, limit: PAGE, host });
    if (next.length === 0) { setHasMore(false); return; }
    setRuns([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Trigger filter is still client-side (cheap; host filter is server-side).
  const filtered = (runs ?? []).filter((r) => {
    if (!archived && filter !== 'all' && r.trigger !== filter) return false;
    return true;
  });

  // Row-level archive / restore / delete — each refetches after success.
  async function handleRowArchive(id: string) {
    try {
      await api.archiveRun(id);
      void refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Archive failed');
    }
  }

  async function handleRowRestore(id: string) {
    try {
      await api.restoreRun(id);
      void refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Restore failed');
    }
  }

  async function handleRowDelete(id: string) {
    if (!window.confirm('Permanently delete this run and its transcripts? This cannot be undone.')) return;
    try {
      await api.deleteRun(id);
      void refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  // Action column — changes shape based on archived mode.
  const actionColumn: Column<RunListRow> = {
    key: 'actions',
    header: '',
    align: 'right',
    width: 'w-36',
    render: (r) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => e.stopPropagation()}
      >
        {archived ? (
          <button
            type="button"
            onClick={() => void handleRowRestore(r.id)}
            className="rounded px-2 py-0.5 text-note font-medium ring-1 bg-panel text-ink-dim ring-border hover:bg-surface-hover"
            aria-label={`Restore run ${r.id}`}
          >
            Restore
          </button>
        ) : (
          <button
            type="button"
            onClick={() => void handleRowArchive(r.id)}
            className="rounded px-2 py-0.5 text-note font-medium ring-1 bg-panel text-ink-dim ring-border hover:bg-surface-hover"
            aria-label={`Archive run ${r.id}`}
          >
            Archive
          </button>
        )}
        <button
          type="button"
          onClick={() => void handleRowDelete(r.id)}
          className="rounded px-2 py-0.5 text-note font-medium ring-1 bg-err-bg text-err ring-err/30 hover:bg-err-bg"
          aria-label={`Delete run ${r.id}`}
        >
          Delete
        </button>
      </div>
    ),
  };

  const columns: Column<RunListRow>[] = [...WORKFLOW_RUN_COLUMNS, actionColumn];

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Workflow Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Workflow executions across this control plane.</p>
        </div>
        <Button variant="secondary" onClick={() => void refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      {/* Lifecycle tabs + Archived toggle */}
      <div className="flex items-center gap-2 mb-4">
        {/* Archived toggle — when active, replaces lifecycle tabs with archived list */}
        <button
          onClick={() => setArchived((v) => !v)}
          className={cn(
            'text-xs font-medium px-3 py-1.5 rounded-md border transition-colors',
            archived
              ? 'bg-brand-600 text-white border-brand-600'
              : 'bg-panel text-ink-dim border-border hover:bg-surface-hover',
          )}
          aria-pressed={archived}
        >
          Archived
        </button>

        {!archived && TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={cn(
              'text-xs font-medium px-3 py-1.5 rounded-md border transition-colors',
              tab === t.id
                ? 'bg-brand-600 text-white border-brand-600'
                : 'bg-panel text-ink-dim border-border hover:bg-surface-hover',
            )}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* Trigger filter chips + Host filter (hidden in archived mode) */}
      <div className={cn('flex flex-wrap items-center gap-2 mb-5', archived && 'hidden')}>
        {FILTERS.map((f) => (
          <button
            key={f}
            onClick={() => setFilter(f)}
            className={cn(
              'text-xs font-medium px-3 py-1 rounded-full border transition-colors',
              filter === f
                ? 'bg-brand-600 text-white border-brand-600'
                : 'bg-panel text-ink-dim border-border hover:bg-surface-hover',
            )}
          >
            {f === 'all' ? 'All' : f.charAt(0).toUpperCase() + f.slice(1)}
          </button>
        ))}

        {/* Host filter — always visible; drives server-side fetch scope. */}
        <select
          value={hostFilter}
          onChange={(e) => setHostFilter(e.target.value)}
          aria-label="Host filter"
          className="text-xs font-medium px-2 py-1 rounded-md border border-border bg-panel text-ink-dim focus:outline-none focus:border-brand-500"
        >
          <option value="local">This host</option>
          <option value={ALL_HOSTS}>All hosts</option>
          {(hosts ?? [])
            .filter((h) => h.transport_kind !== 'local')
            .map((h) => (
              <option key={h.id} value={h.id}>{h.name}</option>
            ))}
        </select>
      </div>

      {error && (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {runs === null ? (
        <div className="text-sm text-ink-dim">Loading runs…</div>
      ) : filtered.length === 0 ? (
        <WorkflowRunsEmpty hasRuns={runs.length > 0} />
      ) : (
        <div className="space-y-6">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
            <UsageBarChart bars={filtered.map((r) => ({
              id: r.id, label: r.workflow_name, to: runHref(r),
              input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
              cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
            }))} />
          </div>
          <SortableTable<RunListRow>
            columns={columns}
            rows={filtered}
            rowKey={(r) => r.id}
            rowHref={runHref}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          {tab !== 'active' && hasMore && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              {loading ? 'loading more…' : 'scroll for more'}
            </div>
          )}
        </div>
      )}
    </div>
  );
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

const WORKFLOW_RUN_COLUMNS: Column<RunListRow>[] = [
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
    key: 'host',
    header: 'Host',
    sortable: true,
    sortValue: (r) => r.host_id ?? 'local',
    render: (r) => (
      <span className="text-note text-ink-mute font-mono">{r.host_id ?? 'local'}</span>
    ),
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

function WorkflowRunsEmpty({ hasRuns }: { hasRuns: boolean }) {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">
        {hasRuns ? 'No runs match this filter' : 'No workflow runs yet'}
      </h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        {hasRuns
          ? 'Try selecting a different trigger or host filter above.'
          : 'Workflow runs will appear here once you dispatch one from the CLI, the desktop app, or a scheduled trigger.'}
      </p>
    </div>
  );
}
