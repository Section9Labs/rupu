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

/** The `pos` (0-based line index within its run's `events.jsonl`) a history
 *  row carries — see `getEvents` / `EventsCursor` in
 *  `crates/rupu-cp/src/api/events.rs`. Live SSE frames never have one. */
function posOfEvent(ev: RunEvent): number | undefined {
  const raw = ev as Record<string, unknown>;
  return typeof raw.pos === 'number' ? raw.pos : undefined;
}

/** Deterministic JSON.stringify with sorted object keys, so two objects with
 *  the same fields in a different order (or produced by different code
 *  paths) stringify identically. Used only to build `identityOf` below. */
function stableStringify(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(stableStringify).join(',')}]`;
  if (v !== null && typeof v === 'object') {
    const obj = v as Record<string, unknown>;
    const keys = Object.keys(obj).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${stableStringify(obj[k])}`).join(',')}}`;
  }
  return JSON.stringify(v);
}

/**
 * Stable content identity for an event — used to dedup the merged
 * history + live list. The firehose's initial-drain replays a currently
 * active run's already-written `events.jsonl` from offset 0 before tailing
 * (see `FileTailRunSource::open` / `tail_all_events_sse`), so the SAME
 * on-disk event occurrence arrives once via `getEvents` history and again
 * via SSE — at two different `ts` (history uses file-mtime/own-ts, SSE
 * stamps client arrival time), so `ts` can't be part of the identity.
 * `pos` similarly is only ever present on history rows (SSE frames don't
 * carry it), so it's excluded here too — the remaining fields (`type`,
 * `run_id`, `step_id`, `index`/`unit_key`, etc.) are exactly the event's own
 * payload and are identical between the two arrivals of the same
 * occurrence.
 */
function identityOf(ev: RunEvent): string {
  const raw = ev as Record<string, unknown>;
  const rest: Record<string, unknown> = {};
  for (const key of Object.keys(raw)) {
    if (key === 'ts' || key === 'pos') continue;
    rest[key] = raw[key];
  }
  return stableStringify(rest);
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
  // Content identities (see `identityOf`) of every event currently in
  // `events` — lets history and live SSE dedup an already-active run's
  // events that arrive via both paths (the SSE firehose replays a run's
  // full `events.jsonl` from offset 0 before tailing new appends — see
  // `FileTailRunSource::open` — so an active run's already-written events
  // land once in the history fetch and again as the first frames on SSE).
  const seenRef = useRef<Set<string>>(new Set());

  // Initial history load — so an idle page is never empty.
  useEffect(() => {
    let cancelled = false;
    api
      .getEvents(PAGE_SIZE)
      .then((rows) => {
        if (cancelled) return;
        seenRef.current = new Set();
        const tagged: SeqEvent[] = [];
        for (const ev of rows) {
          const id = identityOf(ev);
          if (seenRef.current.has(id)) continue; // defensive: guard against dup rows within one page too
          seenRef.current.add(id);
          tagged.push({ seq: ++seqRef.current, event: ev as RunEvent });
        }
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
  // events from starting to arrive). Dedups against `seenRef` so the
  // initial-drain replay of an already-active run's history (see comment on
  // `seenRef`) doesn't render a second row for an event already loaded from
  // history.
  useEffect(() => {
    setConnection('connecting');
    const unsubscribe = api.subscribeEvents(
      (ev) => {
        setConnection('live');
        const id = identityOf(ev);
        if (seenRef.current.has(id)) return; // same occurrence already rendered (history or an earlier SSE frame)
        seenRef.current.add(id);
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
    const oldest = current[current.length - 1].event;
    const oldestTs = tsOfEvent(oldest);
    if (oldestTs == null) return;
    // `pos` (+ `run_id`, already on every event) lets the backend resume
    // exactly after the oldest-loaded row instead of at a `ts` boundary —
    // without it, a run emitting more events than one page (all sharing
    // that run's fallback `ts`) would have the rest permanently excluded by
    // a `before_ts`-only cursor. See `EventsCursor` in
    // crates/rupu-cp/src/api/events.rs.
    const oldestPos = posOfEvent(oldest);
    setLoadingOlder(true);
    try {
      const older = await api.getEvents(PAGE_SIZE, oldestTs, oldest.run_id, oldestPos);
      if (older.length === 0) {
        setHasMoreOlder(false);
        return;
      }
      const tagged: SeqEvent[] = [];
      for (const ev of older) {
        const id = identityOf(ev);
        if (seenRef.current.has(id)) continue; // defensive: shouldn't happen with a compound cursor, but never double-render
        seenRef.current.add(id);
        tagged.push({ seq: ++seqRef.current, event: ev as RunEvent });
      }
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
