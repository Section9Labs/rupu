// Live Events page — subscribes to the global SSE stream (/api/events/stream)
// and renders a scrolling newest-first timeline of all events, with a
// connection indicator and Okesu-style tick-in animation.
//
// Newest-first / follow-the-top logic:
//   - New events are prepended (index 0 = newest). The auto-pin scrolls to
//     scrollTop=0 so new events appear immediately at the top.
//   - When the user scrolls DOWN to read history the follow pin is released.
//     A "Jump to latest" button appears so they can snap back.
//   - Following resumes automatically when the user scrolls back to the top
//     (scrollTop < FOLLOW_THRESHOLD).

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ArrowUp, Radio } from 'lucide-react';
import { api, type RunEvent } from '../lib/api';
import { type ConnectionState, type SeqEvent, ConnectionBadge } from '../components/RunEventFeed';
import EventTimelineList from '../components/EventTimelineList';
import { Button } from '../components/ui/Button';

const MAX_EVENTS = 2000;
// Pixel distance from the top within which we consider the user "at the top"
// and resume following.
const FOLLOW_THRESHOLD = 48;

export default function Events() {
  const [events, setEvents] = useState<SeqEvent[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const seqRef = useRef<number>(0);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  // follow=true → pin to top; flip to false when user scrolls down.
  const [follow, setFollow] = useState(true);

  // Track "fresh" seq IDs so EventTimelineList can apply the tick animation.
  // Each ID is removed after 2500ms (matching the Okesu style).
  const [liveIDs, setLiveIDs] = useState<ReadonlySet<number>>(new Set());

  // Pin to the top of the container when following (new events prepend).
  useLayoutEffect(() => {
    if (!follow) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [events.length, follow]);

  // Detect scroll-away-from-top (pause follow) and scroll-back-to-top (resume).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const atTop = el.scrollTop < FOLLOW_THRESHOLD;
      setFollow(atTop);
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

        // Prepend: newest at index 0.
        setEvents((prev) => {
          const next = [{ seq, event: ev }, ...prev];
          return next.length > MAX_EVENTS ? next.slice(0, MAX_EVENTS) : next;
        });

        // Mark as live/fresh for 2500ms.
        setLiveIDs((prev) => {
          const next = new Set(prev);
          next.add(seq);
          setTimeout(() => {
            setLiveIDs((p) => {
              const n = new Set(p);
              n.delete(seq);
              return n;
            });
          }, 2500);
          return next;
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
          <span className="text-note text-ink-mute tabular-nums">
            {events.length} event{events.length === 1 ? '' : 's'}
          </span>
        </div>
      </header>

      <div className="relative flex-1 min-h-0">
        <div
          ref={scrollRef}
          className="h-full overflow-y-auto rounded-xl border border-border bg-panel shadow-card"
        >
          {events.length === 0 ? (
            <div className="p-8 text-center text-sm text-ink-dim">
              Waiting for events…
            </div>
          ) : (
            <EventTimelineList
              events={events}
              liveIDs={liveIDs}
            />
          )}
        </div>

        {/* "Jump to latest" button — appears when user has scrolled away from top */}
        {!follow && events.length > 0 && (
          <Button
            onClick={() => {
              const el = scrollRef.current;
              if (el) el.scrollTo({ top: 0, behavior: 'smooth' });
              setFollow(true);
            }}
            className="absolute bottom-4 right-4 gap-1.5 px-3 py-2 text-xs rounded-full shadow-card"
          >
            <ArrowUp size={12} />
            Jump to latest
          </Button>
        )}
      </div>
    </div>
  );
}
