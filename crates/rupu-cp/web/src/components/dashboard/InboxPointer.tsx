// InboxPointer — the Overview's one line about what's waiting on the operator.
// It replaces the old "Needs you" hero: the dashboard no longer carries the
// action list (that's the Inbox now), it POINTS at it. A count and a breakdown,
// never the list itself. It reads the same `useInbox` data as the board, so the
// two always agree (a snoozed item doesn't inflate the count here either), and
// it never hides — when nothing waits it says so, so the operator learns where
// to look.

import { Link } from 'react-router-dom';
import { ArrowRight } from 'lucide-react';
import { useInbox } from '../../lib/dashboard/useInbox';
import { formatDuration } from '../../lib/duration';
import type { CustomerScope } from '../../lib/api';

export function InboxPointer({ customer }: { customer?: CustomerScope }) {
  const data = useInbox(customer);
  const waiting = data.actionable.length;
  const oldest = data.oldestActionable;
  const oldestAge =
    oldest && oldest.since ? formatDuration(data.now - Date.parse(oldest.since)) : null;
  const oldestKind = oldest?.kind === 'awaiting' ? 'gate' : oldest?.kind === 'paused' ? 'paused run' : null;

  // While the first load is in flight and we have nothing yet, keep the frame
  // quiet rather than flashing "nothing waiting".
  if (data.loading && waiting === 0 && data.counts.failed === 0) {
    return (
      <div className="h-11 animate-pulse rounded-xl border border-border bg-panel" aria-hidden />
    );
  }

  if (waiting === 0) {
    return (
      <Link
        to="/inbox"
        className="flex items-center gap-2 rounded-xl border border-border bg-panel px-4 py-2.5 text-ui text-ink-mute transition-colors hover:bg-surface/60"
      >
        <span>Nothing waiting on you.</span>
        <span className="ml-auto inline-flex items-center gap-1 font-medium text-ink-dim">
          Open inbox
          <ArrowRight size={13} aria-hidden />
        </span>
      </Link>
    );
  }

  return (
    <Link
      to="/inbox"
      className="flex flex-wrap items-center gap-x-2.5 gap-y-1 rounded-xl border border-border bg-panel px-4 py-2.5 transition-colors hover:bg-surface/60"
    >
      <span className="text-ui font-semibold text-ink">
        <span className="tabular-nums">{waiting}</span> waiting on you
      </span>
      <span className="flex items-center gap-1.5">
        {data.counts.gates > 0 && (
          <span className="rounded-full bg-status-awaiting/15 px-2 py-0.5 text-note font-semibold text-warn">
            {data.counts.gates} {data.counts.gates === 1 ? 'gate' : 'gates'}
          </span>
        )}
        {data.counts.paused > 0 && (
          <span className="rounded-full bg-status-paused/15 px-2 py-0.5 text-note font-semibold text-status-paused">
            {data.counts.paused} paused
          </span>
        )}
      </span>
      {oldestAge && oldestKind && (
        <span className="text-note text-ink-dim">
          · oldest a {oldestKind} waiting{' '}
          <span className="tabular-nums text-ink-dim">{oldestAge}</span>
        </span>
      )}
      <span className="ml-auto inline-flex items-center gap-1 text-note font-medium text-ink">
        Open inbox
        <ArrowRight size={13} aria-hidden />
      </span>
    </Link>
  );
}

export default InboxPointer;
