// Project Sessions tab body — paginated session list scoped to one project,
// with a client-side scope filter (All / Active / Archived) applied to the
// loaded rows.
//
// Ported from pages/ProjectSessions.tsx, reshaped into a self-contained
// component keyed off the `wsId` prop. Rows render via the shared SortableTable.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type SessionSummary } from '../../lib/api';
import SortableTable, { type Column } from '../lists/SortableTable';
import UsageBarChart from '../charts/UsageBarChart';
import { durationBetween } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { sessionStatusDot, sessionStatusLabel, sessionStatusTone } from '../../lib/sessionStatus';
import { shortId } from '../../lib/shortId';
import { cn } from '../../lib/cn';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;

// --- Scope filter -----------------------------------------------------------

type ScopeFilter = 'all' | 'active' | 'archived';

const SCOPE_FILTERS: { id: ScopeFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'active', label: 'Active' },
  { id: 'archived', label: 'Archived' },
];

// A session is "archived" when its status tone resolves to `stopped`
// (done / stopped / archived / exited per sessionStatusTone); everything
// else — running / idle / neutral — is treated as "active".
function isArchived(s: SessionSummary): boolean {
  return sessionStatusTone(s.status) === 'stopped';
}

function matchesScope(s: SessionSummary, filter: ScopeFilter): boolean {
  if (filter === 'all') return true;
  return filter === 'archived' ? isArchived(s) : !isArchived(s);
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
    render: (s) => (
      <Link
        to={`/sessions/${encodeURIComponent(s.session_id)}`}
        className="text-note text-ink-mute font-mono hover:underline"
      >
        {shortId(s.session_id, 10)}
      </Link>
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

export default function ProjectSessionsTab({ wsId }: { wsId: string }) {
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [scope, setScope] = useState<ScopeFilter>('all');

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    setSessions(null);
    setError(null);
    api
      .getProjectSessions(wsId, { limit: PAGE })
      .then((pageData) => {
        if (cancelled) return;
        setSessions(pageData);
        setHasMore(pageData.length >= PAGE);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Failed to load sessions');
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const loadMore = useCallback(async () => {
    if (!wsId) return;
    const current = sessions ?? [];
    const next = await api.getProjectSessions(wsId, { offset: current.length, limit: PAGE });
    if (next.length === 0) {
      setHasMore(false);
      return;
    }
    setSessions([...current, ...next]);
    if (next.length < PAGE) setHasMore(false);
  }, [wsId, sessions]);

  const { sentinelRef, loading } = useInfiniteScroll({ hasMore, loadMore });

  // Clicking the already-active chip returns it to "All".
  const toggleScope = (id: ScopeFilter) => setScope((cur) => (cur === id ? 'all' : id));

  const filtered = (sessions ?? []).filter((s) => matchesScope(s, scope));
  const usageBars = filtered.filter((s) => s.usage);

  return (
    <div className="space-y-4">
      {/* Scope filter chips */}
      <div className="flex items-center gap-2">
        <span className="text-meta font-semibold uppercase tracking-widest text-ink-mute w-14">
          Scope
        </span>
        {SCOPE_FILTERS.map((f) => (
          <button
            key={f.id}
            type="button"
            onClick={() => (f.id === 'all' ? setScope('all') : toggleScope(f.id))}
            className={cn(
              'text-xs font-medium px-3 py-1 rounded-full border transition-colors',
              scope === f.id
                ? 'bg-brand-600 text-white border-brand-600'
                : 'bg-panel text-ink-dim border-border hover:bg-surface-hover',
            )}
          >
            {f.label}
          </button>
        ))}
      </div>

      {error && (
        <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {sessions === null && !error && (
        <div className="text-sm text-ink-dim">Loading sessions…</div>
      )}

      {sessions !== null && filtered.length === 0 && (
        <div className="rounded-xl border border-dashed border-border bg-panel/50 py-12 flex items-center justify-center">
          <p className="text-sm text-ink-mute">
            {sessions.length === 0
              ? 'No sessions for this project yet'
              : 'No sessions match this filter'}
          </p>
        </div>
      )}

      {sessions !== null && filtered.length > 0 && (
        <div className="space-y-4">
          {usageBars.length > 0 && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
              <UsageBarChart
                bars={usageBars.map((s) => ({
                  id: s.session_id,
                  label: s.agent_name,
                  to: `/sessions/${encodeURIComponent(s.session_id)}`,
                  input_tokens: s.usage?.input_tokens ?? 0,
                  output_tokens: s.usage?.output_tokens ?? 0,
                  cached_tokens: s.usage?.cached_tokens ?? 0,
                  cost_usd: s.usage?.cost_usd ?? null,
                }))}
              />
            </div>
          )}
          {/* No initialSort: the server returns sessions most-recent first, so
              source order already satisfies the default. Headers re-sort
              client-side. */}
          <SortableTable<SessionSummary>
            columns={SESSION_COLUMNS}
            rows={filtered}
            rowKey={(s) => s.session_id}
          />
        </div>
      )}

      {sessions !== null && filtered.length > 0 && (
        <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
          {loading
            ? 'loading more…'
            : hasMore
              ? 'scroll for more'
              : `— end of ${filtered.length} —`}
        </div>
      )}
    </div>
  );
}
