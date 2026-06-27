// Sessions list — agent sessions tracked by the control plane. Two tabs
// (Active / Archived) with independent fetches keyed on scope. Active polls
// every 5 s (unpaginated); Archived paginates (no poll) — keeping paginated
// history off the poll loop avoids the scroll-reset flicker. Each row links to
// /sessions/:id. Status is `unknown` on the wire, coerced via lib/sessionStatus.

import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { MessageSquare, RefreshCw } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import { ListCard } from '../components/lists/ListCard';
import MetricRow from '../components/lists/MetricRow';
import UsageBarChart from '../components/charts/UsageBarChart';
import { cn } from '../lib/cn';
import { durationBetween } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { useInfiniteScroll } from '../lib/useInfiniteScroll';

const PAGE = 20;

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

  // Page-0 fetch (and the 5 s poll on the active tab). Reset on tab change.
  // Active: fetch ALL in one call (unpaginated) → poll never resets a scrolled
  // list. Archived: page-0 only; loadMore appends.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const limit = tab === 'active' ? 200 : PAGE;
      const data = await api.getSessions({ scope: tab, limit });
      setSessions(data);
      setHasMore(tab !== 'active' && data.length >= PAGE);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load sessions');
    } finally {
      setRefreshing(false);
    }
  }, [tab]);

  useEffect(() => {
    setSessions(null); // show loading on tab switch
    void refresh();
    if (tab === 'active') {
      const t = window.setInterval(() => void refresh(), 5000);
      return () => window.clearInterval(t);
    }
    return () => {};
  }, [tab, refresh]);

  const loadMore = async () => {
    if (tab === 'active') return; // active is unpaginated
    const current = sessions ?? [];
    const next = await api.getSessions({ scope: tab, offset: current.length, limit: PAGE });
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
        <button
          onClick={() => void refresh()}
          className="inline-flex items-center gap-1.5 text-xs font-medium px-3 py-1.5 rounded-md border border-border bg-panel text-ink hover:bg-slate-100"
        >
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </button>
      </header>

      {/* Scope tabs */}
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

      {sessions === null ? (
        <div className="text-sm text-ink-dim">Loading sessions…</div>
      ) : rows.length === 0 ? (
        <EmptyState tab={tab} />
      ) : (
        <div className="space-y-6">
          {rows.some((s) => s.usage) && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
              <UsageBarChart bars={rows.filter((s) => s.usage).map((s) => ({
                id: s.session_id, label: s.agent_name,
                to: `/sessions/${encodeURIComponent(s.session_id)}`,
                input_tokens: s.usage?.input_tokens ?? 0, output_tokens: s.usage?.output_tokens ?? 0,
                cached_tokens: s.usage?.cached_tokens ?? 0, cost_usd: s.usage?.cost_usd ?? null,
              }))} />
            </div>
          )}
          <ListCard>
            {rows.map((s) => (
              <SessionRow key={s.session_id} session={s} />
            ))}
          </ListCard>
          {tab !== 'active' && hasMore && (
            <div ref={sentinelRef} className="py-2 text-center text-[11px] text-ink-mute">
              {loading ? 'loading more…' : 'scroll for more'}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function SessionRow({ session }: { session: SessionSummary }) {
  const navigate = useNavigate();
  const u = session.usage;
  return (
    <MetricRow
      to={`/sessions/${encodeURIComponent(session.session_id)}`}
      header={<>
        <span className="flex items-center gap-1.5">
          <span className={cn('inline-block w-2 h-2 rounded-full', sessionStatusDot(session.status))} />
          <span className="text-[11px] text-ink-dim">{sessionStatusLabel(session.status)}</span>
        </span>
        <span className="text-sm font-medium text-ink truncate">{session.agent_name}</span>
        <span className="text-[11px] text-ink-mute font-mono">{shortId(session.session_id)}</span>
        <span className="text-[11px] text-ink-mute font-mono">{session.model}</span>
      </>}
      trailing={session.active_run_id ? (
        <button
          type="button"
          onClick={(e) => { e.preventDefault(); e.stopPropagation(); navigate(`/runs/${encodeURIComponent(session.active_run_id ?? '')}`); }}
          className="shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-medium ring-1 bg-blue-50 text-blue-700 ring-blue-200 hover:bg-blue-100"
        >
          active run
        </button>
      ) : undefined}
      metrics={[
        { label: 'in', value: u ? formatTokens(u.input_tokens) : null },
        { label: 'out', value: u ? formatTokens(u.output_tokens) : null },
        { label: 'cached', value: u && u.cached_tokens ? formatTokens(u.cached_tokens) : null },
        { label: 'cost', value: u ? formatCost(u.cost_usd) : null },
        { label: 'turns', value: session.total_turns ? String(session.total_turns) : null },
        { label: 'duration', value: durationBetween(session.created_at, session.updated_at) },
      ]}
    />
  );
}

function EmptyState({ tab }: { tab: Tab }) {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-slate-100 flex items-center justify-center mb-3">
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
