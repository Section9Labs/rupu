// Live Events page — a global timeline combining recent HISTORY
// (`GET /api/events`) with the live SSE firehose (`GET /api/events/stream`).
// The page is never empty while idle: history loads on mount and renders
// immediately; live events then prepend on top of it as they arrive.
//
// This file owns data (history fetch, SSE subscription, lazy-load-older
// paging) and the page chrome (title, connection badge, event count); the
// grouped/filterable/day-sectioned rendering lives in `EventTimeline`.

import { useCallback, useEffect, useRef, useState } from 'react';
import { Radio } from 'lucide-react';
import { api, type RunEvent } from '../lib/api';
import { type ConnectionState, ConnectionBadge, type SeqEvent } from '../components/RunEventFeed';
import EventTimeline from '../components/EventTimeline';

// Default page size for both the initial history load and each lazy
// "load older" page. Matches the backend's own default (see
// `DEFAULT_RECENT_EVENTS_LIMIT` in crates/rupu-cp/src/api/events.rs) so a
// full first page reliably signals "there may be more."
const PAGE_SIZE = 200;
// Hard ceiling on the in-memory event list — lazy loading lets the operator
// scroll back through history; this cap prevents unbounded growth on a long
// session left open.
const MAX_EVENTS = 5000;

/** Stamp a client-side arrival `ts` onto a live SSE frame that doesn't
 *  already carry one (only RunStarted/RunCompleted/RunFailed do — see
 *  `event_own_ts_ms` in crates/rupu-cp/src/api/events.rs). History rows
 *  from `getEvents` always have `ts` already, so this is a no-op for them. */
function withArrivalTs(ev: RunEvent, fallbackTs: number): RunEvent {
  const raw = ev as Record<string, unknown>;
  return typeof raw.ts === 'number' ? ev : { ...ev, ts: fallbackTs };
}

function tsOfEvent(ev: RunEvent): number | undefined {
  const raw = ev as Record<string, unknown>;
  return typeof raw.ts === 'number' ? raw.ts : undefined;
}

export default function Events() {
  const [events, setEvents] = useState<SeqEvent[]>([]);
  const [connection, setConnection] = useState<ConnectionState>('connecting');
  const [hasMoreOlder, setHasMoreOlder] = useState(true);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [liveIDs, setLiveIDs] = useState<ReadonlySet<number>>(new Set());
  const seqRef = useRef(0);
  // Mirrors `events` for `loadOlder`, which is wrapped in `useCallback` but
  // must always see the latest list without re-subscribing to anything.
  const eventsRef = useRef<SeqEvent[]>([]);
  eventsRef.current = events;

  // Initial history load — so an idle page is never empty.
  useEffect(() => {
    let cancelled = false;
    api
      .getEvents(PAGE_SIZE)
      .then((rows) => {
        if (cancelled) return;
        const tagged = rows.map((ev) => ({ seq: ++seqRef.current, event: ev as RunEvent }));
        setEvents(tagged);
        setHasMoreOlder(rows.length >= PAGE_SIZE);
        setHistoryError(null);
      })
      .catch((err) => {
        if (cancelled) return;
        setHistoryError(`Could not load event history: ${String(err)}`);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Live SSE — prepends on top of whatever history is loaded, independent
  // of the history fetch above (a slow history load doesn't block live
  // events from starting to arrive).
  useEffect(() => {
    setConnection('connecting');
    const unsubscribe = api.subscribeEvents(
      (ev) => {
        setConnection('live');
        const seq = ++seqRef.current;
        const tagged: SeqEvent = { seq, event: withArrivalTs(ev, Date.now()) };
        setEvents((prev) => [tagged, ...prev].slice(0, MAX_EVENTS));
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

  const loadOlder = useCallback(async () => {
    const current = eventsRef.current;
    if (current.length === 0) return;
    const oldestTs = tsOfEvent(current[current.length - 1].event);
    if (oldestTs == null) return;
    setLoadingOlder(true);
    try {
      const older = await api.getEvents(PAGE_SIZE, oldestTs);
      if (older.length === 0) {
        setHasMoreOlder(false);
        return;
      }
      const tagged = older.map((ev) => ({ seq: ++seqRef.current, event: ev as RunEvent }));
      setEvents((prev) => [...prev, ...tagged]);
      if (older.length < PAGE_SIZE) setHasMoreOlder(false);
    } catch {
      // Quiet — the operator can retry via the "Load older events" control.
    } finally {
      setLoadingOlder(false);
    }
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

      {historyError && (
        <div className="mb-4 text-xs text-err bg-err-bg border border-err/30 px-3 py-2 rounded-md">
          {historyError}
        </div>
      )}

      <EventTimeline
        events={events}
        liveIDs={liveIDs}
        hasMoreOlder={hasMoreOlder}
        loadingOlder={loadingOlder}
        onLoadOlder={loadOlder}
      />
    </div>
  );
}
