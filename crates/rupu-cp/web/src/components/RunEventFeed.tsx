// Per-run live event feed — a scrolling vertical timeline of one run's SSE
// events. Now renders the shared Situation Room `EventCard`s (same visual
// language as the global /events wall), instead of the old EventTimelineList.
//
// The SSE subscription lives ONE level up in RunDetail (shared with RunGraph),
// so this is a pure render of the accumulated event list plus a connection
// indicator. Auto-scrolls to newest unless the user has scrolled up. Approval
// is handled by RunDetail's own gate banner, so await cards here show status
// only (no inline Approve/Reject) — hence no onApprove/onReject wiring.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { cn } from '../lib/cn';
import { cardFromEvent, type StreamCard } from '../lib/situationRoom/cards';
import EventCard from './situationRoom/EventCard';

export type ConnectionState = 'connecting' | 'live' | 'reconnecting';

/** A RunEvent paired with a monotonically-increasing sequence id for stable React keys. */
export interface SeqEvent {
  seq: number;
  event: import('../lib/api').RunEvent;
}

/** Numeric `ts` if the event carries one (history rows do; raw SSE frames don't). */
function tsOf(ev: SeqEvent['event']): number {
  const raw = ev as Record<string, unknown>;
  return typeof raw.ts === 'number' ? raw.ts : 0;
}

export default function RunEventFeed({
  events,
  connection,
}: {
  events: SeqEvent[];
  connection: ConnectionState;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Newest-first: follow=true pins to the TOP (new events prepend visually).
  const [follow, setFollow] = useState(true);

  // Newest-first cards; drop events that carry nothing worth a row (e.g. a
  // note-less step_working heartbeat → cardFromEvent returns null).
  const cards = useMemo<StreamCard[]>(() => {
    const out: StreamCard[] = [];
    for (let i = events.length - 1; i >= 0; i--) {
      const { seq, event } = events[i];
      const c = cardFromEvent(event, tsOf(event), `run-ev-${seq}`);
      if (c) out.push(c);
    }
    return out;
  }, [events]);

  useLayoutEffect(() => {
    if (!follow) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [events.length, follow]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => setFollow(el.scrollTop < 48);
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, []);

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center justify-between px-1 pb-2">
        <ConnectionBadge state={connection} />
        <span className="text-note text-ink-mute tabular-nums">
          {events.length} event{events.length === 1 ? '' : 's'}
        </span>
      </div>

      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-border bg-panel shadow-card p-3"
      >
        {cards.length === 0 ? (
          <div className="p-8 text-center text-sm text-ink-dim">Waiting for events…</div>
        ) : (
          <div className="flex flex-col gap-2.5">
            {cards.map((card) => (
              <EventCard key={card.key} card={card} hideRunLink />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export function ConnectionBadge({ state }: { state: ConnectionState }) {
  const map: Record<ConnectionState, { label: string; dot: string; text: string }> = {
    connecting: { label: 'Connecting', dot: 'bg-warn', text: 'text-warn' },
    live: { label: 'Live', dot: 'bg-ok', text: 'text-ok' },
    reconnecting: { label: 'Reconnecting', dot: 'bg-warn', text: 'text-warn' },
  };
  const m = map[state];
  return (
    <span className={cn('inline-flex items-center gap-1.5 text-note font-medium', m.text)}>
      <span className={cn('inline-block w-1.5 h-1.5 rounded-full', m.dot, state !== 'live' && 'animate-pulse')} />
      {m.label}
    </span>
  );
}
