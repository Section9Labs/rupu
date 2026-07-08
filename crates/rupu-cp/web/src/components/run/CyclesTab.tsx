// Cycles tab — the entity's autoflow cycle history as a referenceable,
// linked table: the current cycle (the run being viewed) plus any prior
// cycles the autoflow worker ran for the same entity, each row linking to
// its run. Moved out of AutoflowPanel (T3 of the cp-autoflow-ux batch) so
// the cycle list is a proper tab alongside Transcript/Events/Findings
// instead of a side-panel list with no direct links.
//
// A prior cycle's `AutoflowCycleRecord` carries no top-level run id — only a
// raw `events` log (see `AutoflowCycleEventRaw` in lib/api.ts). We recover
// the run this cycle drove for the CURRENT entity by scanning that log for
// the event matching `context.issue_ref`, preferring one that carries a
// `run_id` (a launched run) over a bare awaiting/failed signal.

import type { AutoflowCycleEventRaw, AutoflowPriorCycle, AutoflowRunContext } from '../../lib/api';
import SortableTable, { type Column } from '../lists/SortableTable';
import { relativeTime } from '../../lib/time';
import { shortId } from '../../lib/shortId';
import { cn } from '../../lib/cn';

interface CycleTableRow {
  cycleId: string;
  workflow: string | null;
  status: string | null;
  startedAt: string;
  runId: string | null;
  current: boolean;
}

/** The event within a prior cycle's raw log that best represents what
 *  happened to THIS entity in that cycle: prefer an entry carrying a
 *  `run_id`, else fall back to the last entry that matched the entity at
 *  all (e.g. an `awaiting_human` / `cycle_failed` signal with no run). */
function entityEvent(
  cycle: AutoflowPriorCycle,
  issueRef: string | null,
): AutoflowCycleEventRaw | null {
  const events = cycle.events ?? [];
  const matching = issueRef ? events.filter((e) => e.issue_ref === issueRef) : events;
  const withRun = matching.find((e) => e.run_id);
  if (withRun) return withRun;
  return matching.length > 0 ? matching[matching.length - 1] : null;
}

function priorCycleRow(cycle: AutoflowPriorCycle, issueRef: string | null): CycleTableRow {
  const event = entityEvent(cycle, issueRef);
  return {
    cycleId: cycle.cycle_id,
    workflow: event?.workflow ?? null,
    status: event?.status ?? (cycle.failed_cycles > 0 ? 'failed' : 'ok'),
    startedAt: cycle.started_at,
    runId: event?.run_id ?? null,
    current: false,
  };
}

const STATUS_CLS: Record<string, string> = {
  completed: 'bg-ok-bg text-ok ring-ok/30',
  complete: 'bg-ok-bg text-ok ring-ok/30',
  ok: 'bg-ok-bg text-ok ring-ok/30',
  running: 'bg-status-running/10 text-status-running ring-status-running/30',
  pending: 'bg-surface text-ink ring-border',
  paused: 'bg-status-paused/10 text-status-paused ring-status-paused/30',
  failed: 'bg-err-bg text-err ring-err/30',
  blocked: 'bg-err-bg text-err ring-err/30',
  cancelled: 'bg-surface text-ink ring-border',
  rejected: 'bg-err-bg text-err ring-err/30',
  awaiting_approval: 'bg-warn-bg text-warn ring-warn/30',
  awaiting_human: 'bg-warn-bg text-warn ring-warn/30',
  awaiting_external: 'bg-sky-50 text-sky-700 ring-sky-200',
};

function StatusBadge({ status }: { status: string | null }) {
  if (!status) return <span className="text-ink-mute">—</span>;
  const cls = STATUS_CLS[status] ?? 'bg-surface text-ink ring-border';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-meta font-medium uppercase tracking-wide px-1.5 py-0.5',
        cls,
      )}
    >
      {status.replace(/_/g, ' ')}
    </span>
  );
}

function cycleHref(row: CycleTableRow, host?: string): string | undefined {
  if (!row.runId) return undefined;
  // Only the current row's run is known to have run on `host` (threaded from
  // the page's own ?host= — see the RunDetail doc comment on remote hosts).
  // Prior-cycle runs don't carry a host today (AutoflowCycleEvent has no
  // host_id field; mirrors AutoflowRunContext.host_id's "always local until
  // distributed autoflow dispatch exists" note), so those link bare.
  const hostSuffix = row.current && host && host !== 'local' ? `?host=${encodeURIComponent(host)}` : '';
  return `/runs/${encodeURIComponent(row.runId)}${hostSuffix}`;
}

const COLUMNS: Column<CycleTableRow>[] = [
  {
    key: 'cycle',
    header: 'Cycle',
    sortable: true,
    sortValue: (r) => r.cycleId,
    render: (r) => (
      <span className="inline-flex items-center gap-1.5">
        <span className="font-mono text-sm text-ink">{shortId(r.cycleId, 12)}</span>
        {r.current && (
          <span className="rounded bg-info-bg px-1.5 py-0.5 text-meta font-medium text-info ring-1 ring-info/30">
            current
          </span>
        )}
      </span>
    ),
  },
  {
    key: 'workflow',
    header: 'Workflow',
    sortable: true,
    sortValue: (r) => r.workflow,
    render: (r) => <span className="text-sm text-ink">{r.workflow ?? '—'}</span>,
  },
  {
    key: 'status',
    header: 'Status',
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <StatusBadge status={r.status} />,
  },
  {
    key: 'started',
    header: 'Started',
    sortable: true,
    sortValue: (r) => (r.startedAt ? Date.parse(r.startedAt) : null),
    render: (r) => <span className="text-ink-mute">{relativeTime(r.startedAt)}</span>,
  },
  {
    key: 'run',
    header: 'Run',
    render: (r) =>
      r.runId ? (
        <span className="font-mono text-note text-brand-600">{shortId(r.runId)}</span>
      ) : (
        <span className="text-ink-mute">—</span>
      ),
  },
];

export default function CyclesTab({
  context,
  currentRunId,
  currentRunStartedAt,
  host,
}: {
  context: AutoflowRunContext;
  currentRunId: string;
  currentRunStartedAt: string;
  /** The page's own `?host=` (undefined/`'local'` for a local run). */
  host?: string;
}) {
  const rows: CycleTableRow[] = [
    {
      cycleId: context.cycle_id,
      workflow: context.workflow_name,
      status: context.status,
      startedAt: currentRunStartedAt,
      runId: currentRunId,
      current: true,
    },
    ...context.prior_cycles.map((c) => priorCycleRow(c, context.issue_ref)),
  ];

  return (
    <div data-testid="cycles-tab">
      <SortableTable<CycleTableRow>
        columns={COLUMNS}
        rows={rows}
        rowKey={(r) => r.cycleId}
        rowHref={(r) => cycleHref(r, host)}
        initialSort={{ key: 'started', dir: 'desc' }}
      />
    </div>
  );
}
