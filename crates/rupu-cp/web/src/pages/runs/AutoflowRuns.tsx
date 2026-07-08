// Autoflow run-stream page — mirrors Runs → Workflows: the PRIMARY view is a
// clean run list (same SortableTable shape, row click → /runs/:id). The
// existing "Cycles" (batch view) and "Claims" (requeue/release) views are
// kept as secondary tabs, functionality untouched.
//
// All three tabs render via the shared SortableTable; the page chrome (tab
// switcher, refresh, per-tab pagination, empty/loading states) is preserved.

import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Inbox, RefreshCw } from 'lucide-react';
import {
  api,
  type AutoflowClaim,
  type AutoflowCycleRow,
  type AutoflowEventRow,
  type HostView,
} from '../../lib/api';
import { SectionHeader } from '../../components/lists/SectionHeader';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import UsageChip from '../../components/UsageChip';
import { Button } from '../../components/ui/Button';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { shortId } from '../../lib/shortId';
import { useInfiniteScroll } from '../../lib/useInfiniteScroll';

const PAGE = 20;
/** Sentinel select value meaning "fetch all hosts" (fan-out / no ?host= param). */
const ALL_HOSTS = '__all__';

const MODE_CLS: Record<string, string> = {
  ask:       'bg-warn-bg text-warn ring-warn/30',
  bypass:    'bg-ok-bg text-ok ring-ok/30',
  readonly:  'bg-surface text-ink ring-border',
  tick:      'bg-surface text-ink ring-border',
  serve:     'bg-sky-50 text-sky-700 ring-sky-200',
};

function ModeChip({ mode }: { mode: string }) {
  const cls = MODE_CLS[mode] ?? 'bg-surface text-ink ring-border';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {mode}
    </span>
  );
}

// Per-kind badge styling + human label for the events view.
const KIND_CLS: Record<string, string> = {
  run_launched:     'bg-ok-bg text-ok ring-ok/30',
  awaiting_human:   'bg-warn-bg text-warn ring-warn/30',
  awaiting_external:'bg-sky-50 text-sky-700 ring-sky-200',
  cycle_failed:     'bg-err-bg text-err ring-err/30',
};

const KIND_LABEL: Record<string, string> = {
  run_launched:     'launched',
  awaiting_human:   'awaiting human',
  awaiting_external:'awaiting external',
  cycle_failed:     'failed',
};

function KindBadge({ kind }: { kind: string }) {
  const cls = KIND_CLS[kind] ?? 'bg-surface text-ink ring-border';
  const label = KIND_LABEL[kind] ?? kind.replace(/_/g, ' ');
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {label}
    </span>
  );
}

function IssueChip({ displayRef }: { displayRef: string }) {
  return (
    <span className="inline-flex items-center rounded bg-surface text-ink ring-1 ring-border text-meta font-medium px-1.5 py-0.5">
      {displayRef}
    </span>
  );
}

// Title-case a snake_case status (e.g. `await_human` → `Await Human`).
function titleCase(s: string): string {
  return s
    .split('_')
    .filter(Boolean)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}

// Per-status badge styling for the claim lifecycle.
const CLAIM_STATUS_CLS: Record<string, string> = {
  await_human: 'bg-warn-bg text-warn ring-warn/30',
  running:     'bg-status-running/10 text-status-running ring-status-running/30',
  blocked:     'bg-err-bg text-err ring-err/30',
  complete:    'bg-ok-bg text-ok ring-ok/30',
  released:    'bg-surface text-ink ring-border',
};

function ClaimStatusBadge({ status }: { status: string }) {
  const cls = CLAIM_STATUS_CLS[status] ?? 'bg-surface text-ink ring-border';
  return (
    <span className={cn('inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {titleCase(status)}
    </span>
  );
}

// Cycle wall-clock duration in ms (raw sort value for the Duration column).
function cycleDurationMs(c: AutoflowCycleRow): number | null {
  const start = Date.parse(c.started_at);
  if (Number.isNaN(start)) return null;
  const end = c.finished_at ? Date.parse(c.finished_at) : Date.now();
  if (Number.isNaN(end)) return null;
  return Math.max(0, end - start);
}

/** Build the detail link for a launched-run event, including ?host= for
 *  remote runs. `undefined` when the event has no run (e.g. an awaiting /
 *  failed signal) — SortableTable leaves those rows unlinked. */
function eventHref(e: AutoflowEventRow): string | undefined {
  if (!e.run_id) return undefined;
  const hostSuffix = e.host_id && e.host_id !== 'local'
    ? `?host=${encodeURIComponent(e.host_id)}`
    : '';
  return `/runs/${encodeURIComponent(e.run_id)}${hostSuffix}`;
}

// ---------------------------------------------------------------------------
// Runs (launched-run events) columns — mirrors WorkflowRuns' column set
// (Workflow / Host / Status / token + cost breakdown / Started), plus the
// autoflow-specific Kind, Issue Ref, and Worker columns.
// ---------------------------------------------------------------------------

const EVENT_COLUMNS: Column<AutoflowEventRow>[] = [
  {
    key: 'workflow',
    header: 'Workflow',
    sortable: true,
    sortValue: (e) => e.workflow ?? KIND_LABEL[e.kind] ?? e.kind.replace(/_/g, ' '),
    render: (e) => (
      <span className="text-sm font-medium text-ink truncate">
        {e.workflow ?? KIND_LABEL[e.kind] ?? e.kind.replace(/_/g, ' ')}
      </span>
    ),
  },
  {
    key: 'run',
    header: 'Run',
    render: (e) => (
      <span className="text-note text-ink-mute font-mono">
        {e.run_id ? shortId(e.run_id) : '—'}
      </span>
    ),
  },
  {
    key: 'kind',
    header: 'Kind',
    sortable: true,
    sortValue: (e) => e.kind,
    render: (e) => <KindBadge kind={e.kind} />,
  },
  {
    key: 'issue',
    header: 'Issue Ref',
    sortable: true,
    sortValue: (e) => e.issue_display_ref ?? null,
    render: (e) =>
      e.issue_display_ref ? (
        <IssueChip displayRef={e.issue_display_ref} />
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'host',
    header: 'Host',
    sortable: true,
    sortValue: (e) => e.host_id ?? 'local',
    render: (e) => (
      <span className="text-note text-ink-mute font-mono">{e.host_id ?? 'local'}</span>
    ),
  },
  {
    key: 'worker',
    header: 'Worker',
    sortable: true,
    sortValue: (e) => e.worker_name ?? null,
    render: (e) =>
      e.worker_name ? (
        <span className="text-ink-dim">{e.worker_name}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (e) => e.status ?? null,
    render: (e) =>
      e.status ? (
        <span className="text-ink-dim">{e.status}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'in',
    header: 'In',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (e) => e.usage.input_tokens,
    render: (e) => <span className="text-ink-dim">{formatTokens(e.usage.input_tokens)}</span>,
  },
  {
    key: 'out',
    header: 'Out',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (e) => e.usage.output_tokens,
    render: (e) => <span className="text-ink-dim">{formatTokens(e.usage.output_tokens)}</span>,
  },
  {
    key: 'cached',
    header: 'Cached',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (e) => e.usage.cached_tokens,
    render: (e) =>
      e.usage.cached_tokens ? (
        <span className="text-ink-dim">{formatTokens(e.usage.cached_tokens)}</span>
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
    sortValue: (e) => e.usage.cost_usd,
    render: (e) => <span className="text-ink font-medium">{formatCost(e.usage.cost_usd)}</span>,
  },
  {
    key: 'started',
    header: 'Started',
    align: 'right',
    width: 'w-28',
    sortable: true,
    sortValue: (e) => (e.at ? Date.parse(e.at) : null),
    render: (e) => <span className="text-ink-mute">{relativeTime(e.at)}</span>,
  },
];

// ---------------------------------------------------------------------------
// Cycles columns
// ---------------------------------------------------------------------------

const CYCLE_COLUMNS: Column<AutoflowCycleRow>[] = [
  {
    key: 'cycle',
    header: 'Cycle',
    sortable: true,
    sortValue: (c) => c.cycle_id,
    render: (c) => <span className="text-sm font-medium text-ink font-mono">{shortId(c.cycle_id)}</span>,
  },
  {
    key: 'mode',
    header: 'Mode',
    sortable: true,
    sortValue: (c) => c.mode,
    render: (c) => <ModeChip mode={c.mode} />,
  },
  {
    key: 'worker',
    header: 'Worker',
    sortable: true,
    sortValue: (c) => c.worker_name ?? null,
    render: (c) =>
      c.worker_name ? (
        <span className="text-ink-dim">{c.worker_name}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
  {
    key: 'started',
    header: 'Started',
    sortable: true,
    sortValue: (c) => (c.started_at ? Date.parse(c.started_at) : null),
    render: (c) => <span className="text-ink-mute">{relativeTime(c.started_at)}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    align: 'right',
    width: 'w-24',
    sortable: true,
    sortValue: (c) => cycleDurationMs(c),
    render: (c) => <span className="text-ink-dim">{durationBetween(c.started_at, c.finished_at)}</span>,
  },
  {
    key: 'ran',
    header: 'Ran',
    align: 'right',
    width: 'w-16',
    sortable: true,
    sortValue: (c) => c.ran_cycles,
    render: (c) => <span className="text-ink">{c.ran_cycles}</span>,
  },
  {
    key: 'skipped',
    header: 'Skipped',
    align: 'right',
    width: 'w-20',
    sortable: true,
    sortValue: (c) => c.skipped_cycles,
    render: (c) => <span className="text-ink-dim">{c.skipped_cycles}</span>,
  },
  {
    key: 'failed',
    header: 'Failed',
    align: 'right',
    width: 'w-16',
    sortable: true,
    sortValue: (c) => c.failed_cycles,
    render: (c) => (
      <span className={c.failed_cycles > 0 ? 'text-err font-medium' : 'text-ink-dim'}>
        {c.failed_cycles}
      </span>
    ),
  },
  {
    key: 'usage',
    header: 'Usage',
    align: 'right',
    render: (c) => <UsageChip usage={c.usage} />,
  },
  {
    key: 'host',
    header: 'Host',
    sortable: true,
    sortValue: (c) => c.host_id ?? 'local',
    render: (c) => (
      <span className="text-note text-ink-mute font-mono">{c.host_id ?? 'local'}</span>
    ),
  },
];

// Expandable detail: the run ids spawned by this cycle, each linking to its
// run graph (preserves the linkage the bespoke list rendered inline).
// Appends ?host= when the cycle originated on a remote host.
function CycleDetail(c: AutoflowCycleRow) {
  if (c.run_ids.length === 0) {
    return <span className="text-note text-ink-mute">No runs launched in this cycle.</span>;
  }
  const hostSuffix = c.host_id && c.host_id !== 'local'
    ? `?host=${encodeURIComponent(c.host_id)}`
    : '';
  return (
    <div className="flex items-center gap-1.5 flex-wrap">
      <span className="text-meta text-ink-mute uppercase tracking-wide">runs:</span>
      {c.run_ids.map((rid) => (
        <Link
          key={rid}
          to={`/runs/${encodeURIComponent(rid)}${hostSuffix}`}
          className="text-note font-mono text-brand-600 hover:underline"
        >
          {shortId(rid)}
        </Link>
      ))}
    </div>
  );
}

type Tab = 'runs' | 'cycles' | 'claims';

export default function AutoflowRuns() {
  const [tab, setTab] = useState<Tab>('runs');
  const [events, setEvents] = useState<AutoflowEventRow[] | null>(null);
  const [cycles, setCycles] = useState<AutoflowCycleRow[] | null>(null);
  const [eventsHasMore, setEventsHasMore] = useState(true);
  const [cyclesHasMore, setCyclesHasMore] = useState(true);
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

  // Reset lists to null when the host filter changes so the poll guard allows
  // the first page-0 result through (guard reads prev == null as "allow reset").
  useEffect(() => {
    setEvents(null);
    setCycles(null);
  }, [hostFilter]);

  // Claims tab: lazily fetched on selection (cancel-guarded). `null` = loading.
  const [claims, setClaims] = useState<AutoflowClaim[] | null>(null);
  const [claimsError, setClaimsError] = useState<string | null>(null);

  // Manual refetch after a row action mutates the claim set.
  const refetchClaims = useCallback(async () => {
    try {
      const rows = await api.getAutoflowClaims();
      setClaims(rows);
      setClaimsError(null);
    } catch (e) {
      setClaimsError(e instanceof Error ? e.message : 'Failed to load autoflow claims');
    }
  }, []);

  useEffect(() => {
    if (tab !== 'claims') return;
    let cancelled = false;
    setClaims(null);
    setClaimsError(null);
    void (async () => {
      try {
        const rows = await api.getAutoflowClaims();
        if (!cancelled) setClaims(rows);
      } catch (e) {
        if (!cancelled) {
          setClaimsError(e instanceof Error ? e.message : 'Failed to load autoflow claims');
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tab]);

  // Page-0 fetch (mount + 5 s refresh). Only replaces a list when the user
  // hasn't scroll-extended past page 0 — otherwise the poll would discard
  // accumulated pages and cause the reset/regrow flicker.
  const refresh = useCallback(async () => {
    setRefreshing(true);
    const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
    try {
      const [ev, cy] = await Promise.all([
        api.getAutoflowEvents({ limit: PAGE, host }),
        api.getAutoflowRuns({ limit: PAGE, host }),
      ]);
      // Functional setState so the guard reads the CURRENT length, not a
      // stale closure (refresh is memoised with [] deps).
      setEvents((prev) => {
        if (prev == null || prev.length <= PAGE) {
          setEventsHasMore(ev.length >= PAGE);
          return ev;
        }
        return prev;
      });
      setCycles((prev) => {
        if (prev == null || prev.length <= PAGE) {
          setCyclesHasMore(cy.length >= PAGE);
          return cy;
        }
        return prev;
      });
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load autoflow activity');
    } finally {
      setRefreshing(false);
    }
  }, [hostFilter]);

  useEffect(() => {
    void refresh();
    const t = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

  // Two independent infinite lists: the primary "Runs" (events) feed and the
  // "Cycles" feed each get their own pagination state + sentinel.
  const loadMoreEvents = async () => {
    const current = events ?? [];
    const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
    const next = await api.getAutoflowEvents({ offset: current.length, limit: PAGE, host });
    if (next.length === 0) { setEventsHasMore(false); return; }
    setEvents([...current, ...next]);
    if (next.length < PAGE) setEventsHasMore(false);
  };

  const loadMoreCycles = async () => {
    const current = cycles ?? [];
    const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
    const next = await api.getAutoflowRuns({ offset: current.length, limit: PAGE, host });
    if (next.length === 0) { setCyclesHasMore(false); return; }
    setCycles([...current, ...next]);
    if (next.length < PAGE) setCyclesHasMore(false);
  };

  const { sentinelRef: eventsSentinelRef, loading: eventsLoading } =
    useInfiniteScroll({ hasMore: eventsHasMore, loadMore: loadMoreEvents });
  const { sentinelRef: cyclesSentinelRef, loading: cyclesLoading } =
    useInfiniteScroll({ hasMore: cyclesHasMore, loadMore: loadMoreCycles });

  const eventRows = events ?? [];
  const cycleRows = cycles ?? [];

  // Claims columns close over refetchClaims for the row actions.
  const claimColumns: Column<AutoflowClaim>[] = [
    {
      key: 'issue',
      header: 'Issue Ref',
      sortable: true,
      sortValue: (c) => c.issue_display_ref ?? c.issue_ref,
      render: (c) => {
        const label = c.issue_display_ref ?? c.issue_ref;
        return (
          <div className="min-w-0">
            {c.issue_url ? (
              <a
                href={c.issue_url}
                target="_blank"
                rel="noreferrer"
                className="text-sm font-medium text-brand-600 hover:underline truncate"
              >
                {label}
              </a>
            ) : (
              <span className="text-sm font-medium text-ink truncate">{label}</span>
            )}
            {c.issue_title && (
              <div className="text-note text-ink-dim mt-0.5 truncate">{c.issue_title}</div>
            )}
            {c.last_error ? (
              <div className="text-note text-err mt-0.5">{c.last_error}</div>
            ) : (
              c.last_summary && (
                <div className="text-note text-ink-dim mt-0.5">{c.last_summary}</div>
              )
            )}
            {c.pr_url && (
              <a
                href={c.pr_url}
                target="_blank"
                rel="noreferrer"
                className="inline-block text-note text-brand-600 hover:underline mt-1"
              >
                View PR
              </a>
            )}
          </div>
        );
      },
    },
    {
      key: 'status',
      header: 'Status',
      sortable: true,
      sortValue: (c) => c.status,
      render: (c) => <ClaimStatusBadge status={c.status} />,
    },
    {
      key: 'workflow',
      header: 'Workflow',
      sortable: true,
      sortValue: (c) => c.workflow,
      render: (c) => <IssueChip displayRef={c.workflow} />,
    },
    {
      key: 'repo',
      header: 'Repo',
      sortable: true,
      sortValue: (c) => c.repo_ref,
      render: (c) => <span className="text-note text-ink-dim">{c.repo_ref}</span>,
    },
    {
      key: 'owner',
      header: 'Owner',
      sortable: true,
      sortValue: (c) => c.claim_owner ?? null,
      render: (c) =>
        c.claim_owner ? (
          <span className="text-note text-ink-dim">{c.claim_owner}</span>
        ) : (
          <span className="text-ink-mute">—</span>
        ),
    },
    {
      key: 'updated',
      header: 'Updated',
      sortable: true,
      sortValue: (c) => (c.updated_at ? Date.parse(c.updated_at) : null),
      render: (c) => <span className="text-ink-mute">{relativeTime(c.updated_at)}</span>,
    },
    {
      key: 'actions',
      header: 'Actions',
      align: 'right',
      render: (c) => <ClaimActions claim={c} onChanged={() => void refetchClaims()} />,
    },
  ];

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflows</h1>
          <p className="mt-1 text-sm text-ink-dim">Runs launched by the autoflow worker across this control plane.</p>
        </div>
        <Button variant="secondary" onClick={() => void refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      <div className="mb-4 inline-flex rounded-md border border-border bg-panel p-0.5 text-xs font-medium">
        <button
          onClick={() => setTab('runs')}
          className={cn(
            'px-3 py-1 rounded',
            tab === 'runs' ? 'bg-surface text-ink' : 'text-ink-dim hover:text-ink',
          )}
        >
          Runs
        </button>
        <button
          onClick={() => setTab('cycles')}
          className={cn(
            'px-3 py-1 rounded',
            tab === 'cycles' ? 'bg-surface text-ink' : 'text-ink-dim hover:text-ink',
          )}
        >
          Cycles
        </button>
        <button
          onClick={() => setTab('claims')}
          className={cn(
            'px-3 py-1 rounded',
            tab === 'claims' ? 'bg-surface text-ink' : 'text-ink-dim hover:text-ink',
          )}
        >
          Claims
        </button>
      </div>

      {/* Host filter — shown for runs and cycles tabs; claims are always local. */}
      {tab !== 'claims' && (
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
      )}

      {error && (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      {tab === 'runs' ? (
        events === null ? (
          <div className="text-sm text-ink-dim">Loading autoflow activity…</div>
        ) : eventRows.length === 0 ? (
          <AutoflowEventsEmpty />
        ) : (
          <section>
            <SectionHeader tone="muted" label="Runs" count={eventRows.length} />
            <SortableTable<AutoflowEventRow>
              columns={EVENT_COLUMNS}
              rows={eventRows}
              rowKey={(e) => e.event_id}
              rowHref={eventHref}
              initialSort={{ key: 'started', dir: 'desc' }}
            />
            <div ref={eventsSentinelRef} className="py-2 text-center text-note text-ink-mute">
              {eventsLoading ? 'loading more…' : eventsHasMore ? 'scroll for more' : `— end of ${eventRows.length} —`}
            </div>
          </section>
        )
      ) : tab === 'cycles' ? (
        cycles === null ? (
          <div className="text-sm text-ink-dim">Loading autoflow cycles…</div>
        ) : cycleRows.length === 0 ? (
          <AutoflowCyclesEmpty />
        ) : (
          <section>
            <SectionHeader tone="muted" label="Cycles" count={cycleRows.length} />
            <SortableTable<AutoflowCycleRow>
              columns={CYCLE_COLUMNS}
              rows={cycleRows}
              rowKey={(c) => c.cycle_id}
              initialSort={{ key: 'started', dir: 'desc' }}
              renderDetail={CycleDetail}
            />
            <div ref={cyclesSentinelRef} className="py-2 text-center text-note text-ink-mute">
              {cyclesLoading ? 'loading more…' : cyclesHasMore ? 'scroll for more' : `— end of ${cycleRows.length} —`}
            </div>
          </section>
        )
      ) : claimsError ? (
        <div className="mb-4 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {claimsError}
        </div>
      ) : claims === null ? (
        <div className="text-sm text-ink-dim">Loading autoflow claims…</div>
      ) : claims.length === 0 ? (
        <AutoflowClaimsEmpty />
      ) : (
        <section>
          <SectionHeader tone="muted" label="Claims" count={claims.length} />
          <SortableTable<AutoflowClaim>
            columns={claimColumns}
            rows={claims}
            rowKey={(c) => c.issue_ref}
            initialSort={{ key: 'updated', dir: 'desc' }}
          />
        </section>
      )}
    </div>
  );
}

function ClaimActions({
  claim,
  onChanged,
}: {
  claim: AutoflowClaim;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<'requeue' | 'release' | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [requeued, setRequeued] = useState(false);

  const onRequeue = async () => {
    if (busy) return;
    if (!window.confirm('Requeue this autoflow?')) return;
    setBusy('requeue');
    setActionError(null);
    try {
      await api.requeueClaim(claim.issue_ref);
      setRequeued(true);
      onChanged();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Failed to requeue claim');
    } finally {
      setBusy(null);
    }
  };

  const onRelease = async () => {
    if (busy) return;
    if (!window.confirm('Release this claim?')) return;
    setBusy('release');
    setActionError(null);
    try {
      await api.releaseClaim(claim.issue_ref);
      onChanged();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Failed to release claim');
      setBusy(null);
    }
  };

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex items-center gap-2 shrink-0">
        {requeued && (
          <span className="text-meta font-medium text-ok">requeued</span>
        )}
        <Button variant="secondary" size="sm" onClick={() => void onRequeue()} disabled={busy !== null}>
          {busy === 'requeue' ? 'Requeuing…' : 'Requeue'}
        </Button>
        <Button
          variant="danger-outline"
          size="sm"
          onClick={() => void onRelease()}
          disabled={busy !== null}
        >
          {busy === 'release' ? 'Releasing…' : 'Release'}
        </Button>
      </div>
      {actionError && (
        <div role="alert" className="text-note text-err">
          {actionError}
        </div>
      )}
    </div>
  );
}

function AutoflowClaimsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No active claims</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Issues the autoflow worker has leased will appear here, each with requeue and release controls.
      </p>
    </div>
  );
}

function AutoflowEventsEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No autoflow activity yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Runs launched by the autoflow worker will appear here, each linking to its run graph.
      </p>
    </div>
  );
}

function AutoflowCyclesEmpty() {
  return (
    <div className="rounded-xl border border-dashed border-border bg-panel/50 py-16 flex flex-col items-center justify-center text-center">
      <div className="w-12 h-12 rounded-full bg-surface flex items-center justify-center mb-3">
        <Inbox size={20} className="text-ink-mute" />
      </div>
      <h2 className="text-sm font-medium text-ink">No autoflow cycles yet</h2>
      <p className="mt-1 text-xs text-ink-dim max-w-xs">
        Autoflow scheduling cycles will appear here once the autoflow worker runs.
      </p>
    </div>
  );
}
