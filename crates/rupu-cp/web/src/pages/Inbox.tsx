// Inbox — the operator action board (its own route). It answers one question,
// "what is waiting on me, and can I clear it?", and is built to be worked to
// zero: every item can be acted on (approve/reject/resume), snoozed or
// dismissed. This split-out replaces the old dashboard "Needs you" hero so the
// Overview can be about system state and this can be a real queue.
//
// The one finding leads: a "stalled run time" ledger — the total time the
// fleet's runs have sat waiting on a human — which only falls when the operator
// acts. Rendering only; every data source is an existing endpoint (`useInbox`).

import { useState } from 'react';
import { useInbox } from '../lib/dashboard/useInbox';
import { InboxBoard, type InboxFilter } from '../components/inbox/InboxBoard';
import { PageHeader } from '../components/ui/PageHeader';
import { Segmented } from '../components/ui/Segmented';
import { Button } from '../components/ui/Button';
import { formatDuration } from '../lib/duration';
import { useScopedList } from '../lib/useScopedList';
import { ScopeChip } from '../components/customers/ScopeChip';

const FILTER_OPTIONS = [
  { value: 'all', label: 'All' },
  { value: 'gates', label: 'Gates' },
  { value: 'paused', label: 'Paused' },
  { value: 'failed', label: 'Failed' },
];

const HOUR = 3_600_000;

export default function Inbox() {
  const { customer } = useScopedList(undefined, []);
  const data = useInbox(customer);
  const [filter, setFilter] = useState<InboxFilter>('all');

  const waiting = data.actionable.length;
  const oldest = data.oldestActionable;
  const oldestAge =
    oldest && oldest.since ? formatDuration(data.now - Date.parse(oldest.since)) : null;
  const stalledUrgent = data.stalledMs >= HOUR;

  const subtitle = (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-1">
      <span>
        {waiting} waiting on you
        {data.counts.failed > 0 && <span className="text-ink-dim"> · {data.counts.failed} recent failures</span>}
      </span>
      <ScopeChip />
    </span>
  );

  return (
    <div className="space-y-2 p-4 lg:p-6">
      <PageHeader
        title="Inbox"
        subtitle={subtitle}
        actions={
          <>
            <Segmented
              ariaLabel="Filter inbox"
              options={FILTER_OPTIONS}
              value={filter}
              onChange={(v) => setFilter(v as InboxFilter)}
            />
            <Button variant="secondary" size="sm" onClick={() => data.refresh()}>
              Refresh
            </Button>
          </>
        }
      />

      {/* Stalled-run-time ledger — the one finding, derived from the same wait
          timestamps and falling only when the operator acts. */}
      {waiting > 0 && (
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 rounded-xl border border-border bg-panel px-4 py-3">
          <span
            className={`text-2xl font-semibold tabular-nums tracking-tight ${stalledUrgent ? 'text-status-failed' : 'text-ink'}`}
          >
            {formatDuration(data.stalledMs)}
          </span>
          <span className="text-ui text-ink-dim">
            of stalled run time across{' '}
            <span className="tabular-nums text-ink">{waiting}</span> waiting{' '}
            {waiting === 1 ? 'run' : 'runs'}
            {oldestAge && (
              <>
                {' · oldest '}
                <span className="tabular-nums text-ink">{oldestAge}</span>
              </>
            )}
          </span>
        </div>
      )}

      <div className="pt-2">
        <InboxBoard data={data} filter={filter} />
      </div>
    </div>
  );
}
