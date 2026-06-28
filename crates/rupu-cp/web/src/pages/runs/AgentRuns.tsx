// Agent run-stream page — standalone and session-bound agent runs.
// No DAG graph (agent runs have no workflow DAG); shows transcript_path as text.
// status and started_at are optional (standalone runs may lack them).
// Three tabs (Running / Completed / Failed-Rejected) with independent fetches.
// Running polls every 5 s (unpaginated); Completed/Failed paginate (no poll) —
// keeping paginated history off the poll loop avoids the scroll-reset flicker.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import { api, type AgentRunRow } from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { Button } from '../../components/ui/Button';
import { cn } from '../../lib/cn';
import { relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;

type Tab = 'active' | 'completed' | 'failed';

const TABS: { id: Tab; label: string }[] = [
  { id: 'active', label: 'Running' },
  { id: 'completed', label: 'Completed' },
  { id: 'failed', label: 'Failed / Rejected' },
];

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}…` : id;
}

const SOURCE_CLS: Record<string, string> = {
  standalone: 'bg-slate-100 text-slate-600 ring-slate-200',
  session:    'bg-sky-50 text-sky-700 ring-sky-200',
};

function SourceChip({ source }: { source: string }) {
  const cls = SOURCE_CLS[source] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {source}
    </span>
  );
}

// Render the raw status string as a simple badge — AgentRunRow.status is a
// free-form string (not RunStatusStr), so we map known values to colors
// and fall back to neutral for unknown ones.
const STATUS_CLS: Record<string, string> = {
  running:           'bg-blue-50 text-blue-700 ring-blue-200',
  completed:         'bg-green-50 text-green-700 ring-green-200',
  failed:            'bg-red-50 text-red-700 ring-red-200',
  awaiting_approval: 'bg-amber-50 text-amber-800 ring-amber-200',
  rejected:          'bg-red-50 text-red-700 ring-red-200',
  pending:           'bg-slate-100 text-slate-600 ring-slate-200',
};

function StatusBadge({ status }: { status: string }) {
  const cls = STATUS_CLS[status] ?? 'bg-slate-100 text-slate-600 ring-slate-200';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium px-1.5 py-0.5', cls)}>
      {status}
    </span>
  );
}

export default function AgentRuns() {
  const [tab, setTab] = useState<Tab>('active');
  const [agentRuns, setAgentRuns] = useState<AgentRunRow[] | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // Page-0 fetch (and the 5 s poll on the active tab). Reset on tab change.
  // Active: fetch ALL in one call (unpaginated) → poll never resets a scrolled
  // list. Completed/Failed: page-0 only; loadMore appends.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const limit = tab === 'active' ? 200 : PAGE;
      const data = await api.getAgentRuns({ lifecycle: tab, limit });
      setAgentRuns(data);
      setHasMore(tab !== 'active' && data.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load agent runs');
    } finally {
      setRefreshing(false);
    }
  }, [tab]);

  useEffect(() => {
    setAgentRuns(null); // show loading on tab switch
    void refresh();
    if (tab === 'active') {
      const t = window.setInterval(() => void refresh(), 5000);
      return () => window.clearInterval(t);
    }
    return () => {};
  }, [tab, refresh]);

  const loadMore = async () => {
    if (tab === 'active') return; // active is unpaginated
    const current = agentRuns ?? [];
    const next = await api.getAgentRuns({ lifecycle: tab, offset: current.length, limit: PAGE });
    if (next.length === 0) { setHasMore(false); return; }
    setAgentRuns([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Sort newest-first where started_at is available; runs without it sink to
  // the bottom so that active/recent runs remain prominent.
  const sorted = [...(agentRuns ?? [])].sort((a, b) => {
    if (!a.started_at && !b.started_at) return 0;
    if (!a.started_at) return 1;
    if (!b.started_at) return -1;
    return Date.parse(b.started_at) - Date.parse(a.started_at);
  });

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Agent Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Standalone and session-bound agent invocations.</p>
        </div>
        <Button variant="secondary" onClick={() => void refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      {/* Lifecycle tabs */}
      <div className="flex items-center gap-2 mb-5">
        {TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={cn(
              'text-xs font-medium px-3 py-1.5 rounded-md border transition-colors',
              tab === t.id
                ? 'bg-brand-600 text-white border-brand-600'
                : 'bg-panel text-ink-dim border-border hover:bg-slate-100',
            )}
          >
            {t.label}
          </button>
        ))}
      </div>

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {agentRuns === null ? (
        <div className="text-sm text-ink-dim">Loading agent runs…</div>
      ) : sorted.length === 0 ? (
        <AgentRunsEmpty />
      ) : (
        <section>
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
            <UsageBarChart bars={sorted.map((r) => ({
              id: r.run_id,
              label: r.agent ?? r.run_id,
              to: r.transcript_path
                ? `/transcript?path=${encodeURIComponent(r.transcript_path)}&live=${isRunning(r.status) ? 1 : 0}`
                : undefined,
              input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
              cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
            }))} />
          </div>
          <SortableTable<AgentRunRow>
            columns={AGENT_RUN_COLUMNS}
            rows={sorted}
            rowKey={(r) => r.run_id}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          {tab !== 'active' && hasMore && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              {loading ? 'loading more…' : 'scroll for more'}
            </div>
          )}
        </section>
      )}
    </div>
  );
}

/** Returns true when the status string indicates the run is still in progress. */
function isRunning(status: string | null | undefined): boolean {
  return status === 'running' || status === 'awaiting_approval';
}

const AGENT_RUN_COLUMNS: Column<AgentRunRow>[] = [
  {
    key: 'agent',
    header: 'Agent',
    sortable: true,
    sortValue: (r) => r.agent ?? null,
    render: (r) => (
      <div className="min-w-0">
        <span className="text-sm font-medium text-ink">{r.agent ?? '—'}</span>
        {(r.trigger_source || r.session_id) && (
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5">
            {r.trigger_source && (
              <span className="text-note text-ink-dim">
                via <span className="font-mono">{r.trigger_source}</span>
              </span>
            )}
            {r.session_id && (
              <span className="text-note text-ink-dim">
                session{' '}
                <Link
                  to={`/sessions/${encodeURIComponent(r.session_id)}`}
                  className="text-brand-600 hover:underline font-mono"
                >
                  {shortId(r.session_id)}
                </Link>
              </span>
            )}
          </div>
        )}
      </div>
    ),
  },
  {
    key: 'run',
    header: 'Run',
    // Agent runs are NOT in the workflow run-store (/api/runs/:id would 404), so
    // the Run id links to the transcript view — matching the page's pre-table
    // behavior. No transcript_path → render the id as plain text.
    render: (r) => {
      const href = r.transcript_path
        ? `/transcript?path=${encodeURIComponent(r.transcript_path)}&live=${isRunning(r.status) ? 1 : 0}`
        : null;
      return href ? (
        <Link to={href} className="text-note text-ink-mute font-mono hover:underline">
          {shortId(r.run_id)}
        </Link>
      ) : (
        <span className="text-note text-ink-mute font-mono">{shortId(r.run_id)}</span>
      );
    },
  },
  {
    key: 'source',
    header: 'Source',
    sortable: true,
    sortValue: (r) => r.source,
    render: (r) => <SourceChip source={r.source} />,
  },
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (r) => r.status ?? null,
    render: (r) => (r.status ? <StatusBadge status={r.status} /> : <span className="text-ink-mute">—</span>),
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
    sortValue: (r) => r.duration_ms ?? null,
    render: (r) => <span className="text-ink-dim">{formatDuration(r.duration_ms)}</span>,
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
    render: (r) => (
      <span className="text-ink-mute">{r.started_at ? relativeTime(r.started_at) : '—'}</span>
    ),
  },
];

function AgentRunsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No agent runs yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Standalone and session-bound agent invocations will appear here once they run.
      </p>
    </div>
  );
}
