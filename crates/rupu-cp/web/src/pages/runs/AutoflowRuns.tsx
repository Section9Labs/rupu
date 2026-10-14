// Autoflow run-stream page — ONE Workflows-shaped table (spec 2026-10-14, CP
// autoflow runs as one table): the workflow runs an autoflow launched plus
// one failed row per attempt that never produced a recorded run, newest
// first, row click → /runs/:id. The Cycles and Claims tables are gone (per-
// issue cycle history is the run page's Cycles tab; claim actions live on
// the run page and on an attempt row that has no run to open).
//
// Structure is `WorkflowRuns.tsx`'s — same components, header, spacing — with
// the Workflows columns (`WORKFLOW_RUN_COLUMNS`) except: the Workflow name is
// a button that narrows the table to that autoflow (a removable chip in the
// filter bar), an attempt row shows its error under the name, an Issue
// column follows (display ref → tracker, `PR ↗` when the run opened one),
// and no trigger pills (every row is cron/event). An attempt row carries
// `attempt`; one whose `id` is a history event id (no `run_recorded`)
// expands in place — the error, cycle, worker, claim status, Requeue /
// Release for its claim (local host only, as the claim endpoints are) —
// while one whose `id` is an unrecorded run id (`run_recorded: false`) links
// to the run page, which renders unpersisted runs from history.
//
// Fetch/paginate/poll is owned by `usePerHostPagedList` (per-host
// progressive loading): All hosts by default, local paints at once and each
// remote merges in as it answers; a host that can't report `autoflow` (501)
// is an unavailable slice. Archived uses the local archive, narrowed
// client-side to rows with a non-null `autoflow`. A problems strip
// (`AutoflowProblems`) sits between the filter bar and the table.
//
// Customer scope (customers Plan 2B): as Workflows, through the same per-host
// engine (`?customer=<slug>`). Attempt rows record no customer, so the server
// lists none while scoped; the page says so in one line.

import { useCallback, useState } from 'react';
import { RefreshCw, X } from 'lucide-react';
import { api, type AutoflowRunRow } from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import { WORKFLOW_RUN_COLUMNS } from '../../components/runs/workflowRunColumns';
import { AutoflowProblems } from '../../components/runs/AutoflowProblems';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { CrewChip } from '../../components/codename/CrewChip';
import { parseCodename } from '../../lib/codename';
import { Button } from '../../components/ui/Button';
import { FilterBar } from '../../components/ui/FilterBar';
import { FilterPills, type FilterPillOption } from '../../components/ui/FilterPills';
import { SearchInput } from '../../components/ui/SearchInput';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import HostSelect, { ALL_HOSTS } from '../../components/HostSelect';
import { usePerHostPagedList, type PerHostFetchParams } from '../../lib/perHost/usePerHostPagedList';
import { PerHostFooter, PerHostStrip, perHostFooterText } from '../../components/lists/PerHostStatus';
import { noHostAnswered, notIncluded, waitingLabel } from '../../lib/perHost/status';
import { cn } from '../../lib/cn';
import { runHref } from '../../lib/runs';
import { shortId } from '../../lib/shortId';
import { useRunCustomers, withCustomerColumn } from '../../components/customers/RunCustomer';
import { HostsWithoutCustomerBanner } from '../../components/customers/HostsWithoutCustomerBanner';
import { useScopedList } from '../../lib/useScopedList';

/** The server's `lifecycle` values plus `all` (no `lifecycle` sent): attempts
 *  are listed under All and Failed only, so All is the default. */
type Tab = 'all' | 'active' | 'completed' | 'failed';
type Lifecycle = Tab | 'archived';

const LIFECYCLE_OPTIONS: FilterPillOption[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Running' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed / Rejected' },
  { value: 'archived', label: 'Archived' },
];
/** While scoped to a customer: the archive has no customer filter. */
const SCOPED_LIFECYCLE_OPTIONS = LIFECYCLE_OPTIONS.filter((o) => o.value !== 'archived');

/** A row's host for actions: the server tags every row; absent means local. */
const hostOf = (r: AutoflowRunRow) => r.host_id ?? 'local';

/** An attempt whose `id` is a history event id: nothing to open, expands in place. */
const expandsInPlace = (r: AutoflowRunRow) => r.attempt !== undefined && r.run_recorded === undefined;

export default function AutoflowRuns() {
  const [tab, setTab] = useState<Tab>('all');
  const [archivedPicked, setArchived] = useState(false);
  const [workflow, setWorkflow] = useState<string | null>(null);
  const [hostFilter, setHostFilter] = useState<string>(ALL_HOSTS);
  const scoped = useScopedList(undefined, [tab, hostFilter, workflow]);
  const { customer } = scoped;
  const archived = archivedPicked && !customer;
  const [actionError, setActionError] = useState<string | null>(null);
  const [busyClaim, setBusyClaim] = useState<string | null>(null);
  const [query, setQuery] = useState('');

  const fetchRows = useCallback(
    ({ host, offset, limit, signal }: PerHostFetchParams): Promise<AutoflowRunRow[]> => {
      if (archived) {
        // /api/runs/archived has no offset/limit and no autoflow filter: one
        // fetch, narrowed here; later pages are empty so the host settles.
        return offset === 0
          ? api.getArchivedRuns('workflow').then((rows) =>
              rows.filter((r) => r.autoflow && (!workflow || r.workflow_name === workflow)),
            )
          : Promise.resolve([]);
      }
      const lifecycle = tab === 'all' ? undefined : tab;
      const wf = workflow ?? undefined;
      if (!customer) return api.getAutoflowRunList({ lifecycle, offset, limit, host, signal, workflow: wf });
      return scoped.guard(
        api.getAutoflowRunList({
          lifecycle,
          offset,
          limit,
          host,
          signal,
          workflow: wf,
          customer,
          onHostsWithoutCustomer: scoped.reportHosts(host, offset),
        }),
      );
    },
    [archived, tab, workflow, customer, scoped.guard, scoped.reportHosts],
  );

  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended } =
    usePerHostPagedList<AutoflowRunRow>({
      host: archived ? 'local' : hostFilter === ALL_HOSTS ? null : hostFilter,
      fetch: fetchRows,
      timeField: 'started_at',
      idField: 'id',
      deps: [archived, tab, workflow, customer],
      poll: !archived && tab === 'active',
    });

  // Find — case-insensitive substring over what this table renders: workflow
  // name, run id, host, codename, the issue refs and an attempt's error.
  const q = query.trim().toLowerCase();
  const visible = q
    ? rows.filter((r) =>
        [
          r.workflow_name,
          r.id,
          r.host_id,
          r.codename,
          r.autoflow?.issue_display_ref,
          r.autoflow?.issue_ref,
          r.attempt?.detail,
        ]
          .filter((v): v is string => Boolean(v))
          .some((v) => v.toLowerCase().includes(q)),
      )
    : rows;

  const lifecycleValue: Lifecycle = archived ? 'archived' : tab;
  function handleLifecycleChange(v: string) {
    if (v === 'archived') {
      setArchived(true);
    } else {
      setArchived(false);
      setTab(v as Tab);
    }
  }

  async function handleRowArchive(id: string, host: string) {
    try {
      await api.archiveRun(id, host);
      setActionError(null);
      removeRow(host, id);
      refreshHost(host);
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Archive failed');
    }
  }

  async function handleRowRestore(id: string, host: string) {
    try {
      await api.restoreRun(id, host);
      setActionError(null);
      removeRow(host, id);
      refreshHost(host);
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Restore failed');
    }
  }

  async function handleRowDelete(id: string, host: string) {
    if (!window.confirm('Permanently delete this run and its transcripts? This cannot be undone.')) return;
    try {
      await api.deleteRun(id, host);
      setActionError(null);
      removeRow(host, id);
      refreshHost(host);
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  // Claim actions on an expanded attempt row (local host only, as the claim
  // endpoints are). The confirmation wording is the old Claims table's.
  async function handleClaim(issueRef: string, action: 'requeue' | 'release') {
    if (busyClaim) return;
    if (!window.confirm(action === 'requeue' ? 'Requeue this autoflow?' : 'Release this claim?')) return;
    setBusyClaim(issueRef);
    try {
      if (action === 'requeue') await api.requeueClaim(issueRef);
      else await api.releaseClaim(issueRef);
      setActionError(null);
      refreshHost('local');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : `Failed to ${action} claim`);
    } finally {
      setBusyClaim(null);
    }
  }

  // The Workflows columns, with the Workflow column swapped for one whose name
  // narrows the table (and carries an attempt's error), and an Issue column
  // after it. Both render their own controls/links, so they are unwrapped
  // (`interactive`) cells rather than part of the row link.
  const workflowColumn: Column<AutoflowRunRow> = {
    key: 'workflow',
    header: 'Workflow',
    subject: true,
    sortable: true,
    interactive: true,
    sortValue: (r) => r.workflow_name,
    titleValue: (r) => r.workflow_name,
    render: (r) => (
      <>
        <span className="inline-flex items-center gap-2">
          {r.codename && <CrewChip crew={parseCodename(r.codename).crew} derived={r.codename_derived} />}
          <button
            type="button"
            className="text-sm font-medium text-ink hover:underline"
            title={`Only ${r.workflow_name}`}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              setWorkflow(r.workflow_name);
            }}
          >
            {r.workflow_name}
          </button>
        </span>
        {r.attempt?.detail && <div className="text-note text-err truncate">{r.attempt.detail}</div>}
      </>
    ),
  };
  const issueColumn: Column<AutoflowRunRow> = {
    key: 'issue',
    header: 'Issue',
    fit: true,
    interactive: true,
    sortable: true,
    sortValue: (r) => r.autoflow?.issue_display_ref ?? r.autoflow?.issue_ref ?? null,
    render: (r) => {
      const af = r.autoflow;
      const ref = af?.issue_display_ref ?? af?.issue_ref;
      if (!ref && !af?.pr_url) return <span className="text-ink-mute">—</span>;
      return (
        <span className="inline-flex items-center gap-2 text-note">
          {ref &&
            (af?.issue_url ? (
              <a
                href={af.issue_url}
                target="_blank"
                rel="noreferrer"
                className="text-brand-600 hover:underline"
                onClick={(e) => e.stopPropagation()}
              >
                {ref}
              </a>
            ) : (
              <span className="text-ink-dim">{ref}</span>
            ))}
          {af?.pr_url && (
            <a
              href={af.pr_url}
              target="_blank"
              rel="noreferrer"
              className="text-brand-600 hover:underline"
              onClick={(e) => e.stopPropagation()}
            >
              PR ↗
            </a>
          )}
        </span>
      );
    },
  };

  // Archive / Restore / Delete — run rows only; an attempt is not a run record.
  const actionColumn: Column<AutoflowRunRow> = {
    key: 'actions',
    header: '',
    fit: true,
    interactive: true,
    render: (r) =>
      r.attempt ? null : (
        <div
          className="flex items-center justify-end gap-1"
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          {archived ? (
            <Button variant="ring" onClick={() => void handleRowRestore(r.id, hostOf(r))} aria-label={`Restore run ${r.id}`}>
              Restore
            </Button>
          ) : (
            <Button variant="ring" onClick={() => void handleRowArchive(r.id, hostOf(r))} aria-label={`Archive run ${r.id}`}>
              Archive
            </Button>
          )}
          <Button variant="ring-danger" onClick={() => void handleRowDelete(r.id, hostOf(r))} aria-label={`Delete run ${r.id}`}>
            Delete
          </Button>
        </div>
      ),
  };

  // An attempt never ran: its usage, turns and duration are not zero, they
  // don't exist. Those cells show a dash instead of the server's zero fill.
  const NO_USAGE_KEYS = new Set(['in', 'out', 'cached', 'cost', 'turns', 'duration']);
  const dashForAttempts = (c: Column<AutoflowRunRow>): Column<AutoflowRunRow> =>
    NO_USAGE_KEYS.has(c.key)
      ? {
          ...c,
          sortValue: (r) => (r.attempt ? null : c.sortValue?.(r) ?? null),
          render: (r) => (r.attempt ? <span className="text-ink-mute">—</span> : c.render(r)),
        }
      : c;

  const customers = useRunCustomers(!customer && !archived ? rows.map((r) => r.customer) : []);
  const baseColumns = (WORKFLOW_RUN_COLUMNS as Column<AutoflowRunRow>[]).flatMap((c) =>
    c.key === 'workflow' ? [workflowColumn, issueColumn] : [dashForAttempts(c)],
  );
  const columns: Column<AutoflowRunRow>[] = [
    ...withCustomerColumn(baseColumns, customers, rows, !customer && !archived),
    actionColumn,
  ];

  /** The in-place detail of an attempt that has no run to open. */
  function renderDetail(r: AutoflowRunRow) {
    if (!expandsInPlace(r) || !r.attempt) return null;
    const a = r.attempt;
    const issueRef = r.autoflow?.issue_ref;
    const local = hostOf(r) === 'local';
    return (
      <div className="space-y-2">
        {a.detail && <pre className="text-note font-mono whitespace-pre-wrap text-err">{a.detail}</pre>}
        <dl className="flex flex-wrap gap-x-6 gap-y-1 text-note">
          <div className="flex gap-1.5">
            <dt className="text-ink-mute">cycle</dt>
            <dd className="font-mono text-ink-dim" title={a.cycle_id}>
              {shortId(a.cycle_id)}
            </dd>
          </div>
          <div className="flex gap-1.5">
            <dt className="text-ink-mute">worker</dt>
            <dd className="font-mono text-ink-dim">{a.worker ?? '—'}</dd>
          </div>
          <div className="flex gap-1.5">
            <dt className="text-ink-mute">claim</dt>
            <dd className="font-mono text-ink-dim">{a.claim_status ?? '—'}</dd>
          </div>
        </dl>
        {issueRef && local ? (
          <div className="flex items-center gap-1">
            <Button variant="ring" disabled={busyClaim !== null} onClick={() => void handleClaim(issueRef, 'requeue')}>
              Requeue claim
            </Button>
            <Button variant="ring-danger" disabled={busyClaim !== null} onClick={() => void handleClaim(issueRef, 'release')}>
              Release claim
            </Button>
          </div>
        ) : issueRef ? (
          <p className="text-note text-ink-mute">Claim actions are available on the host that ran this autoflow.</p>
        ) : null}
      </div>
    );
  }

  const bannerError = actionError ?? error;
  const waiting = waitingLabel(slices);
  const missing = notIncluded(slices);
  const listFooter = perHostFooterText({ slices, loading, hasMore, ended, count: rows.length, waitingShown: visible.length === 0 });
  const footer = (text: string) =>
    !archived && <PerHostFooter sentinelRef={sentinelRef} text={text} slices={slices} onRetry={retryPaging} />;
  const problemsHost = archived ? 'local' : hostFilter === ALL_HOSTS ? null : hostFilter;

  return (
    <div className="p-8">
      <header className="mb-6 flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Autoflow Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Workflow runs launched by autoflows, and attempts that failed before a run started.
          </p>
        </div>
        <Button variant="secondary" onClick={() => refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      <div className="mb-5">
        <FilterBar
          filters={
            <>
              <FilterPills
                options={customer ? SCOPED_LIFECYCLE_OPTIONS : LIFECYCLE_OPTIONS}
                value={lifecycleValue}
                onChange={handleLifecycleChange}
              />
              {workflow && (
                <Button
                  variant="ring"
                  className="gap-1"
                  onClick={() => setWorkflow(null)}
                  aria-label={`workflow: ${workflow} — remove filter`}
                >
                  workflow: {workflow}
                  <X size={11} aria-hidden />
                </Button>
              )}
            </>
          }
          search={
            <SearchInput
              aria-label="Find runs"
              placeholder="Find runs…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape') setQuery('');
              }}
            />
          }
          scope={
            !archived && (
              <HostSelect allowAll ariaLabel="Host filter" value={hostFilter} onChange={setHostFilter} />
            )
          }
        />
      </div>
      {!archived && <PerHostStrip slices={slices} />}
      {customer && (
        <>
          <HostsWithoutCustomerBanner
            className="mb-4"
            hosts={slices.map((sl) => ({ id: sl.hostId, name: sl.name, state: sl.state, reason: sl.reason }))}
            without={scoped.hostsWithoutCustomer}
          />
          <p className="mb-4 text-note text-ink-mute">
            Failed attempts aren’t listed while a customer is selected — they record no customer. Archived runs aren’t
            listed either — the archive can’t be filtered by customer.
          </p>
        </>
      )}

      <AutoflowProblems host={problemsHost} />

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading runs…" />
        </div>
      ) : rows.length === 0 && waiting ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label={waiting} />
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          title={
            noHostAnswered(slices)
              ? 'No hosts answered'
              : missing
                ? 'No autoflow runs on the hosts that answered'
                : workflow
                  ? `No autoflow runs of ${workflow}`
                  : 'No autoflow runs yet'
          }
          hint={
            missing
              ? `Not included: ${missing}.`
              : customer === 'none'
                ? 'No autoflow runs without a customer yet.'
                : customer
                  ? 'No autoflow runs are attributed to this customer yet.'
                  : 'Runs will appear here once an autoflow launches a workflow, or an attempt to launch one fails.'
          }
        />
      ) : visible.length === 0 ? (
        <>
          {waiting ? (
            <div className="py-16 flex items-center justify-center">
              <Spinner label={`No matches yet · ${waiting}`} />
            </div>
          ) : (
            <EmptyState title="No matches" hint={`No runs match "${query}".`} />
          )}
          {footer(listFooter)}
        </>
      ) : (
        <div className="space-y-6">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
            <UsageBarChart
              bars={visible
                .filter((r) => !r.attempt)
                .map((r) => ({
                  id: r.id,
                  label: r.workflow_name,
                  to: runHref(r),
                  input_tokens: r.usage.input_tokens,
                  output_tokens: r.usage.output_tokens,
                  cached_tokens: r.usage.cached_tokens,
                  cost_usd: r.usage.cost_usd,
                }))}
            />
          </div>
          <SortableTable<AutoflowRunRow>
            columns={columns}
            rows={visible}
            rowKey={(r) => `${hostOf(r)}:${r.id}`}
            rowHref={(r) => (expandsInPlace(r) ? undefined : runHref(r))}
            renderDetail={renderDetail}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          {footer(q ? `${visible.length} matches of ${rows.length} loaded` : listFooter)}
        </div>
      )}
    </div>
  );
}
