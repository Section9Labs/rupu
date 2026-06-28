// rupu-native live event timeline — a scrolling vertical feed of a run's SSE
// events. Built (not the 1370-line Okesu EventTimeline port), but reuses
// Okesu's `.timeline-*` CSS animations from styles.css and the same lucide
// icon + colored-dot visual language.
//
// The SSE subscription lives ONE level up in RunDetail (shared with RunGraph),
// so this component is a pure render of the accumulated event list plus a small
// connection indicator. Auto-scrolls to newest unless the user has scrolled up.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { cn } from '../lib/cn';
import EventTimelineList from './EventTimelineList';

export type ConnectionState = 'connecting' | 'live' | 'reconnecting';

/** A RunEvent paired with a monotonically-increasing sequence id for stable React keys. */
export interface SeqEvent {
  seq: number;
  event: import('../lib/api').RunEvent;
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
  // Flips false when the user scrolls down to read history; resumes when they
  // scroll back to the top. Matches the global Live Events page.
  const [follow, setFollow] = useState(true);

  // Render newest-first (latest at top) — a timeline, not a chat log. The
  // parent accumulates events oldest-first, so reverse a shallow copy.
  const ordered = useMemo(() => events.slice().reverse(), [events]);

  // Stick to the top on new events while following.
  useLayoutEffect(() => {
    if (!follow) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [events.length, follow]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const atTop = el.scrollTop < 48;
      setFollow(atTop);
    };
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
        className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-border bg-panel shadow-card"
      >
        {ordered.length === 0 ? (
          <div className="p-8 text-center text-sm text-ink-dim">
            Waiting for events…
          </div>
        ) : (
          <EventTimelineList events={ordered} />
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
