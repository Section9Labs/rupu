// Project Runs tab body — paginated run list scoped to one project, mirroring
// the standalone WorkflowRuns page (pages/runs/WorkflowRuns.tsx) on the kit:
// FilterBar + FilterPills (Status, Trigger) + SearchInput Find + usePagedList
// + SortableTable fit/subject columns + StatusPill + EmptyState/ErrorBanner.
//
// Project tabs are already workspace-scoped (the project IS the scope), so
// there is no HostSelect slot here — unlike WorkflowRuns, which fans out
// across hosts.
//
// Ported from pages/ProjectRuns.tsx (fetch / load-more rendering), reshaped
// into a self-contained component keyed off the `wsId` prop. Rows render via
// the shared SortableTable.
//
// The usage graph above the table is `ProjectUsageTimeline` (Task U4) — the
// project-scoped mount of the SAME interactive spend-over-time graph
// `/usage` uses, replacing the old per-run `UsageBarChart` (one bar per run,
// stacked by token type). See that component's doc comment for why it owns
// its own fetch/state rather than sharing this tab's `runs`/filter state.
//
// Find (parity with WorkflowRuns' 2026-07-23 amendment): a `SearchInput` in
// the FilterBar's search slot narrows the loaded rows client-side, live per
// keystroke, over workflow name / run id / trigger — composing with (not
// replacing) the status/trigger pills above it.

import { useState } from 'react';
import { api, type RunListRow, type RunStatusStr } from '../../lib/api';
import { StatusPill } from '../StatusPill';
import { TriggerChip } from '../TriggerChip';
import SortableTable, { type Column } from '../lists/SortableTable';
import { FilterBar } from '../ui/FilterBar';
import { FilterPills, type FilterPillOption } from '../ui/FilterPills';
import { SearchInput } from '../ui/SearchInput';
import { EmptyState } from '../ui/EmptyState';
import { ErrorBanner } from '../ui/ErrorBanner';
import { Spinner } from '../ui/Spinner';
import ProjectUsageTimeline from './ProjectUsageTimeline';
import { usePagedList } from '../../lib/usePagedList';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { shortId } from '../../lib/shortId';

// --- Filter definitions -----------------------------------------------------

type StatusFilter = 'all' | 'running' | 'completed' | 'failed';
type TriggerFilter = 'all' | 'manual' | 'event' | 'cron';

const STATUS_OPTIONS: FilterPillOption[] = [
  { value: 'all', label: 'All' },
  { value: 'running', label: 'Running' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
];

const TRIGGER_OPTIONS: FilterPillOption[] = [
  { value: 'all', label: 'All' },
  { value: 'manual', label: 'Manual' },
  { value: 'event', label: 'Event' },
  { value: 'cron', label: 'Cron' },
];

// Status grouping mirrors the run-status enum in StatusPill:
//   running   → running | pending | awaiting_approval
//   completed → completed
//   failed    → failed | rejected
const STATUS_GROUP: Record<Exclude<StatusFilter, 'all'>, ReadonlySet<RunStatusStr>> = {
  running: new Set<RunStatusStr>(['running', 'pending', 'awaiting_approval']),
  completed: new Set<RunStatusStr>(['completed']),
  failed: new Set<RunStatusStr>(['failed', 'rejected']),
};

function matchesStatus(status: RunStatusStr, filter: StatusFilter): boolean {
  return filter === 'all' || STATUS_GROUP[filter].has(status);
}

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
    key: 'workflow',
    header: 'Workflow',
    subject: true,
    sortable: true,
    sortValue: (r) => r.workflow_name,
    titleValue: (r) => r.workflow_name,
    render: (r) => <span className="text-sm font-medium text-ink">{r.workflow_name}</span>,
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
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusPill status={r.status} />,
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
    render: (r) => <span className="text-ink font-medium">{formatCost(r.usage.cost_usd)}</span>,
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
    key: 'turns',
    header: 'Turns',
    align: 'right',
    fit: true,
    sortable: true,
    sortValue: (r) => r.turns,
    render: (r) => <span className="text-ink">{r.turns ? String(r.turns) : '—'}</span>,
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
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [triggerFilter, setTriggerFilter] = useState<TriggerFilter>('all');
  const [query, setQuery] = useState('');

  const { rows, loading, error, hasMore, sentinelRef, ended } = usePagedList<RunListRow>({
    fetch: ({ offset, limit }) => api.getProjectRuns(wsId, { offset, limit }),
    deps: [wsId],
  });

  const filtered = rows.filter(
    (r) => matchesStatus(r.status, statusFilter) && (triggerFilter === 'all' || r.trigger === triggerFilter),
  );

  // Find — case-insensitive substring across the fields this table actually
  // renders: workflow name (subject), run id, and trigger. Composes with
  // (narrows within) the status/trigger pills above.
  const q = query.trim().toLowerCase();
  const visible = q
    ? filtered.filter((r) =>
        [r.workflow_name, r.id, r.trigger]
          .filter((v): v is string => Boolean(v))
          .some((v) => v.toLowerCase().includes(q)),
      )
    : filtered;

  return (
    <div className="space-y-4">
      <FilterBar
        filters={
          <>
            <FilterPills
              label="Status"
              options={STATUS_OPTIONS}
              value={statusFilter}
              onChange={(v) => setStatusFilter(v as StatusFilter)}
            />
            <FilterPills
              label="Trigger"
              options={TRIGGER_OPTIONS}
              value={triggerFilter}
              onChange={(v) => setTriggerFilter(v as TriggerFilter)}
            />
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
      ) : filtered.length === 0 ? (
        <EmptyState
          title={rows.length === 0 ? 'No runs for this project yet' : 'No runs match this filter'}
        />
      ) : visible.length === 0 ? (
        <EmptyState title="No matches" hint={`No runs match "${query}".`} />
      ) : (
        <SortableTable<RunListRow>
          columns={RUN_COLUMNS}
          rows={visible}
          rowKey={(r) => r.id}
          rowHref={(r) => `/runs/${encodeURIComponent(r.id)}`}
          initialSort={{ key: 'started', dir: 'desc' }}
        />
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
