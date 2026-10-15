// Autoflow run-stream page — ONE Workflows-shaped table (spec 2026-10-14, CP
// autoflow runs as one table): the workflow runs an autoflow launched plus
// one failed row per attempt that never produced a recorded run, newest
// first, row click → /runs/:id. The Cycles and Claims tables are gone (per-
// issue cycle history is the run page's Cycles tab; claim actions live on
// the run page and on an attempt row that has no run to open).
//
// Structure is `WorkflowRuns.tsx`'s — the query line, per-host loading, the
// same components and header — with the Workflows columns
// (`WORKFLOW_RUN_COLUMNS`) except: the Workflow cell carries the issue — or
// an attempt's error — as its second line, and the actions column adds a
// "show only this workflow" button (it puts `workflow:<name>` in the query)
// and the run's PR link. An attempt row carries `attempt`; one whose `id` is
// a history event id (no `run_recorded`) expands in place — the error,
// cycle, worker, claim status, Requeue / Release for its claim (local host
// only, as the claim endpoints are) — while one whose `id` is an unrecorded
// run id (`run_recorded: false`) links to the run page, which renders
// unpersisted runs from history.
//
// One query line filters the list and the server evaluates it (`?q=` on
// every request, `AUTOFLOW_RUN_FIELDS`: the workflow-run fields plus
// `issue:`, `pr:`, `kind:` and `error:`), `is:archived` switching to the
// local archive. Fetch/paginate/poll is owned by `usePerHostQueryList`:
// every host the query's `host:` terms allow, local painting at once and
// each remote merging in as it answers; a host that can't report `autoflow`
// (an older rupu) is named in the line under the bar. The problems strip
// (`AutoflowProblems`) sits between the bar and the table, over the same
// hosts.
//
// Customer scope (customers Plan 2B): as Workflows, through the same per-host
// engine (`?customer=<slug>`). Attempt rows record no customer, so the server
// lists none while scoped; the page says so in one line.

import { useCallback, useMemo, useState } from 'react';
import { ListFilter, RefreshCw } from 'lucide-react';
import { shortIssueRef } from '../../lib/issueRef';
import { api, type AutoflowRunRow, type ListQueryOpts } from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import { WORKFLOW_RUN_COLUMNS } from '../../components/runs/workflowRunColumns';
import { AutoflowProblems } from '../../components/runs/AutoflowProblems';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { CrewChip } from '../../components/codename/CrewChip';
import { parseCodename } from '../../lib/codename';
import { Button } from '../../components/ui/Button';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import { ListQueryBar, NoMatches, useListQuery } from '../../components/query/ListQueryBar';
import { hostAlerts } from '../../components/query/hostAlerts';
import { ListFooter } from '../../components/lists/ListFooter';
import { usePerHostQueryList } from '../../lib/query/usePerHostQueryList';
import { AUTOFLOW_RUN_FIELDS } from '../../lib/query/lists/autoflowRuns';
import { asksArchived } from '../../lib/query/lists/workflowRuns';
import { hostPickOf } from '../../lib/query/serverList';
import { withTerm } from '../../lib/findingQuery/shortcut';
import type { PerHostFetchParams } from '../../lib/perHost/usePerHostPagedList';
import { noHostAnswered, waitingLabel } from '../../lib/perHost/status';
import { cn } from '../../lib/cn';
import { runHref } from '../../lib/runs';
import { shortId } from '../../lib/shortId';
import { useRunCustomers, withCustomerColumn } from '../../components/customers/RunCustomer';
import { useScopedList } from '../../lib/useScopedList';

/** A row's host for actions: the server tags every row; absent means local. */
const hostOf = (r: AutoflowRunRow) => r.host_id ?? 'local';

/** An attempt whose `id` is a history event id: nothing to open, expands in place. */
const expandsInPlace = (r: AutoflowRunRow) => r.attempt !== undefined && r.run_recorded === undefined;

/** An attempt never ran: its usage, turns and duration are not zero, they
 *  don't exist. Those cells show a dash instead of the server's zero fill. */
const NO_USAGE_KEYS = new Set(['in', 'out', 'cached', 'cost', 'turns', 'duration']);

export default function AutoflowRuns() {
  const [q, setQ] = useListQuery('q');
  const scoped = useScopedList(undefined, []);
  const { customer } = scoped;
  // Row-action (archive/restore/delete, claim) failures — kept separate from
  // the list-fetch error the hook owns, but shown in the same banner.
  const [actionError, setActionError] = useState<string | null>(null);
  const [busyClaim, setBusyClaim] = useState<string | null>(null);
  const archive = asksArchived(q, AUTOFLOW_RUN_FIELDS);
  // The hosts the query asks, for the problems strip too.
  const pick = useMemo(() => hostPickOf(q, AUTOFLOW_RUN_FIELDS), [q]);

  const fetchRows = useCallback(
    (p: PerHostFetchParams & ListQueryOpts): Promise<AutoflowRunRow[]> =>
      customer ? scoped.guard(api.getAutoflowRunList({ ...p, customer })) : api.getAutoflowRunList(p),
    [customer, scoped.guard],
  );

  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended, facets, queryError, notes } =
    usePerHostQueryList<AutoflowRunRow>({
      q,
      fields: AUTOFLOW_RUN_FIELDS,
      scope: [customer],
      fetch: fetchRows,
      timeField: 'started_at',
      idField: 'id',
      poll: !archive,
    });

  // Row-level archive / restore / delete — each drops the row and re-syncs
  // just its host after success, on the host that owns the run.
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

  // The Workflows columns, unchanged in width: the Workflow cell (the row's
  // link and the table's one flexible column) carries the issue — or an
  // attempt's error — as a second line, and the "show only this workflow"
  // filter and the PR link ride the actions column. Extra columns here would
  // starve the Workflow column at ordinary widths (Workflows already leaves
  // it ~180px at 1600px).
  const workflowColumn: Column<AutoflowRunRow> = {
    key: 'workflow',
    header: 'Workflow',
    subject: true,
    sortable: true,
    sortValue: (r) => r.workflow_name,
    titleValue: (r) => {
      const issue = r.autoflow?.issue_display_ref ?? r.autoflow?.issue_ref;
      return issue ? `${r.workflow_name} · ${issue}` : r.workflow_name;
    },
    render: (r) => {
      const issue = r.autoflow?.issue_display_ref ?? r.autoflow?.issue_ref;
      return (
        <>
          <span className="inline-flex items-center gap-2">
            {r.codename && <CrewChip crew={parseCodename(r.codename).crew} derived={r.codename_derived} />}
            <span className="text-sm font-medium text-ink">{r.workflow_name}</span>
          </span>
          {r.attempt?.detail ? (
            <div className="text-note text-err truncate">{r.attempt.detail}</div>
          ) : (
            issue && <div className="text-note text-ink-mute font-mono truncate">{shortIssueRef(issue)}</div>
          )}
        </>
      );
    },
  };

  // Show only this workflow (a `workflow:` term in the query), the run's PR,
  // and Archive / Restore / Delete (run rows only — an attempt is not a run
  // record). Its own real buttons: independently focusable, not swallowed by
  // the row link; the wrapper cancels the link's navigation (I7).
  const actionColumn: Column<AutoflowRunRow> = {
    key: 'actions',
    header: '',
    fit: true,
    interactive: true,
    render: (r) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        <button
          type="button"
          className="rounded p-1 text-ink-mute hover:bg-surface-hover hover:text-ink"
          aria-label={`Show only ${r.workflow_name} runs`}
          title={`Show only ${r.workflow_name} runs`}
          onClick={() => setQ(withTerm(q, AUTOFLOW_RUN_FIELDS, 'workflow', [r.workflow_name]))}
        >
          <ListFilter size={12} />
        </button>
        {r.autoflow?.pr_url && (
          <a
            href={r.autoflow.pr_url}
            target="_blank"
            rel="noreferrer"
            className="px-1 text-note text-brand-600 hover:underline"
            onClick={(e) => e.stopPropagation()}
          >
            PR ↗
          </a>
        )}
        {!r.attempt && (
          <>
            {r.archived ? (
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
          </>
        )}
      </div>
    ),
  };

  const dashForAttempts = (c: Column<AutoflowRunRow>): Column<AutoflowRunRow> =>
    NO_USAGE_KEYS.has(c.key)
      ? {
          ...c,
          sortValue: (r) => (r.attempt ? null : c.sortValue?.(r) ?? null),
          render: (r) => (r.attempt ? <span className="text-ink-mute">—</span> : c.render(r)),
        }
      : c;

  // The Customer column resolves slugs (archived customers load on first need).
  const customers = useRunCustomers(!customer ? rows.map((r) => r.customer) : []);
  const baseColumns = (WORKFLOW_RUN_COLUMNS as Column<AutoflowRunRow>[]).flatMap((c) =>
    c.key === 'workflow' ? [workflowColumn] : [dashForAttempts(c)],
  );
  const columns: Column<AutoflowRunRow>[] = [
    ...withCustomerColumn(baseColumns, customers, rows, !customer),
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
          {issueRef && (
            <div className="flex gap-1.5">
              <dt className="text-ink-mute">issue</dt>
              <dd className="font-mono">
                {r.autoflow?.issue_url ? (
                  <a href={r.autoflow.issue_url} target="_blank" rel="noreferrer" className="text-brand-600 hover:underline">
                    {r.autoflow.issue_display_ref ?? issueRef}
                  </a>
                ) : (
                  <span className="text-ink-dim">{r.autoflow?.issue_display_ref ?? issueRef}</span>
                )}
              </dd>
            </div>
          )}
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

  // A fresh action error must win over a stale fetch error from an earlier
  // load, never the other way around.
  const bannerError = actionError ?? error;
  // Never claim "nothing" while a host is still loading.
  const waiting = waitingLabel(slices);

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

      <ListQueryBar
        value={q}
        onChange={setQ}
        fields={AUTOFLOW_RUN_FIELDS}
        facets={facets}
        label="Filter autoflow runs"
        placeholder="Filter runs… e.g. status:failed since:7d · kind:attempt · issue:acme/api#7 · pr:any"
        error={queryError}
        alerts={hostAlerts(notes, 'runs', retryPaging)}
      />

      {customer && (
        <p className="mb-4 text-note text-ink-mute">
          Failed attempts aren’t listed while a customer is selected — they record no customer.
        </p>
      )}

      <AutoflowProblems pick={pick} />

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading runs…" />
        </div>
      ) : rows.length === 0 && waiting ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label={waiting} />
        </div>
      ) : rows.length === 0 && noHostAnswered(slices) ? (
        <EmptyState title="No hosts answered" hint="Every host is offline or can't serve this list; the line under the query says which." />
      ) : rows.length === 0 ? (
        q.trim() ? (
          <NoMatches noun="autoflow runs" />
        ) : (
          <EmptyState
            title="No autoflow runs yet"
            hint={
              customer === 'none'
                ? 'No autoflow run without a customer yet. is:archived lists the archive.'
                : customer
                  ? 'No autoflow run is attributed to this customer yet. is:archived lists the archive.'
                  : 'Runs appear here once an autoflow launches a workflow, or an attempt to launch one fails. is:archived lists the archive.'
            }
          />
        )
      ) : (
        <div className="space-y-6">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
            <UsageBarChart
              bars={rows
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
            rows={rows}
            rowKey={(r) => `${hostOf(r)}:${r.id}`}
            rowHref={(r) => (expandsInPlace(r) ? undefined : runHref(r))}
            renderDetail={renderDetail}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          <ListFooter sentinelRef={sentinelRef} loading={loading} hasMore={hasMore} ended={ended} count={rows.length} />
        </div>
      )}
    </div>
  );
}
