// Workflow run-stream page — execution history for workflow runs.
// One lifecycle FilterPills group (Running / Completed / Failed-Rejected /
// Archived — Archived is a 4th mutually-exclusive state, exactly matching
// the old toggle-hides-tabs behavior) + a trigger FilterPills group (hidden
// in Archived mode, same as the old code hid its trigger chips + host
// select together) + a host scope select (hidden in Archived mode too).
//
// Fetch/paginate/poll is owned by `usePagedList`: the active/Running
// lifecycle is the only one that polls (5 s, page-0 only — matches today).
// Archived has no server-side pagination (`/api/runs/archived` returns
// everything in one shot); the fetcher returns `[]` for any offset > 0 so
// the hook settles as `ended` after that single page.
//
// Find (2026-07-23 operator feedback amendment #1): a `SearchInput` in the
// FilterBar's search slot narrows the loaded rows client-side, live per
// keystroke, over workflow name / run id / host id — composing with (not
// replacing) the lifecycle/trigger pills above it.

import { useCallback, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { api, type RunListRow } from '../../lib/api';
import { StatusPill } from '../../components/StatusPill';
import SortableTable, { type Column } from '../../components/lists/SortableTable';
import UsageBarChart from '../../components/charts/UsageBarChart';
import { Button } from '../../components/ui/Button';
import { FilterBar } from '../../components/ui/FilterBar';
import { FilterPills, type FilterPillOption } from '../../components/ui/FilterPills';
import { SearchInput } from '../../components/ui/SearchInput';
import { EmptyState } from '../../components/ui/EmptyState';
import { ErrorBanner } from '../../components/ui/ErrorBanner';
import { Spinner } from '../../components/ui/Spinner';
import HostSelect, { ALL_HOSTS } from '../../components/HostSelect';
import { usePagedList } from '../../lib/usePagedList';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { shortId } from '../../lib/shortId';

type Tab = 'active' | 'completed' | 'failed';
/** The lifecycle FilterPills group's value space: the three tabs plus the
 *  Archived state, which today is a separate boolean but behaves as (and
 *  always rendered as) a 4th mutually-exclusive state — selecting a tab
 *  always implies "not archived", and archived mode always hides the tabs. */
type Lifecycle = Tab | 'archived';

const LIFECYCLE_OPTIONS: FilterPillOption[] = [
  { value: 'active', label: 'Running' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed / Rejected' },
  { value: 'archived', label: 'Archived' },
];

type TriggerFilter = 'all' | 'manual' | 'cron' | 'event';

const TRIGGER_OPTIONS: FilterPillOption[] = [
  { value: 'all', label: 'All' },
  { value: 'manual', label: 'Manual' },
  { value: 'cron', label: 'Cron' },
  { value: 'event', label: 'Event' },
];

const TRIGGER_CHIP_CLS: Record<string, string> = {
  manual: 'bg-surface text-ink ring-border',
  cron:   'bg-violet-50 text-violet-700 ring-violet-200',
  event:  'bg-sky-50 text-sky-700 ring-sky-200',
};

function TriggerChip({ trigger }: { trigger: string }) {
  const cls = TRIGGER_CHIP_CLS[trigger] ?? 'bg-surface text-ink ring-border';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5 whitespace-nowrap',
        cls,
      )}
    >
      {trigger}
    </span>
  );
}

/** Build the detail link for a run, including ?host= for remote runs. */
function runHref(r: RunListRow): string {
  const hid = r.host_id;
  if (hid && hid !== 'local') {
    return `/runs/${encodeURIComponent(r.id)}?host=${encodeURIComponent(hid)}`;
  }
  return `/runs/${encodeURIComponent(r.id)}`;
}

export default function WorkflowRuns() {
  const [tab, setTab] = useState<Tab>('active');
  const [archived, setArchived] = useState(false);
  const [filter, setFilter] = useState<TriggerFilter>('all');
  // Default to 'local' → fast server-side path; ALL_HOSTS → fan-out.
  const [hostFilter, setHostFilter] = useState<string>('local');
  // Row-action (archive/restore/delete) failures — kept separate from the
  // list-fetch error the hook owns, but shown in the same banner.
  const [actionError, setActionError] = useState<string | null>(null);
  const [query, setQuery] = useState('');

  const fetchRows = useCallback(
    ({ offset, limit }: { offset: number; limit: number }): Promise<RunListRow[]> => {
      if (archived) {
        // /api/runs/archived has no offset/limit — it's a single fetch.
        // Any page beyond the first returns empty so the hook settles.
        return offset === 0 ? api.getArchivedRuns('workflow') : Promise.resolve([]);
      }
      const host = hostFilter === ALL_HOSTS ? undefined : hostFilter;
      return api.getWorkflowRuns({ lifecycle: tab, offset, limit, host });
    },
    [archived, tab, hostFilter],
  );

  const { rows, loading, error, hasMore, sentinelRef, refresh, ended } = usePagedList<RunListRow>({
    fetch: fetchRows,
    deps: [archived, tab, hostFilter],
    poll: !archived && tab === 'active',
  });

  // Trigger filter is still client-side (cheap; host filter is server-side).
  const filtered = rows.filter((r) => {
    if (!archived && filter !== 'all' && r.trigger !== filter) return false;
    return true;
  });

  // Find — case-insensitive substring across the fields this table actually
  // renders: workflow name (subject), run id, and host id. Composes with
  // (narrows within) the lifecycle/trigger pills above.
  const q = query.trim().toLowerCase();
  const visible = q
    ? filtered.filter((r) =>
        [r.workflow_name, r.id, r.host_id]
          .filter((v): v is string => Boolean(v))
          .some((v) => v.toLowerCase().includes(q)),
      )
    : filtered;

  const lifecycleValue: Lifecycle = archived ? 'archived' : tab;
  function handleLifecycleChange(v: string) {
    if (v === 'archived') {
      setArchived(true);
    } else {
      setArchived(false);
      setTab(v as Tab);
    }
  }

  // Row-level archive / restore / delete — each refetches after success.
  async function handleRowArchive(id: string) {
    try {
      await api.archiveRun(id);
      setActionError(null);
      refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Archive failed');
    }
  }

  async function handleRowRestore(id: string) {
    try {
      await api.restoreRun(id);
      setActionError(null);
      refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Restore failed');
    }
  }

  async function handleRowDelete(id: string) {
    if (!window.confirm('Permanently delete this run and its transcripts? This cannot be undone.')) return;
    try {
      await api.deleteRun(id);
      setActionError(null);
      refresh();
    } catch (e) {
      setActionError(e instanceof Error ? e.message : 'Delete failed');
    }
  }

  // Action column — changes shape based on archived mode.
  const actionColumn: Column<RunListRow> = {
    key: 'actions',
    header: '',
    fit: true,
    render: (r) => (
      <div
        className="flex items-center justify-end gap-1"
        onClick={(e) => e.stopPropagation()}
      >
        {archived ? (
          <Button
            variant="ring"
            onClick={() => void handleRowRestore(r.id)}
            aria-label={`Restore run ${r.id}`}
          >
            Restore
          </Button>
        ) : (
          <Button
            variant="ring"
            onClick={() => void handleRowArchive(r.id)}
            aria-label={`Archive run ${r.id}`}
          >
            Archive
          </Button>
        )}
        <Button
          variant="ring-danger"
          onClick={() => void handleRowDelete(r.id)}
          aria-label={`Delete run ${r.id}`}
        >
          Delete
        </Button>
      </div>
    ),
  };

  const columns: Column<RunListRow>[] = [...WORKFLOW_RUN_COLUMNS, actionColumn];
  const bannerError = error ?? actionError;

  return (
    <div className="p-8">
      <header className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Workflow Runs</h1>
          <p className="mt-1 text-sm text-ink-dim">Workflow executions across this control plane.</p>
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
              <FilterPills options={LIFECYCLE_OPTIONS} value={lifecycleValue} onChange={handleLifecycleChange} />
              {!archived && (
                <FilterPills
                  options={TRIGGER_OPTIONS}
                  value={filter}
                  onChange={(v) => setFilter(v as TriggerFilter)}
                />
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
              <HostSelect
                allowAll
                ariaLabel="Host filter"
                value={hostFilter}
                onChange={setHostFilter}
              />
            )
          }
        />
      </div>

      {bannerError && <ErrorBanner className="mb-4">{bannerError}</ErrorBanner>}

      {loading && rows.length === 0 ? (
        <div className="py-16 flex items-center justify-center">
          <Spinner label="Loading runs…" />
        </div>
      ) : filtered.length === 0 ? (
        <EmptyState
          title={rows.length > 0 ? 'No runs match this filter' : 'No workflow runs yet'}
          hint={
            rows.length > 0
              ? 'Try selecting a different trigger or host filter above.'
              : 'Workflow runs will appear here once you dispatch one from the CLI, the desktop app, or a scheduled trigger.'
          }
        />
      ) : visible.length === 0 ? (
        <EmptyState title="No matches" hint={`No runs match "${query}".`} />
      ) : (
        <div className="space-y-6">
          <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3 mb-4">
            <UsageBarChart bars={visible.map((r) => ({
              id: r.id, label: r.workflow_name, to: runHref(r),
              input_tokens: r.usage.input_tokens, output_tokens: r.usage.output_tokens,
              cached_tokens: r.usage.cached_tokens, cost_usd: r.usage.cost_usd,
            }))} />
          </div>
          <SortableTable<RunListRow>
            columns={columns}
            rows={visible}
            rowKey={(r) => r.id}
            rowHref={runHref}
            initialSort={{ key: 'started', dir: 'desc' }}
          />
          {!archived && (loading || hasMore || ended) && (
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
      )}
    </div>
  );
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

const WORKFLOW_RUN_COLUMNS: Column<RunListRow>[] = [
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
    key: 'host',
    header: 'Host',
    fit: true,
    sortable: true,
    sortValue: (r) => r.host_id ?? 'local',
    render: (r) => (
      <span className="text-note text-ink-mute font-mono">{r.host_id ?? 'local'}</span>
    ),
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
