// The Workflows run-table columns, shared by Runs → Workflows and Runs →
// Autoflows (spec 2026-10-14, CP autoflow runs as one table). Moved verbatim
// from `pages/runs/WorkflowRuns.tsx`; its rendered output must not change.
// Canonical order (table-standardization follow-up, I1): Status leads, then
// Subject (Workflow) → ID (Run) → Source (Trigger) → Host → usage → Turns →
// Duration → Started. Pages append their own action column.

import type { RunListRow } from '../../lib/api';
import { RunStatusChip } from '../ui/RunStatusChip';
import { CrewChip } from '../codename/CrewChip';
import { parseCodename } from '../../lib/codename';
import type { Column } from '../lists/SortableTable';
import { cn } from '../../lib/cn';
import { durationBetween, relativeTime } from '../../lib/time';
import { formatTokens, formatCost } from '../../lib/usage';
import { formatDuration } from '../../lib/duration';
import { shortId } from '../../lib/shortId';
import { PricingErrorMark } from '../customers/PricingErrorMark';

const TRIGGER_CHIP_CLS: Record<string, string> = {
  manual: 'bg-surface text-ink ring-border',
  cron:   'bg-violet-50 text-violet-700 ring-violet-200',
  event:  'bg-sky-50 text-sky-700 ring-sky-200',
};

export function TriggerChip({ trigger }: { trigger: string }) {
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

/** Run duration in ms — explicit duration_ms, else derived from start/finish. */
export function runDurationMs(run: RunListRow): number | null {
  if (run.duration_ms != null) return run.duration_ms;
  const start = Date.parse(run.started_at);
  if (Number.isNaN(start)) return null;
  const end = run.finished_at ? Date.parse(run.finished_at) : Date.now();
  if (Number.isNaN(end)) return null;
  return Math.max(0, end - start);
}

export const WORKFLOW_RUN_COLUMNS: Column<RunListRow>[] = [
  {
    key: 'status',
    header: 'Status',
    fit: true,
    sortable: true,
    sortValue: (r) => r.status,
    render: (r) => <RunStatusChip status={r.status} />,
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
