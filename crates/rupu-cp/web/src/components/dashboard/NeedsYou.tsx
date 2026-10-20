// NeedsYou — the dashboard triage hero (spec / dashboard-redesign.html): the
// biggest block answers "what is blocked on me?". Parked gates and paused runs
// carry inline actions (approve / reject / resume — the real mutations), recent
// failures link out to the run. Backed by `useNeedsYou`; this component is
// presentational (takes the hook result) so it renders without network in tests.
//
// Visual language: one StatusChip per row (dot for the live state), mono
// codename, a meta line that names its baseline ("waiting 2h 14m"), a `solid`
// primary action + a `secondary`. Loading shows skeleton rows in the real
// layout; empty keeps the frame with one plain sentence.

import { Link } from 'react-router-dom';
import { Button } from '../ui/Button';
import { StatusChip, type ChipTone } from '../ui/StatusChip';
import { EmptyState } from '../ui/EmptyState';
import { Skeleton } from '../ui/Skeleton';
import { AgentName } from '../codename/AgentName';
import { formatDuration } from '../../lib/duration';
import { runHref } from '../../lib/runs';
import type { NeedsYouItem, UseNeedsYou } from '../../lib/dashboard/useNeedsYou';

const KIND_CHIP: Record<NeedsYouItem['kind'], { tone: ChipTone; label: string }> = {
  awaiting: { tone: 'awaiting_approval', label: 'gate' },
  paused: { tone: 'paused', label: 'paused' },
  failed: { tone: 'failed', label: 'failed' },
};

function sinceLabel(item: NeedsYouItem, now: number): string {
  if (!item.since) return item.kind === 'failed' ? 'failed' : 'waiting';
  const ago = formatDuration(now - Date.parse(item.since));
  return item.kind === 'failed' ? `failed ${ago} ago` : `waiting ${ago}`;
}

function Row({
  item,
  data,
  now,
}: {
  item: NeedsYouItem;
  data: UseNeedsYou;
  now: number;
}) {
  const chip = KIND_CHIP[item.kind];
  const busy = data.pending[item.id] != null;
  const href = runHref({ id: item.id, host_id: item.host === 'local' ? undefined : item.host });

  return (
    <div className="flex items-center gap-3 border-t border-border px-4 py-3 first:border-t-0 hover:bg-surface">
      <StatusChip tone={chip.tone} dot>
        {chip.label}
      </StatusChip>
      <Link to={href} className="min-w-0 truncate hover:underline">
        <AgentName codename={item.codename} derived={item.codenameDerived} showCrew />
      </Link>
      <span className="min-w-0 flex-1 truncate text-note text-ink-mute">
        {item.workflowName} · {sinceLabel(item, now)}
      </span>

      <div className="flex shrink-0 items-center gap-2">
        {item.kind === 'awaiting' && (
          <>
            <Button variant="secondary" size="sm" disabled={busy} onClick={() => data.reject(item)}>
              Reject
            </Button>
            <Button variant="solid" size="sm" disabled={busy} onClick={() => data.approve(item)}>
              {data.pending[item.id] === 'approve' ? 'Approving…' : 'Approve'}
            </Button>
          </>
        )}
        {item.kind === 'paused' && (
          <Button variant="solid" size="sm" disabled={busy} onClick={() => data.resume(item)}>
            {data.pending[item.id] === 'resume' ? 'Resuming…' : 'Resume'}
          </Button>
        )}
        {item.kind === 'failed' && (
          <Link
            to={href}
            className="inline-flex items-center justify-center rounded-md border border-border bg-panel px-2.5 py-1 text-note font-medium text-ink transition-colors hover:bg-bg"
          >
            View run
          </Link>
        )}
      </div>
    </div>
  );
}

export function NeedsYou({ data }: { data: UseNeedsYou }) {
  const now = Date.now();

  if (data.loading && data.items.length === 0) {
    return (
      <div className="overflow-hidden rounded-xl border border-border bg-panel">
        {[0, 1, 2].map((i) => (
          <div key={i} className="flex items-center gap-3 border-t border-border px-4 py-3 first:border-t-0">
            <Skeleton className="h-5 w-16 rounded-full" />
            <Skeleton className="h-4 w-40" />
            <span className="flex-1" />
            <Skeleton className="h-7 w-20 rounded-md" />
          </div>
        ))}
      </div>
    );
  }

  if (data.items.length === 0) {
    return (
      <EmptyState
        title="Nothing needs you"
        hint="Parked gates, paused runs and recent failures show up here."
      />
    );
  }

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-panel">
      {data.items.map((item) => (
        <Row key={`${item.host}:${item.id}`} item={item} data={data} now={now} />
      ))}
    </div>
  );
}

export default NeedsYou;
