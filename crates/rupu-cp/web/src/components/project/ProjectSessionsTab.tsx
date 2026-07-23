// Project Sessions tab body — paginated session list scoped to one project,
// mirroring the standalone Sessions page (pages/Sessions.tsx) on the kit:
// FilterBar + FilterPills (Scope) + SearchInput Find + usePagedList +
// SortableTable fit/subject columns + kit empty/loading/error states.
//
// Project tabs are already workspace-scoped (the project IS the scope), so
// there is no HostSelect slot here — unlike Sessions, which fans out across
// hosts.
//
// Ported from pages/ProjectSessions.tsx, reshaped into a self-contained
// component keyed off the `wsId` prop. Rows render via the shared SortableTable.
//
// Find (parity with Sessions' 2026-07-23 amendment): a `SearchInput` in the
// FilterBar's search slot narrows the loaded rows client-side, live per
// keystroke, over agent name / session id — composing with (not replacing)
// the Scope pill above it.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type SessionSummary } from '../../lib/api';
import SortableTable, { type Column } from '../lists/SortableTable';
import UsageBarChart from '../charts/UsageBarChart';
import { FilterBar } from '../ui/FilterBar';
import { FilterPills, type FilterPillOption } from '../ui/FilterPills';
import { SearchInput } from '../ui/SearchInput';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import { usePagedList } from '../../lib/usePagedList';
import { durationBetween } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { sessionStatusDot, sessionStatusLabel, sessionStatusTone } from '../../lib/sessionStatus';
import { shortId } from '../../lib/shortId';
import { cn } from '../../lib/cn';

// --- Scope filter -----------------------------------------------------------

type ScopeFilter = 'all' | 'active' | 'archived';

const SCOPE_OPTIONS: FilterPillOption[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Active' },
  { value: 'archived', label: 'Archived' },
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
    fit: true,
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
    subject: true,
    sortable: true,
    sortValue: (s) => s.agent_name,
    titleValue: (s) => s.agent_name,
    render: (s) => <span className="text-sm font-medium text-ink">{s.agent_name}</span>,
  },
  {
    key: 'session',
    header: 'Session',
    fit: true,
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
    fit: true,
    sortable: true,
    sortValue: (s) => s.model,
    render: (s) => <span className="text-note text-ink-mute font-mono">{s.model}</span>,
  },
  {
    key: 'in',
    header: 'In',
    align: 'right',
    fit: true,
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
    fit: true,
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
    fit: true,
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
    fit: true,
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
    fit: true,
    sortable: true,
    sortValue: (s) => s.total_turns,
    render: (s) => <span className="text-ink">{s.total_turns ? String(s.total_turns) : '—'}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (s) => sessionDurationMs(s),
    render: (s) => (
      <span className="text-ink-dim">{durationBetween(s.created_at, s.updated_at)}</span>
    ),
  },
  {
    key: 'action',
    header: '',
    fit: true,
    align: 'right',
    render: (s) =>
      s.active_run_id ? (
        <Link
          to={`/runs/${encodeURIComponent(s.active_run_id)}`}
          className="inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 bg-info-bg text-info ring-info/30 hover:bg-info-bg"
          onClick={(e) => e.stopPropagation()}
        >
          active run
        </Link>
      ) : null,
  },
];

export default function ProjectSessionsTab({ wsId }: { wsId: string }) {
  const [scope, setScope] = useState<ScopeFilter>('all');
  const [query, setQuery] = useState('');

  const { rows, loading, error, hasMore, sentinelRef, ended } = usePagedList<SessionSummary>({
    fetch: ({ offset, limit }) => api.getProjectSessions(wsId, { offset, limit }),
    deps: [wsId],
  });

  const filtered = rows.filter((s) => matchesScope(s, scope));

  // Find — case-insensitive substring across the fields this table actually
  // renders: agent name (subject) and session id. Composes with (narrows
  // within) the Scope pill above.
  const q = query.trim().toLowerCase();
  const visible = q
    ? filtered.filter((s) =>
        [s.agent_name, s.session_id]
          .filter((v): v is string => Boolean(v))
          .some((v) => v.toLowerCase().includes(q)),
      )
    : filtered;

  const usageBars = visible.filter((s) => s.usage);

  return (
    <div className="space-y-4">
      <FilterBar
        filters={
          <FilterPills
            label="Scope"
            options={SCOPE_OPTIONS}
            value={scope}
            onChange={(v) => setScope(v as ScopeFilter)}
          />
        }
        search={
          <SearchInput
            aria-label="Find sessions"
            placeholder="Find sessions…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setQuery('');
            }}
          />
        }
      />

      {error && <ErrorBanner>{error}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-12 flex items-center justify-center">
          <Spinner label="Loading sessions…" />
        </div>
      ) : filtered.length === 0 ? (
        <EmptyState
          title={rows.length === 0 ? 'No sessions for this project yet' : 'No sessions match this filter'}
        />
      ) : visible.length === 0 ? (
        <EmptyState title="No matches" hint={`No sessions match "${query}".`} />
      ) : (
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
            rows={visible}
            rowKey={(s) => s.session_id}
          />
        </div>
      )}

      {visible.length > 0 && (loading || hasMore || ended) && (
        <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
          {q
            ? `${visible.length} matches of ${filtered.length} loaded`
            : loading
              ? 'loading more…'
              : hasMore
                ? 'scroll for more'
                : `— end of ${filtered.length} —`}
        </div>
      )}
    </div>
  );
}
