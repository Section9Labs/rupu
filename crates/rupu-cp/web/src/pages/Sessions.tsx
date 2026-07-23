// Sessions list — agent sessions tracked by the control plane. Active /
// Archived FilterPills group (Active is the default) + a host scope select;
// fetch/paginate/poll is owned by the shared `usePagedList` hook — Active
// polls every 5 s (page 0 only, spliced back over the head of the list, same
// as WorkflowRuns/AgentRuns' "Running" tab — see `usePagedList`'s doc comment
// for why that doesn't reset a scrolled view). Each row links to
// /sessions/:id. Status is `unknown` on the wire, coerced via
// lib/sessionStatus — a distinct vocabulary from the run-status enum
// `StatusPill` renders, so this page keeps its own dot+label rendering
// rather than routing through StatusPill.
//
// Find (2026-07-23 operator feedback amendment #1): a `SearchInput` in the
// FilterBar's search slot narrows the loaded rows client-side, live per
// keystroke, over agent name / session id / host id — composing with (not
// replacing) the Active/Archived pill above it.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { RefreshCw } from 'lucide-react';
import { api, type SessionSummary } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import UsageBarChart from '../components/charts/UsageBarChart';
import { Button } from '../components/ui/Button';
import { FilterBar } from '../components/ui/FilterBar';
import { FilterPills, type FilterPillOption } from '../components/ui/FilterPills';
import { SearchInput } from '../components/ui/SearchInput';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import HostSelect, { ALL_HOSTS } from '../components/HostSelect';
import { usePagedList } from '../lib/usePagedList';
import { cn } from '../lib/cn';
import { durationBetween } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { sessionStatusDot, sessionStatusLabel } from '../lib/sessionStatus';
import { shortId } from '../lib/shortId';

type Tab = 'active' | 'archived';

const TAB_OPTIONS: FilterPillOption[] = [
  { value: 'active', label: 'Active' },
  { value: 'archived', label: 'Archived' },
];

export default function Sessions() {
  const [tab, setTab] = useState<Tab>('active');
  // Default to 'local' → fast server-side path; ALL_HOSTS → fan-out.
  const [hostFilter, setHostFilter] = useState<string>('local');
  // Row-action (archive/restore/delete) failures — kept separate from the
  // list-fetch error the hook owns, but shown in the same banner.
  const [actionError, setActionError] = useState<string | null>(null);
  const [query, setQuery] = useState('');

  const { rows, loading, error, hasMore, sentinelRef, refresh, ended } = usePagedList<SessionSummary>({
    fetch: ({ offset, limit }) => {
      const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
      return api.getSessions({ scope: tab, offset, limit, host });
    },
    deps: [tab, hostFilter],
    poll: tab === 'active',
  });

  // Find — case-insensitive substring across the fields this table actually
  // renders: agent name (subject), session id, and host id. Composes with
  // (narrows within) the Active/Archived pill above.
  const q = query.trim().toLowerCase();
  const visible = q
    ? rows.filter((r) =>
        [r.agent_name, r.session_id, r.host_id]
          .filter((v): v is string => Boolean(v))
          .some((v) => v.toLowerCase().includes(q)),
      )
    : rows;

  // Row-level archive / restore / delete — each refetches after success.
  async function handleRowArchive(id: string) {
    try {
      await api.archiveSession(id);
      setActionError(null);
      refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Archive failed');
    }
  }

  async function handleRowRestore(id: string) {
    try {
      await api.restoreSession(id);
      setActionError(null);
      refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Restore failed');
    }
  }

  async function handleRowDelete(id: string) {
    if (!window.confirm('Permanently delete this session and its transcripts? This cannot be undone.')) return;
    try {
      await api.deleteSession(id);
      setActionError(null);
      refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  // Action column — changes shape based on the current tab (active vs archived).
  const actionColumn = buildActionColumn(tab, handleRowArchive, handleRowRestore, handleRowDelete);
  const columns: Column<SessionSummary>[] = [...SESSION_BASE_COLUMNS, actionColumn];
  const bannerError = error ?? actionError;

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
        <Button variant="secondary" onClick={() => refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      <div className="mb-5">
        <FilterBar
          filters={<FilterPills options={TAB_OPTIONS} value={tab} onChange={(v) => setTab(v as Tab)} />}
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
          scope={<HostSelect allowAll ariaLabel="Host filter" value={hostFilter} onChange={setHostFilter} />}
        />
      </div>

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading sessions…" />
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          title={tab === 'active' ? 'No active sessions' : 'No archived sessions'}
          hint={
            tab === 'active'
              ? 'Active sessions appear here once an agent conversation is started against this control plane.'
              : 'Archived sessions appear here once an active conversation is closed.'
          }
        />
      ) : visible.length === 0 ? (
        <EmptyState title="No matches" hint={`No sessions match "${query}".`} />
      ) : (
        <div className="space-y-6">
          {visible.some((s) => s.usage) && (
            <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
              <UsageBarChart bars={visible.filter((s) => s.usage).map((s) => {
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
            columns={columns}
            rows={visible}
            rowKey={(s) => s.session_id}
          />
          {(loading || hasMore || ended) && (
            <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
              {q
                ? `${visible.length} matches of ${rows.length} loaded`
                : loading
                  ? 'loading more…'
                  : hasMore
                    ? 'scroll for more'
                    : `— end of ${rows.length} —`}
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

const SESSION_BASE_COLUMNS: Column<SessionSummary>[] = [
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
    fit: true,
    sortable: true,
    sortValue: (s) => s.host_id ?? 'local',
    render: (s) => (
      <span className="text-note text-ink-mute font-mono">{s.host_id ?? 'local'}</span>
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
];

/** Build the per-row action column. Recreated whenever the tab changes. */
function buildActionColumn(
  tab: Tab,
  onArchive: (id: string) => void,
  onRestore: (id: string) => void,
  onDelete: (id: string) => void,
): Column<SessionSummary> {
  return {
    key: 'action',
    header: '',
    fit: true,
    align: 'right',
    render: (s) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => e.stopPropagation()}
      >
        {s.active_run_id && (
          <Link
            to={`/runs/${encodeURIComponent(s.active_run_id)}`}
            className="inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 bg-info-bg text-info ring-info/30 hover:bg-info-bg"
            onClick={(e) => e.stopPropagation()}
          >
            active run
          </Link>
        )}
        {tab === 'active' ? (
          <Button
            variant="ring"
            onClick={() => onArchive(s.session_id)}
            aria-label={`Archive session ${s.session_id}`}
          >
            Archive
          </Button>
        ) : (
          <Button
            variant="ring"
            onClick={() => onRestore(s.session_id)}
            aria-label={`Restore session ${s.session_id}`}
          >
            Restore
          </Button>
        )}
        <Button
          variant="ring-danger"
          onClick={() => onDelete(s.session_id)}
          aria-label={`Delete session ${s.session_id}`}
        >
          Delete
        </Button>
      </div>
    ),
  };
}
