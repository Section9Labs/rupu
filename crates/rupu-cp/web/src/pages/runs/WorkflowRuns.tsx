// Workflow run-stream page — execution history for workflow runs.
//
// One query line filters the list, and the server evaluates it (`?q=` on
// every request, `WORKFLOW_RUN_FIELDS`): `status:`, `trigger:`, `host:`,
// `since:` and the rest, with `is:archived` switching to the archive. With no
// query it lists every run, newest first.
//
// Fetch/paginate/poll is owned by `usePerHostQueryList` (per-host progressive
// loading, spec 2026-10-01): every host the query's `host:` terms allow,
// local painting at once and each remote merging in as it answers. Host
// health is in the `host:` suggestions and one quiet line under the bar.
//
// Customer scope (customers Plan 2B): the list follows the global scope
// (`useScopedList`), or — embedded in a customer's Runs tab — a fixed
// `customer` prop, which also drops the page header and padding, and keeps
// the query in `wq` so it doesn't follow the tab to Agent Runs. Either way
// `?customer=<slug>` ANDs with the query. A scope the backend rejects (400)
// is cleared with a notice.

import { useCallback, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { api, type RunListRow } from '../../lib/api';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import { WORKFLOW_RUN_COLUMNS } from '../../components/runs/workflowRunColumns';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { Button } from '../../components/ui/Button';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import { ListQueryBar, NoMatches, useListQuery } from '../../components/query/ListQueryBar';
import { hostAlerts } from '../../components/query/hostAlerts';
import { ListFooter } from '../../components/lists/ListFooter';
import { usePerHostQueryList } from '../../lib/query/usePerHostQueryList';
import { WORKFLOW_RUN_FIELDS, asksArchived } from '../../lib/query/lists/workflowRuns';
import type { PerHostFetchParams } from '../../lib/perHost/usePerHostPagedList';
import type { ListQueryOpts } from '../../lib/api';
import { noHostAnswered, waitingLabel } from '../../lib/perHost/status';
import { cn } from '../../lib/cn';
import { runHref } from '../../lib/runs';
import { useRunCustomers, withCustomerColumn } from '../../components/customers/RunCustomer';
import { useScopedList } from '../../lib/useScopedList';

export default function WorkflowRuns({ customer: fixedCustomer }: { customer?: string } = {}) {
  const [q, setQ] = useListQuery(fixedCustomer ? 'wq' : 'q');
  const scoped = useScopedList(fixedCustomer, []);
  const { customer, embedded } = scoped;
  // Row-action (archive/restore/delete) failures — kept separate from the
  // list-fetch error the hook owns, but shown in the same banner.
  const [actionError, setActionError] = useState<string | null>(null);
  const archive = asksArchived(q);

  const fetchRows = useCallback(
    (p: PerHostFetchParams & ListQueryOpts): Promise<RunListRow[]> =>
      customer ? scoped.guard(api.getWorkflowRuns({ ...p, customer })) : api.getWorkflowRuns(p),
    [customer, scoped.guard],
  );

  const { rows, slices, loading, error, hasMore, sentinelRef, refresh, refreshHost, removeRow, retryPaging, ended, facets, queryError, notes } =
    usePerHostQueryList<RunListRow>({
      q,
      fields: WORKFLOW_RUN_FIELDS,
      scope: [customer],
      fetch: fetchRows,
      timeField: 'started_at',
      idField: 'id',
      poll: !archive,
    });

  // Row-level archive / restore / delete — each drops the row and re-syncs
  // just its host after success.
  // `host` is the row's own `host_id` (undefined/`"local"` → local store,
  // any other id → proxied through that host's connector) so a fanned-out
  // remote-host row's action lands on the host that actually owns the run.
  async function handleRowArchive(id: string, host?: string) {
    try {
      await api.archiveRun(id, host);
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
      await api.restoreRun(id, host);
      setActionError(null);
      // The row left this list; drop it now, then re-sync just its host.
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Restore failed');
    }
  }

  async function handleRowDelete(id: string, host?: string) {
    if (!window.confirm('Permanently delete this run and its transcripts? This cannot be undone.')) return;
    try {
      await api.deleteRun(id, host);
      setActionError(null);
      // The row left this list; drop it now, then re-sync just its host.
      removeRow(host ?? 'local', id);
      refreshHost(host ?? 'local');
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  // Action column — an archived row restores, any other archives.
  const actionColumn: Column<RunListRow> = {
    key: 'actions',
    header: '',
    fit: true,
    // Its own real buttons (Archive/Restore/Delete) — keep them
    // independently focusable/announced rather than swallowed by the
    // row-link's mouse-only treatment (I7).
    interactive: true,
    render: (r) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => {
          // The row is now link-wrapped (rowHref) — without both of these,
          // this click either soft- or hard-navigates to the run instead of
          // acting (stopPropagation alone does not block the enclosing
          // <a>'s native default navigation action).
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        {r.archived ? (
          <Button
            variant="ring"
            onClick={() => void handleRowRestore(r.id, r.host_id)}
            aria-label={`Restore run ${r.id}`}
          >
            Restore
          </Button>
        ) : (
          <Button
            variant="ring"
            onClick={() => void handleRowArchive(r.id, r.host_id)}
            aria-label={`Archive run ${r.id}`}
          >
            Archive
          </Button>
        )}
        <Button
          variant="ring-danger"
          onClick={() => void handleRowDelete(r.id, r.host_id)}
          aria-label={`Delete run ${r.id}`}
        >
          Delete
        </Button>
      </div>
    ),
  };

  // The Customer column resolves slugs (archived customers load on first need).
  const customers = useRunCustomers(!customer ? rows.map((r) => r.customer) : []);
  const columns: Column<RunListRow>[] = [
    ...withCustomerColumn(WORKFLOW_RUN_COLUMNS, customers, rows, !customer),
    actionColumn,
  ];
  // A fresh action error (e.g. this click's Archive/Delete refusal) must win
  // over a stale fetch error from an earlier load — never the other way
  // around, or the operator sees the wrong banner for what just happened.
  const bannerError = actionError ?? error;
  // Never claim "nothing" while a host is still loading.
  const waiting = waitingLabel(slices);

  return (
    <div className={embedded ? undefined : 'p-8'}>
      <header className={cn('flex items-center justify-between', embedded ? 'mb-3 justify-end' : 'mb-6')}>
        {!embedded && (
          <div>
            <h1 className="text-2xl font-semibold text-ink">Workflow Runs</h1>
            <p className="mt-1 text-sm text-ink-dim">Workflow executions across this control plane.</p>
          </div>
        )}
        <Button variant="secondary" onClick={() => refresh()} className="gap-1.5">
          <RefreshCw size={12} className={cn(loading && 'animate-spin')} />
          Refresh
        </Button>
      </header>

      <ListQueryBar
        value={q}
        onChange={setQ}
        fields={WORKFLOW_RUN_FIELDS}
        facets={facets}
        label="Filter workflow runs"
        placeholder="Filter runs… e.g. status:running · status:failed since:7d · is:archived"
        error={queryError}
        alerts={hostAlerts(notes, 'runs', retryPaging)}
      />

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
          <NoMatches noun="workflow runs" />
        ) : (
          <EmptyState
            title="No workflow runs"
            hint={
              customer === 'none'
                ? 'No workflow run without a customer yet. is:archived lists the archive.'
                : customer
                  ? 'No workflow run is attributed to this customer yet. is:archived lists the archive.'
                  : 'Runs appear here once a workflow runs. is:archived lists the archive.'
            }
          />
        )
      ) : (
        <div className="space-y-6">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
            <UsageBarChart bars={rows.map((r) => ({
              id: r.id, label: r.workflow_name, to: runHref(r),
              input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
              cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
            }))} />
          </div>
          <SortableTable<RunListRow>
            columns={columns}
            rows={rows}
            rowKey={(r) => `${r.host_id ?? 'local'}:${r.id}`}
            rowHref={runHref}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          <ListFooter sentinelRef={sentinelRef} loading={loading} hasMore={hasMore} ended={ended} count={rows.length} />
        </div>
      )}
    </div>
  );
}
