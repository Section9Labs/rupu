// Sessions list — agent sessions tracked by the control plane. One query
// line filters the list, and the server evaluates it (`?q=` on every
// request, `SESSION_FIELDS`); with no `is:` term it lists the active
// sessions, as the Active pill did (`is:archived` the archive,
// `is:active,archived` both). Fetch/paginate/poll is owned by
// `usePerHostQueryList`: every host the query's `host:` terms allow, local
// painting at once and each remote merging in as it answers; host health is
// in the `host:` suggestions and one quiet line under the bar. Each row links
// to /sessions/:id.
//
// Column order + status glyph now match the canonical run-table standard
// (`docs/superpowers/plans/2026-07-24-rupu-cp-table-standardization.md`
// Task 3): Status leads via the shared `SessionStatusPill` (Task 2), then
// Agent (subject) → Session id → Host → Model → In/Out/Cached/Cost →
// Turns → Duration → Started → actions. Status is `unknown` on the wire —
// `SessionStatusPill` is handed the raw value directly and does its OWN
// exact-match-or-neutral-fallback (never coerces an unrecognized status onto
// a real state; see `components/StatusPill.tsx`).
//
// Customer scope (customers Plan 2B): the list follows the global scope
// (`useScopedList`; `?customer=` on every per-host request, AND-ed with the
// query). A scope the backend rejects (400) is cleared with a notice.

import { useCallback, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { MessageSquare, RefreshCw } from 'lucide-react';
import { api, type ListQueryOpts, type SessionSummary } from '../lib/api';
import SortableTable, { type Column } from '../components/lists/SortableTable';
import UsageBarChart from '../components/charts/UsageBarChart';
import { Button } from '../components/ui/Button';
import { EmptyState } from '../components/ui/EmptyState';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';
import { SessionStatusPill } from '../components/StatusPill';
import { AgentName } from '../components/codename/AgentName';
import { memberLabel } from '../lib/codename';
import type { PerHostFetchParams } from '../lib/perHost/usePerHostPagedList';
import { noHostAnswered, waitingLabel } from '../lib/perHost/status';
import { usePerHostQueryList } from '../lib/query/usePerHostQueryList';
import { SESSION_FIELDS, sessionsQuery } from '../lib/query/lists/sessions';
import { ListQueryBar, NoMatches, useListQuery } from '../components/query/ListQueryBar';
import { hostAlerts } from '../components/query/hostAlerts';
import { ListFooter } from '../components/lists/ListFooter';
import { cn } from '../lib/cn';
import { durationBetween, relativeTime } from '../lib/time';
import { formatTokens, formatCost } from '../lib/usage';
import { sessionStatusDisplayLabel } from '../lib/sessionStatus';
import { shortId } from '../lib/shortId';
import { useScopedList } from '../lib/useScopedList';
import { PricingErrorMark } from '../components/customers/PricingErrorMark';

/** Build the detail link for a session, including ?host= for remote sessions
 *  (mirrors WorkflowRuns.tsx's `runHref`). */
function sessionHref(s: SessionSummary): string {
  const hid = s.host_id;
  if (hid && hid !== 'local') {
    return `/sessions/${encodeURIComponent(s.session_id)}?host=${encodeURIComponent(hid)}`;
  }
  return `/sessions/${encodeURIComponent(s.session_id)}`;
}

export default function Sessions() {
  const navigate = useNavigate();
  const [q, setQ] = useListQuery();
  // Row-action (archive/restore/delete) failures — kept separate from the
  // list-fetch error the hook owns, but shown in the same banner.
  const [actionError, setActionError] = useState<string | null>(null);
  const scoped = useScopedList(undefined, []);
  const { customer } = scoped;

  const fetchRows = useCallback(
    (p: PerHostFetchParams & ListQueryOpts) =>
      customer ? scoped.guard(api.getSessions({ ...p, customer })) : api.getSessions(p),
    [customer, scoped.guard],
  );
  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended, facets, queryError, notes } =
    usePerHostQueryList<SessionSummary>({
      q: sessionsQuery(q),
      fields: SESSION_FIELDS,
      scope: [customer],
      fetch: fetchRows,
      timeField: 'updated_at',
      idField: 'session_id',
      poll: true,
    });
  // Never claim "nothing" while a host is still loading.
  const waiting = waitingLabel(slices);

  // Row-level archive / restore / delete — each drops the row and re-syncs
  // just its host after success.
  // `host` is the row's own `host_id` (undefined/`"local"` → local
  // mutator, any other id → proxied through that host's connector) so a
  // fanned-out remote-host row's action lands on the host that actually
  // owns the session.
  async function handleRowArchive(id: string, host?: string) {
    try {
      await api.archiveSession(id, host);
      setActionError(null);
      // The row left this list; drop it now, then re-sync just its host.
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Archive failed');
    }
  }

  async function handleRowRestore(id: string, host?: string) {
    try {
      await api.restoreSession(id, host);
      setActionError(null);
      // The row left this list; drop it now, then re-sync just its host.
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Restore failed');
    }
  }

  async function handleRowDelete(id: string, host?: string) {
    if (!window.confirm('Permanently delete this session and its transcripts? This cannot be undone.')) return;
    try {
      await api.deleteSession(id, host);
      setActionError(null);
      // The row left this list; drop it now, then re-sync just its host.
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  // Action column — an archived session restores, an active one archives.
  const actionColumn = buildActionColumn(navigate, handleRowArchive, handleRowRestore, handleRowDelete);
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

      <ListQueryBar
        value={q}
        onChange={setQ}
        fields={SESSION_FIELDS}
        facets={facets}
        label="Filter sessions"
        placeholder="Active sessions · agent:recon · is:archived"
        error={queryError}
        alerts={hostAlerts(notes, 'sessions', retryPaging)}
      />

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading sessions…" />
        </div>
      ) : rows.length === 0 && waiting ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label={waiting} />
        </div>
      ) : rows.length === 0 && noHostAnswered(slices) ? (
        <EmptyState title="No hosts answered" hint="Every host is offline or can't serve this list; the line under the query says which." />
      ) : rows.length === 0 ? (
        q.trim() ? (
          <NoMatches noun="sessions" />
        ) : (
          <EmptyState
            icon={<MessageSquare size={20} />}
            title="No active sessions"
            hint="Active sessions appear here once an agent conversation is started against this control plane; is:archived lists closed ones."
          />
        )
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
            columns={columns}
            rows={rows}
            rowKey={(s) => `${s.host_id ?? 'local'}:${s.session_id}`}
            rowHref={sessionHref}
          />
          <ListFooter sentinelRef={sentinelRef} loading={loading} hasMore={hasMore} ended={ended} count={rows.length} />
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
    // Sort on the label actually displayed (M3) — not the raw wire string —
    // so sorting groups rows by what the operator sees in the pill.
    sortValue: (s) => sessionStatusDisplayLabel(s.status),
    render: (s) => <SessionStatusPill status={s.status} />,
  },
  {
    key: 'agent',
    header: 'Agent',
    subject: true,
    sortable: true,
    sortValue: (s) => s.agent_name,
    titleValue: (s) => s.agent_name,
    render: (s) => (
      <span className="text-sm font-medium text-ink">
        {s.codename ? (
          <AgentName
            codename={s.codename}
            agent={s.agent_name}
            provider={s.provider_name}
            model={s.model}
            showCrew
            derived={s.codename_derived}
          />
        ) : (
          <span className="font-mono">
            {memberLabel(undefined, s.agent_name, s.provider_name, s.model)}
          </span>
        )}
      </span>
    ),
  },
  {
    key: 'session',
    header: 'Session',
    fit: true,
    // Plain content — row-level navigation is `rowHref` (SortableTable
    // link-wraps the whole row); an inline <Link> here would nest an <a>
    // inside SortableTable's own <a>.
    render: (s) => (
      <span className="text-note text-ink-mute font-mono">{shortId(s.session_id)}</span>
    ),
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
      <span className="inline-flex items-center justify-end gap-1 text-ink font-medium">
        <PricingErrorMark error={s.usage?.pricing_error} />
        {s.usage ? formatCost(s.usage.cost_usd) : '—'}
      </span>
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
    key: 'started',
    header: 'Started',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (s) => (s.created_at ? Date.parse(s.created_at) : null),
    render: (s) => <span className="text-ink-mute">{relativeTime(s.created_at)}</span>,
  },
];

/** Build the per-row action column. Recreated whenever the tab changes. */
function buildActionColumn(
  navigate: ReturnType<typeof useNavigate>,
  onArchive: (id: string, host?: string) => void,
  onRestore: (id: string, host?: string) => void,
  onDelete: (id: string, host?: string) => void,
): Column<SessionSummary> {
  return {
    key: 'action',
    header: '',
    fit: true,
    align: 'right',
    // Its own real buttons (Archive/Restore/Delete) — keep them
    // independently focusable/announced (I7).
    interactive: true,
    render: (s) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => {
          // The row is now link-wrapped (rowHref) — without both of these,
          // a click here either soft- or hard-navigates to the session
          // instead of acting (stopPropagation alone does not block the
          // enclosing <a>'s native default navigation action).
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        {s.active_run_id && (
          <button
            type="button"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              navigate(`/runs/${encodeURIComponent(s.active_run_id!)}`);
            }}
            className="inline-flex items-center rounded px-2 py-0.5 text-note font-medium ring-1 bg-info-bg text-info ring-info/30 hover:bg-info-bg"
          >
            active run
          </button>
        )}
        {s.scope !== 'archived' ? (
          <Button
            variant="ring"
            onClick={() => onArchive(s.session_id, s.host_id)}
            aria-label={`Archive session ${s.session_id}`}
          >
            Archive
          </Button>
        ) : (
          <Button
            variant="ring"
            onClick={() => onRestore(s.session_id, s.host_id)}
            aria-label={`Restore session ${s.session_id}`}
          >
            Restore
          </Button>
        )}
        <Button
          variant="ring-danger"
          onClick={() => onDelete(s.session_id, s.host_id)}
          aria-label={`Delete session ${s.session_id}`}
        >
          Delete
        </Button>
      </div>
    ),
  };
}
