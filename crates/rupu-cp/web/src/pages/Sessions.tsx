// Sessions list — agent sessions tracked by the control plane. Two tabs
// (Active / Archived) with independent fetches keyed on scope. Active polls
// every 5 s (unpaginated); Archived paginates (no poll) — keeping paginated
// history off the poll loop avoids the scroll-reset flicker. Each row links to
// /sessions/:id. Status is `unknown` on the wire, coerced via lib/sessionStatus.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { MessageSquare, RefreshCw } from 'lucide-react';
import { api, type SessionSummary, type HostView } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import UsageBarChart from '../components/charts/UsageBarChart';
import { Button } from '../components/ui/Button';
import { cn } from '../lib/cn';
import { durationBetween } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const PAGE = 20;
/** Sentinel select value meaning "fetch all hosts" (fan-out / no ?host= param). */
const ALL_HOSTS = '__all__';

type Tab = 'active' | 'archived';

const TABS: { id: Tab; label: string }[] = [
  { id: 'active', label: 'Active' },
  { id: 'archived', label: 'Archived' },
];

function shortId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 10)}…` : id;
}

export default function Sessions() {
  const [tab, setTab] = useState<Tab>('active');
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Default to 'local' → fast server-side path; ALL_HOSTS → fan-out.
  const [hostFilter, setHostFilter] = useState<string>('local');
  const [hosts, setHosts] = useState<HostView[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.getHosts().then((hs) => { if (!cancelled) setHosts(hs); }).catch(() => { if (!cancelled) setHosts([]); });
    return () => { cancelled = true; };
  }, []);

  // Page-0 fetch (and the 5 s poll on the active tab). Reset on tab/host change.
  // Active: fetch ALL in one call (unpaginated) → poll never resets a scrolled
  // list. Archived: page-0 only; loadMore appends.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const limit = tab === 'active' ? 200 : PAGE;
      const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
      const data = await api.getSessions({ scope: tab, limit, host });
      setSessions(data);
      setHasMore(tab !== 'active' && data.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load sessions');
    } finally {
      setRefreshing(false);
    }
  }, [tab, hostFilter]);

  useEffect(() => {
    setSessions(null); // show loading on tab/host switch
    void refresh();
    if (tab === 'active') {
      const t = window.setInterval(() => void refresh(), 5000);
      return () => window.clearInterval(t);
    }
    return () => {};
  }, [tab, hostFilter, refresh]);

  const loadMore = async () => {
    if (tab === 'active') return; // active is unpaginated
    const current = sessions ?? [];
    const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
    const next = await api.getSessions({ scope: tab, offset: current.length, limit: PAGE, host });
    if (next.length === 0) { setHasMore(false); return; }
    setSessions([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  };

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  const rows = sessions ?? [];

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Sessions</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Agent sessions tracked by this control plane — active conversations and their archived
            history.
          </p>
        </div>
        <Button variant="secondary" onClick={() => void refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      {/* Scope tabs */}
      <div className="flex items-center gap-2 mb-4">
        {TABS.map((t) => (
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

      {/* Host filter — drives server-side fetch scope. */}
      <div className="flex items-center gap-2 mb-5">
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

      {sessions === null ? (
        <div className="text-sm text-ink-dim">Loading sessions…</div>
      ) : rows.length === 0 ? (
        <EmptyState tab={tab} />
      ) : (
        <div className="space-y-6">
          {rows.some((s) => s.usage) && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
              <UsageBarChart bars={rows.filter((s) => s.usage).map((s) => {
                const hostSuffix = s.host_id && s.host_id !== 'local'
                  ? `?host=${encodeURIComponent(s.host_id)}`
                  : '';
                return {
                  id: s.session_id, label: s.agent_name,
                  to: `/sessions/${encodeURIComponent(s.session_id)}${hostSuffix}`,
                  input_tokens: s.usage?.input_tokens ?? 0, output_tokens: s.usage?.output_tokens ?? 0,
                  cached_tokens: s.usage?.cached_tokens ?? 0, cost_usd: s.usage?.cost_usd ?? null,
                };
              })} />
            </div>
          )}
          {/* No initialSort: the server returns sessions most-recent (updated_at)
              first, so source order already satisfies the default. Clicking any
              header re-sorts client-side. */}
          <SortableTable<SessionSummary>
            columns={SESSION_COLUMNS}
            rows={rows}
            rowKey={(s) => s.session_id}
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

/** Session duration in ms (created → last update); null when timestamps are bad. */
function sessionDurationMs(s: SessionSummary): number | null {
  const start = Date.parse(s.created_at);
  const end = Date.parse(s.updated_at);
  if (Number.isNaN(start) || Number.isNaN(end)) return null;
  return Math.max(0, end - start);
}

const SESSION_COLUMNS: Column<SessionSummary>[] = [
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (s) => sessionStatusLabel(s.status),
    render: (s) => (
      <span className="flex items-center gap-1.5">
        <span className={cn('inline-block w-2 h-2 rounded-full', sessionStatusDot(s.status))} />
        <span className="text-note text-ink-dim">{sessionStatusLabel(s.status)}</span>
      </span>
    ),
  },
  {
    key: 'agent',
    header: 'Agent',
    sortable: true,
    sortValue: (s) => s.agent_name,
    render: (s) => <span className="text-sm font-medium text-ink truncate">{s.agent_name}</span>,
  },
  {
    key: 'session',
    header: 'Session',
    render: (s) => {
      const hostSuffix = s.host_id && s.host_id !== 'local'
        ? `?host=${encodeURIComponent(s.host_id)}`
        : '';
      return (
        <Link
          to={`/sessions/${encodeURIComponent(s.session_id)}${hostSuffix}`}
          className="text-note text-ink-mute font-mono hover:underline"
        >
          {shortId(s.session_id)}
        </Link>
      );
    },
  },
  {
    key: 'host',
    header: 'Host',
    sortable: true,
    sortValue: (s) => s.host_id ?? 'local',
    render: (s) => (
      <span className="text-note text-ink-mute font-mono">{s.host_id ?? 'local'}</span>
    ),
  },
  {
    key: 'model',
    header: 'Model',
    sortable: true,
    sortValue: (s) => s.model,
    render: (s) => <span className="text-note text-ink-mute font-mono">{s.model}</span>,
  },
  {
    key: 'in',
    header: 'In',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (s) => s.usage?.input_tokens ?? null,
    render: (s) => (
      <span className="text-ink-dim">{s.usage ? formatTokens(s.usage.input_tokens) : '—'}</span>
    ),
  },
  {
    key: 'out',
    header: 'Out',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (s) => s.usage?.output_tokens ?? null,
    render: (s) => (
      <span className="text-ink-dim">{s.usage ? formatTokens(s.usage.output_tokens) : '—'}</span>
    ),
  },
  {
    key: 'cached',
    header: 'Cached',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (s) => s.usage?.cached_tokens ?? null,
    render: (s) =>
      s.usage?.cached_tokens ? (
        <span className="text-ink-dim">{formatTokens(s.usage.cached_tokens)}</span>
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
    sortValue: (s) => s.usage?.cost_usd ?? null,
    render: (s) => (
      <span className="text-ink font-medium">{s.usage ? formatCost(s.usage.cost_usd) : '—'}</span>
    ),
  },
  {
    key: 'turns',
    header: 'Turns',
    align: 'right',
    width: 'w-16',
    sortable: true,
    sortValue: (s) => s.total_turns,
    render: (s) => <span className="text-ink">{s.total_turns ? String(s.total_turns) : '—'}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (s) => sessionDurationMs(s),
    render: (s) => (
      <span className="text-ink-dim">{durationBetween(s.created_at, s.updated_at)}</span>
    ),
  },
  {
    key: 'action',
    header: '',
    align: 'right',
    width: 'w-24',
    render: (s) =>
      s.active_run_id ? (
        <Link
          to={`/runs/${encodeURIComponent(s.active_run_id)}`}
          className="inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 bg-info-bg text-info ring-info/30 hover:bg-info-bg"
        >
          active run
        </Link>
      ) : null,
  },
];

function EmptyState({ tab }: { tab: Tab }) {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <MessageSquare size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">
        {tab === 'active' ? 'No active sessions' : 'No archived sessions'}
      </h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        {tab === 'active'
          ? 'Active sessions appear here once an agent conversation is started against this control plane.'
          : 'Archived sessions appear here once an active conversation is closed.'}
      </p>
    </div>
  );
}
