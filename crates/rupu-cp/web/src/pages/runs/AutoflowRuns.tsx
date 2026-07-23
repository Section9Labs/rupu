// Autoflow run-stream page — mirrors Runs → Workflows: the PRIMARY view is a
// clean run list (same SortableTable shape, row click → /runs/:id). The
// existing "Cycles" (batch view) and "Claims" (requeue/release) views are
// kept as secondary tabs, functionality untouched.
//
// One Control Language migration (Phase 2, Task E): the Runs/Cycles/Claims
// strip is now `Segmented` in FilterBar's view slot; the host filter is the
// shared `HostSelect allowAll`; fetch/paginate/poll for the Runs and Cycles
// tabs is owned by `usePagedList` (Claims stays a single lazily-fetched list,
// same as before — no server-side pagination for that endpoint). Table rules
// (fit/subject columns) applied to all three tables. Task A2's Event column,
// `cycle_failed` detail expansion, and whole-row nav on run rows are
// preserved verbatim.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { RefreshCw } from 'lucide-react';
import {
  api,
  type AutoflowClaim,
  type AutoflowCycleRow,
  type AutoflowEventRow,
} from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import UsageChip from '../../components/UsageChip';
import { Button } from '../../components/ui/Button';
import { FilterBar } from '../../components/ui/FilterBar';
import { Segmented, type SegmentedOption } from '../../components/ui/Segmented';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import HostSelect, { ALL_HOSTS } from '../../components/HostSelect';
import { RUN_STATUS_STYLES } from '../../components/StatusPill';
import { usePagedList } from '../../lib/usePagedList';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { shortId } from '../../lib/shortId';

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
    <span className={cn('inline-flex items-center whitespace-nowrap rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5', cls)}>
      {label}
    </span>
  );
}

// `cycle_failed` is a scheduling event, not a run outcome — it deliberately
// borrows the shared StatusPill "failed" tone (rather than the plain KIND_CLS
// treatment every other event kind uses) so it reads as visually distinct: a
// worker-level failure, never mistaken for a run's own status.
function CycleFailedPill() {
  const s = RUN_STATUS_STYLES.failed;
  const Icon = s.icon;
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 whitespace-nowrap rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5',
        s.cls,
      )}
    >
      <Icon size={10} />
      CYCLE FAILED
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

// A `run_id`-bearing event is a launched run — every other kind (awaiting /
// failed signals) is a scheduling event with nothing to show in the
// run-shaped columns (Run / Worker / Status / tokens / Cost): those render
// empty rather than a wall of "—" placeholders.
function isRunEvent(e: AutoflowEventRow): boolean {
  return Boolean(e.run_id);
}

// ---------------------------------------------------------------------------
// Runs (launched-run events) columns — mirrors WorkflowRuns' column set
// (Workflow / Host / Status / token + cost breakdown / Started), plus the
// autoflow-specific Event, Issue Ref, and Worker columns.
// ---------------------------------------------------------------------------

const EVENT_COLUMNS: Column<AutoflowEventRow>[] = [
  {
    key: 'workflow',
    header: 'Workflow',
    subject: true,
    sortable: true,
    sortValue: (e) => e.workflow ?? KIND_LABEL[e.kind] ?? e.kind.replace(/_/g, ' '),
    titleValue: (e) => e.workflow ?? KIND_LABEL[e.kind] ?? e.kind.replace(/_/g, ' '),
    // Plain content — row-level navigation is `rowHref` (SortableTable
    // link-wraps the whole row for non-expandable rows); an inline <Link>
    // here would nest an <a> inside SortableTable's own <a>.
    render: (e) => (
      <span className="text-sm font-medium text-ink">
        {e.workflow ?? KIND_LABEL[e.kind] ?? e.kind.replace(/_/g, ' ')}
      </span>
    ),
  },
  {
    key: 'run',
    header: 'Run',
    fit: true,
    render: (e) =>
      e.run_id ? (
        <span className="text-note text-ink-mute font-mono">{shortId(e.run_id)}</span>
      ) : null,
  },
  {
    key: 'kind',
    header: 'Event',
    fit: true,
    sortable: true,
    sortValue: (e) => e.kind,
    render: (e) => (e.kind === 'cycle_failed' ? <CycleFailedPill /> : <KindBadge kind={e.kind} />),
  },
  {
    key: 'issue',
    header: 'Issue Ref',
    fit: true,
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
    fit: true,
    sortable: true,
    sortValue: (e) => e.host_id ?? 'local',
    render: (e) => (
      <span className="text-note text-ink-mute font-mono">{e.host_id ?? 'local'}</span>
    ),
  },
  {
    key: 'worker',
    header: 'Worker',
    fit: true,
    sortable: true,
    sortValue: (e) => e.worker_name ?? null,
    render: (e) => {
      if (!isRunEvent(e)) return null;
      return e.worker_name ? (
        <span className="text-ink-dim">{e.worker_name}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      );
    },
  },
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (e) => e.status ?? null,
    render: (e) => {
      if (!isRunEvent(e)) return null;
      return e.status ? (
        <span className="text-ink-dim">{e.status}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      );
    },
  },
  {
    key: 'in',
    header: 'In',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (e) => e.usage.input_tokens,
    render: (e) =>
      isRunEvent(e) ? (
        <span className="text-ink-dim">{formatTokens(e.usage.input_tokens)}</span>
      ) : null,
  },
  {
    key: 'out',
    header: 'Out',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (e) => e.usage.output_tokens,
    render: (e) =>
      isRunEvent(e) ? (
        <span className="text-ink-dim">{formatTokens(e.usage.output_tokens)}</span>
      ) : null,
  },
  {
    key: 'cached',
    header: 'Cached',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (e) => e.usage.cached_tokens,
    render: (e) => {
      if (!isRunEvent(e)) return null;
      return e.usage.cached_tokens ? (
        <span className="text-ink-dim">{formatTokens(e.usage.cached_tokens)}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      );
    },
  },
  {
    key: 'cost',
    header: 'Cost',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (e) => e.usage.cost_usd,
    render: (e) =>
      isRunEvent(e) ? (
        <span className="text-ink font-medium">{formatCost(e.usage.cost_usd)}</span>
      ) : null,
  },
  {
    key: 'started',
    header: 'Started',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (e) => (e.at ? Date.parse(e.at) : null),
    render: (e) => <span className="text-ink-mute">{relativeTime(e.at)}</span>,
  },
];

// Expandable detail for a `cycle_failed` (or any `detail`-bearing) event —
// mirrors the Claims tab's `last_error` treatment: small mono text in the
// failed tone, indented in the full-width detail row SortableTable renders
// below the row. Rows without `detail` render nothing.
function EventDetail(e: AutoflowEventRow) {
  if (!e.detail) return null;
  return <div className="pl-1 text-note font-mono text-err">{e.detail}</div>;
}

// ---------------------------------------------------------------------------
// Cycles columns
// ---------------------------------------------------------------------------

// No column here is a natural free-text "subject" (unlike the Runs/Claims
// tables, a cycle has no single describing name — it spans `workflow_count`
// workflows) — every column is `fit`, matching table-rules §5 for tables with
// no dominant descriptive column.
const CYCLE_COLUMNS: Column<AutoflowCycleRow>[] = [
  {
    key: 'cycle',
    header: 'Cycle',
    fit: true,
    sortable: true,
    sortValue: (c) => c.cycle_id,
    render: (c) => <span className="text-sm font-medium text-ink font-mono">{shortId(c.cycle_id)}</span>,
  },
  {
    key: 'mode',
    header: 'Mode',
    fit: true,
    sortable: true,
    sortValue: (c) => c.mode,
    render: (c) => <ModeChip mode={c.mode} />,
  },
  {
    key: 'worker',
    header: 'Worker',
    fit: true,
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
    fit: true,
    sortable: true,
    sortValue: (c) => (c.started_at ? Date.parse(c.started_at) : null),
    render: (c) => <span className="text-ink-mute">{relativeTime(c.started_at)}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (c) => cycleDurationMs(c),
    render: (c) => <span className="text-ink-dim">{durationBetween(c.started_at, c.finished_at)}</span>,
  },
  {
    key: 'ran',
    header: 'Ran',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (c) => c.ran_cycles,
    render: (c) => <span className="text-ink">{c.ran_cycles}</span>,
  },
  {
    key: 'skipped',
    header: 'Skipped',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (c) => c.skipped_cycles,
    render: (c) => <span className="text-ink-dim">{c.skipped_cycles}</span>,
  },
  {
    key: 'failed',
    header: 'Failed',
    align: 'right',
    fit: true,
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
    fit: true,
    render: (c) => <UsageChip usage={c.usage} />,
  },
  {
    key: 'host',
    header: 'Host',
    fit: true,
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

const VIEW_OPTIONS: SegmentedOption[] = [
  { value: 'runs', label: 'Runs' },
  { value: 'cycles', label: 'Cycles' },
  { value: 'claims', label: 'Claims' },
];

export default function AutoflowRuns() {
  const [tab, setTab] = useState<Tab>('runs');
  // Default to 'local' → fast server-side path; ALL_HOSTS → fan-out.
  const [hostFilter, setHostFilter] = useState<string>('local');

  // The primary "Runs" (events) feed and the "Cycles" feed each get their own
  // usePagedList instance (independent pagination + sentinel), matching the
  // pre-migration two-list-machine shape. Both poll every 5s regardless of
  // the active tab — unchanged from before (so switching tabs always shows
  // fresh data, not a stale snapshot from before the tab was last visited).
  const events = usePagedList<AutoflowEventRow>({
    fetch: ({ offset, limit }) => {
      const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
      return api.getAutoflowEvents({ offset, limit, host });
    },
    deps: [hostFilter],
    poll: true,
  });

  const cycles = usePagedList<AutoflowCycleRow>({
    fetch: ({ offset, limit }) => {
      const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
      return api.getAutoflowRuns({ offset, limit, host });
    },
    deps: [hostFilter],
    poll: true,
  });

  // Claims tab: lazily fetched on selection, same as before — the endpoint
  // has no offset/limit (a single full-list fetch), so any page beyond the
  // first returns `[]` (mirrors the WorkflowRuns Archived-tab precedent) and
  // the hook settles as `ended` after that one page. No poll (unchanged).
  const claims = usePagedList<AutoflowClaim>({
    fetch: ({ offset }) => {
      if (tab !== 'claims') return Promise.resolve([]);
      if (offset > 0) return Promise.resolve([]);
      return api.getAutoflowClaims();
    },
    deps: [tab],
    poll: false,
  });

  // Claims columns close over claims.refresh for the row actions.
  const claimColumns: Column<AutoflowClaim>[] = [
    {
      key: 'issue',
      header: 'Issue Ref',
      subject: true,
      sortable: true,
      sortValue: (c) => c.issue_display_ref ?? c.issue_ref,
      titleValue: (c) => c.issue_display_ref ?? c.issue_ref,
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
      fit: true,
      sortable: true,
      sortValue: (c) => c.status,
      render: (c) => <ClaimStatusBadge status={c.status} />,
    },
    {
      key: 'workflow',
      header: 'Workflow',
      fit: true,
      sortable: true,
      sortValue: (c) => c.workflow,
      render: (c) => <IssueChip displayRef={c.workflow} />,
    },
    {
      key: 'repo',
      header: 'Repo',
      fit: true,
      sortable: true,
      sortValue: (c) => c.repo_ref,
      render: (c) => <span className="text-note text-ink-dim">{c.repo_ref}</span>,
    },
    {
      key: 'owner',
      header: 'Owner',
      fit: true,
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
      align: 'right',
      fit: true,
      sortable: true,
      sortValue: (c) => (c.updated_at ? Date.parse(c.updated_at) : null),
      render: (c) => <span className="text-ink-mute">{relativeTime(c.updated_at)}</span>,
    },
    {
      key: 'actions',
      header: 'Actions',
      align: 'right',
      fit: true,
      render: (c) => <ClaimActions claim={c} onChanged={() => claims.refresh()} />,
    },
  ];

  const bannerError = tab === 'runs' ? events.error : tab === 'cycles' ? cycles.error : claims.error;
  const refreshing = events.loading || cycles.loading;

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflows</h1>
          <p className="mt-1 text-sm text-ink-dim">Runs launched by the autoflow worker across this control plane.</p>
        </div>
        <Button
          variant="secondary"
          onClick={() => {
            events.refresh();
            cycles.refresh();
          }}
          className="gap-1.5"
        >
          <RefreshCw size={12} className={cn(refreshing && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      <div className="mb-5">
        <FilterBar
          view={
            <Segmented
              options={VIEW_OPTIONS}
              value={tab}
              onChange={(v) => setTab(v as Tab)}
              ariaLabel="View"
            />
          }
          scope={
            // Host filter — shown for runs and cycles tabs; claims are always local.
            tab !== 'claims' && (
              <HostSelect allowAll ariaLabel="Host filter" value={hostFilter} onChange={setHostFilter} />
            )
          }
        />
      </div>

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {tab === 'runs' ? (
        events.loading && events.rows.length === 0 ? (
          <div className="py-16 flex items-center justify-center">
            <Spinner label="Loading autoflow activity…" />
          </div>
        ) : events.rows.length === 0 ? (
          <EmptyState
            title="No autoflow activity yet"
            hint="Runs launched by the autoflow worker will appear here, each linking to its run graph."
          />
        ) : (
          <section>
            <SortableTable<AutoflowEventRow>
              columns={EVENT_COLUMNS}
              rows={events.rows}
              rowKey={(e) => e.event_id}
              rowHref={eventHref}
              initialSort={{ key: 'started', dir: 'desc' }}
              renderDetail={EventDetail}
            />
            <div ref={events.sentinelRef} className="py-2 text-center text-note text-ink-mute">
              {events.loading
                ? 'loading more…'
                : events.hasMore
                  ? 'scroll for more'
                  : `— end of ${events.rows.length} —`}
            </div>
          </section>
        )
      ) : tab === 'cycles' ? (
        cycles.loading && cycles.rows.length === 0 ? (
          <div className="py-16 flex items-center justify-center">
            <Spinner label="Loading autoflow cycles…" />
          </div>
        ) : cycles.rows.length === 0 ? (
          <EmptyState
            title="No autoflow cycles yet"
            hint="Autoflow scheduling cycles will appear here once the autoflow worker runs."
          />
        ) : (
          <section>
            <SortableTable<AutoflowCycleRow>
              columns={CYCLE_COLUMNS}
              rows={cycles.rows}
              rowKey={(c) => c.cycle_id}
              initialSort={{ key: 'started', dir: 'desc' }}
              renderDetail={CycleDetail}
            />
            <div ref={cycles.sentinelRef} className="py-2 text-center text-note text-ink-mute">
              {cycles.loading
                ? 'loading more…'
                : cycles.hasMore
                  ? 'scroll for more'
                  : `— end of ${cycles.rows.length} —`}
            </div>
          </section>
        )
      ) : claims.loading && claims.rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading autoflow claims…" />
        </div>
      ) : claims.rows.length === 0 ? (
        <EmptyState
          title="No active claims"
          hint="Issues the autoflow worker has leased will appear here, each with requeue and release controls."
        />
      ) : (
        <section>
          <SortableTable<AutoflowClaim>
            columns={claimColumns}
            rows={claims.rows}
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

// The three bespoke dashed-box empty states (Claims/Events/Cycles) are
// superseded by the kit `EmptyState` component (same copy, inlined at each
// call site above) per the One Control Language migration.
