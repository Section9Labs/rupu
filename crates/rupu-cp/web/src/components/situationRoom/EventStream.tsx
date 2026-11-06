// Live Events — the center stream: the page's query line over the event rail
// (components/events/EventRail), every run's events and the newest findings
// merged newest first, the Needs-you deck pinned above them. The page filters
// the cards; this lays them out.

import type { ReactNode } from 'react';
import type { StreamCard } from '../../lib/situationRoom/cards';
import type { Resolved } from '../../lib/events/rail';
import EventRail from '../events/EventRail';
import type { EventGate } from '../events/GateButtons';
import { Rolling } from '../events/parts';

export default function EventStream({
  cards,
  filtered,
  queryBar,
  resolve,
  gate,
  terminal,
  isRunLive,
  liveRuns,
  hasMoreOlder,
  loadingOlder,
  onLoadOlder,
}: {
  /** The cards the query keeps, newest first. */
  cards: StreamCard[];
  /** Whether a query narrows them. */
  filtered: boolean;
  queryBar: ReactNode;
  resolve: (card: StreamCard) => Resolved;
  gate: EventGate;
  terminal: (runId: string) => string | undefined;
  isRunLive: (runId: string) => boolean;
  /** How many runs are working right now. */
  liveRuns: number;
  hasMoreOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder: () => void;
}) {
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="border-b border-border px-4 py-2.5">
        <div className="mb-2 flex items-center gap-3">
          <h2 className="m-0 text-meta font-semibold uppercase tracking-[0.08em] text-ink-mute">Live stream</h2>
          <span className="ml-auto font-mono text-note tabular-nums text-ink-mute" data-testid="stream-count">
            <Rolling value={cards.length} /> events
          </span>
        </div>
        {queryBar}
      </div>
      <EventRail
        className="flex-1"
        cards={cards}
        scope="live"
        resolve={resolve}
        gate={gate}
        terminal={terminal}
        isRunLive={isRunLive}
        live={liveRuns > 0}
        nowSub={liveRuns > 0 ? `${liveRuns} run${liveRuns === 1 ? '' : 's'} live · streaming` : 'nothing live · stream idle'}
        emptyText={filtered ? 'No events match the filter.' : 'Waiting for events…'}
        hasMoreOlder={hasMoreOlder}
        loadingOlder={loadingOlder}
        onLoadOlder={onLoadOlder}
        footer={
          !hasMoreOlder && cards.length > 0 ? <div className="evx-foot-note">Beginning of history</div> : undefined
        }
      />
    </div>
  );
}
