// Project Runs tab body — paginated run list scoped to one project, mirroring
// the standalone WorkflowRuns page (pages/runs/WorkflowRuns.tsx): one query
// line the server evaluates (`?q=` on every request, `WORKFLOW_RUN_FIELDS`),
// over every status unless the query names one, + usePagedList + SortableTable
// fit/subject columns + StatusPill + EmptyState/ErrorBanner.
//
// Project tabs are already workspace-scoped (the project IS the scope), and
// the project's runs are this host's.
//
// The usage graph above the table is `ProjectUsageTimeline` (Task U4) — the
// project-scoped mount of the SAME interactive spend-over-time graph
// `/usage` uses, replacing the old per-run `UsageBarChart` (one bar per run,
// stacked by token type). See that component's doc comment for why it owns
// its own fetch/state rather than sharing this tab's `runs`/filter state.

import { api, type RunListRow } from '../../lib/api';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import { CrewChip } from '../codename/CrewChip';
import SortableTable, { type Column } from '../lists/SortableTable';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import ProjectUsageTimeline from './ProjectUsageTimeline';
import { PricingErrorMark } from '../customers/PricingErrorMark';
import { ListQueryBar, NoMatches, useListQuery } from '../query/ListQueryBar';
import { ListFooter } from '../lists/ListFooter';
import { usePagedQueryList } from '../../lib/query/usePagedQueryList';
import { WORKFLOW_RUN_FIELDS } from '../../lib/query/lists/workflowRuns';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { shortId } from '../../lib/shortId';
import { parseCodename } from '../../lib/codename';

/** Run duration in ms — explicit duration_ms, else derived from start/finish. */
function runDurationMs(run: RunListRow): number | null {
  if (run.duration_ms != null) return run.duration_ms;
  const start = Date.parse(run.started_at);
  if (Number.isNaN(start)) return null;
  const end = run.finished_at ? Date.parse(run.finished_at) : Date.now();
  if (Number.isNaN(end)) return null;
  return Math.max(0, end - start);
}

const RUN_COLUMNS: Column<RunListRow>[] = [
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusPill status={r.status} />,
  },
  {
    key: 'workflow',
    header: 'Workflow',
    subject: true,
    sortable: true,
    sortValue: (r) => r.workflow_name,
    titleValue: (r) => r.workflow_name,
    render: (r) => (
      <span className="inline-flex items-center gap-2">
        {r.codename && <CrewChip crew={parseCodename(r.codename).crew} derived={r.codename_derived} />}
        <span className="text-sm font-medium text-ink">{r.workflow_name}</span>
      </span>
    ),
  },
  {
    key: 'run',
    header: 'Run',
    fit: true,
    render: (r) => <span className="text-note text-ink-mute font-mono">{shortId(r.id)}</span>,
  },
  {
    key: 'trigger',
    header: 'Trigger',
    fit: true,
    sortable: true,
    sortValue: (r) => r.trigger,
    render: (r) => <TriggerChip trigger={r.trigger} />,
  },
  {
    key: 'in',
    header: 'In',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.usage.input_tokens,
    render: (r) => <span className="text-ink-dim">{formatTokens(r.usage.input_tokens)}</span>,
  },
  {
    key: 'out',
    header: 'Out',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.usage.output_tokens,
    render: (r) => <span className="text-ink-dim">{formatTokens(r.usage.output_tokens)}</span>,
  },
  {
    key: 'cached',
    header: 'Cached',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.usage.cached_tokens,
    render: (r) =>
      r.usage.cached_tokens ? (
        <span className="text-ink-dim">{formatTokens(r.usage.cached_tokens)}</span>
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
    sortValue: (r) => r.usage.cost_usd,
    render: (r) => (
      <span className="inline-flex items-center justify-end gap-1 text-ink font-medium">
        <PricingErrorMark error={r.usage.pricing_error} />
        {formatCost(r.usage.cost_usd)}
      </span>
    ),
  },
  {
    key: 'turns',
    header: 'Turns',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.turns,
    render: (r) => <span className="text-ink">{r.turns ? String(r.turns) : '—'}</span>,
  },
  {
    key: 'duration',
    header: 'Duration',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => runDurationMs(r),
    render: (r) => (
      <span className="text-ink-dim">
        {r.duration_ms != null
          ? formatDuration(r.duration_ms)
          : durationBetween(r.started_at, r.finished_at)}
      </span>
    ),
  },
  {
    key: 'started',
    header: 'Started',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => (r.started_at ? Date.parse(r.started_at) : null),
    render: (r) => <span className="text-ink-mute">{relativeTime(r.started_at)}</span>,
  },
];

export default function ProjectRunsTab({ wsId }: { wsId: string }) {
  const [q, setQ] = useListQuery();
  const { rows, loading, error, hasMore, sentinelRef, ended, facets, queryError } = usePagedQueryList<RunListRow>({
    q,
    fields: WORKFLOW_RUN_FIELDS,
    scope: [wsId],
    fetch: (p) => api.getProjectRuns(wsId, p),
  });

  return (
    <div className="space-y-4">
      <ListQueryBar
        className="mb-0"
        value={q}
        onChange={setQ}
        fields={WORKFLOW_RUN_FIELDS}
        facets={facets}
        label="Filter runs"
        placeholder="status:failed · trigger:cron · since:7d"
        error={queryError}
      />

      {/* The spend-over-time graph — Task U4: the SAME `/usage` graph,
          scoped to this project's `workspace_id`. Independent data source
          from the run list below (its own `getUsageRuns(window, wsId)`
          fetch), so it renders regardless of the run-list's loading/filter
          state. */}
      <ProjectUsageTimeline wsId={wsId} />

      {error && <ErrorBanner>{error}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-12 flex items-center justify-center">
          <Spinner label="Loading runs…" />
        </div>
      ) : rows.length === 0 ? (
        q.trim() ? <NoMatches noun="runs" /> : <EmptyState title="No runs for this project yet" />
      ) : (
        <>
          <SortableTable<RunListRow>
            columns={RUN_COLUMNS}
            rows={rows}
            rowKey={(r) => r.id}
            rowHref={(r) => `/runs/${encodeURIComponent(r.id)}`}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          <ListFooter sentinelRef={sentinelRef} loading={loading} hasMore={hasMore} ended={ended} count={rows.length} />
        </>
      )}
    </div>
  );
}
