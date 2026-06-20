// Live Events page — subscribes to the global SSE stream (/api/events/stream)
// and renders a scrolling timeline of all events, with a connection indicator.
// The Phase-1 global stream tails the most-recent active run; the page just
// renders whatever the stream sends.

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Radio } from 'lucide-react';
import { api, type RunEvent } from '../lib/api';
import { type ConnectionState, type SeqEvent, ConnectionBadge } from '../components/RunEventFeed';
import EventTimelineList from '../components/EventTimelineList';

const MAX_EVENTS = 2000;

export default function Events() {
  const [events, setEvents] = useState<SeqEvent[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const seqRef = useRef<number>(0);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [follow, setFollow] = useState(true);

  // Auto-scroll to tail while following.
  useLayoutEffect(() => {
    if (!follow) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [events.length, follow]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
      setFollow(nearBottom);
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, []);

  useEffect(() => {
    setEvents([]);
    seqRef.current = 0;
    setConnection('connecting');

    const unsubscribe = api.subscribeEvents(
      (ev: RunEvent) => {
        setConnection('live');
        const seq = ++seqRef.current;
        setEvents((prev) => {
          const trimmed = prev.length >= MAX_EVENTS ? prev.slice(prev.length - MAX_EVENTS + 1) : prev;
          return [...trimmed, { seq, event: ev }];
        });
      },
      undefined,
      () => setConnection('reconnecting'),
    );
    return unsubscribe;
  }, []);

  return (
    <div className="flex flex-col h-full min-h-0 px-8 py-6">
      <header className="flex items-center justify-between gap-4 mb-4">
        <div className="flex items-center gap-2">
          <Radio size={18} className="text-ink-dim" />
          <h1 className="text-2xl font-semibold text-ink">Live Events</h1>
        </div>
        <div className="flex items-center gap-3">
          <ConnectionBadge state={connection} />
          <span className="text-[11px] text-ink-mute tabular-nums">
            {events.length} event{events.length === 1 ? '' : 's'}
          </span>
        </div>
      </header>

      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-border bg-panel shadow-card"
      >
        {events.length === 0 ? (
          <div className="p-8 text-center text-sm text-ink-dim">
            Waiting for events…
          </div>
        ) : (
          <EventTimelineList events={events} />
        )}
      </div>
    </div>
  );
}
