// RecentActivity — a read-only feed of the fleet's latest runs (any status),
// newest first. It gives the Overview a sense of motion; it is NOT the action
// list (that's the Inbox). Each row links to the run. Backed by
// `useRecentActivity` (existing endpoints).

import { Link } from 'react-router-dom';
import { StatusChip, type ChipTone } from '../ui/StatusChip';
import { Skeleton } from '../ui/Skeleton';
import { EmptyState } from '../ui/EmptyState';
import { AgentName } from '../codename/AgentName';
import { formatDuration } from '../../lib/duration';
import { runHref } from '../../lib/runs';
import { useRecentActivity, type RecentRun } from '../../lib/dashboard/useRecentActivity';
import type { CustomerScope, RunStatusStr } from '../../lib/api';

const STATUS_CHIP: Partial<Record<RunStatusStr, { tone: ChipTone; label: string }>> = {
  running: { tone: 'running', label: 'running' },
  completed: { tone: 'completed', label: 'completed' },
  failed: { tone: 'failed', label: 'failed' },
  awaiting_approval: { tone: 'awaiting_approval', label: 'gate' },
  paused: { tone: 'paused', label: 'paused' },
  cancelled: { tone: 'cancelled', label: 'cancelled' },
  rejected: { tone: 'rejected', label: 'rejected' },
};

function timeLabel(run: RecentRun, now: number): string {
  if (!run.at) return '';
  const ago = formatDuration(now - Date.parse(run.at));
  return run.running ? ago : `${ago} ago`;
}

export function RecentActivity({ customer }: { customer?: CustomerScope }) {
  const { runs, loading } = useRecentActivity(customer);
  const now = Date.now();

  if (loading && runs.length === 0) {
    return (
      <div className="overflow-hidden rounded-xl border border-border bg-panel">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="flex items-center gap-3 border-t border-border px-4 py-2.5 first:border-t-0">
            <Skeleton className="h-5 w-16 rounded-full" />
            <Skeleton className="h-4 w-40" />
            <span className="flex-1" />
            <Skeleton className="h-3 w-12" />
          </div>
        ))}
      </div>
    );
  }

  if (runs.length === 0) {
    return <EmptyState title="No recent runs" hint="Workflow runs across the fleet show up here." />;
  }

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-panel">
      {runs.map((run) => {
        const chip = STATUS_CHIP[run.status] ?? { tone: 'neutral' as ChipTone, label: run.status };
        const href = runHref({ id: run.id, host_id: run.host === 'local' ? undefined : run.host });
        return (
          <Link
            key={`${run.host}:${run.id}`}
            to={href}
            className="flex items-center gap-3 border-t border-border px-4 py-2.5 first:border-t-0 hover:bg-surface/60"
          >
            <StatusChip tone={chip.tone} dot={run.running}>
              {chip.label}
            </StatusChip>
            <span className="min-w-0 shrink-0 truncate">
              <AgentName codename={run.codename} derived={run.codenameDerived} />
            </span>
            <span className="flex min-w-0 flex-1 items-center gap-2 truncate text-note text-ink-dim">
              <span className="truncate">{run.workflowName}</span>
              {run.host !== 'local' && (
                <span className="shrink-0 rounded bg-surface px-1.5 py-0.5 font-mono text-meta text-ink-mute">
                  {run.host}
                </span>
              )}
            </span>
            <span className="shrink-0 tabular-nums text-note text-ink-mute">{timeLabel(run, now)}</span>
          </Link>
        );
      })}
    </div>
  );
}

export default RecentActivity;
