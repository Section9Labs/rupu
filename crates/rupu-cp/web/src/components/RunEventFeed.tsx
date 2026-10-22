// The Events tab of a workflow run (and, through `cards`, of an agentiflow):
// one run's events on the shared event rail (components/events/EventRail) —
// the same rows, threads and Needs-you deck as Live Events, scoped to the
// run. Its gates are answered from the deck and the gate row through the run
// page's own gate control (the graph's), so all three show one decision.
//
// The SSE subscription lives one level up in RunDetail (shared with RunGraph);
// this renders the accumulated list.

import { useCallback, useMemo } from 'react';
import { cn } from '../lib/cn';
import { agentUnitIndex, cardFromEvent, unitKeyIndex, type StreamCard } from '../lib/situationRoom/cards';
import { identityOf } from '../lib/events/identity';
import EventRail from './events/EventRail';
import { IDLE_GATE, type EventGate } from './events/GateButtons';
import type { GateControl } from './graph/GateActions';

export type ConnectionState = 'connecting' | 'live' | 'reconnecting';

/** A RunEvent paired with a monotonically-increasing sequence id. */
export interface SeqEvent {
  seq: number;
  event: import('../lib/api').RunEvent;
}

/** Numeric `ts` if the event carries one (history rows do; a run's own log doesn't). */
function tsOf(ev: SeqEvent['event']): number {
  const raw = ev as Record<string, unknown>;
  return typeof raw.ts === 'number' ? raw.ts : 0;
}

const TERMINAL = new Set(['completed', 'failed', 'cancelled', 'rejected']);

export default function RunEventFeed({
  events,
  cards: externalCards,
  connection,
  crewByRun,
  gate,
  status,
}: {
  /** The run's SSE events; mapped to cards via `cardFromEvent`. */
  events?: SeqEvent[];
  /** Pre-built cards, when the caller's events are not orchestrator `RunEvent`s
   *  (the agentiflow Events tab — see `lib/agentiflowEvents`). Takes
   *  precedence over `events`. */
  cards?: StreamCard[];
  /** The live-connection indicator; omit it for a non-SSE feed (no badge). */
  connection?: ConnectionState;
  /** run_id → run codename; gives run-level cards (no codename of their own)
   *  the run's crew tint. */
  crewByRun?: ReadonlyMap<string, string>;
  /** The run page's gate control: Approve / Reject on gate rows and the deck. */
  gate?: GateControl;
  /** The run's status, when the page knows it (`running`, `completed`, …). */
  status?: string;
}) {
  // Newest first; keyed by content identity (so a row seen on Live Events is
  // seen here), with an occurrence suffix for an event the run logged twice.
  const cards = useMemo<StreamCard[]>(() => {
    if (externalCards) return [...externalCards].reverse().sort((a, b) => b.ts - a.ts);
    const list = events ?? [];
    const out: StreamCard[] = [];
    const evs = list.map((e) => e.event);
    const ctx = { unitKeys: unitKeyIndex(evs), agentUnits: agentUnitIndex(evs), crewByRun };
    const seen = new Map<string, number>();
    for (const { event } of list) {
      const id = identityOf(event);
      const n = seen.get(id) ?? 0;
      seen.set(id, n + 1);
      const c = cardFromEvent(event, tsOf(event), n === 0 ? id : `${id}#${n}`, ctx);
      if (c) out.push(c);
    }
    return out.reverse();
  }, [externalCards, events, crewByRun]);

  const count = externalCards ? externalCards.length : (events?.length ?? 0);
  const ended = status ? TERMINAL.has(status) : false;
  const live = !ended && (connection === 'live' || status === 'running' || status === 'awaiting_approval');

  const eventGate = useMemo<EventGate | undefined>(
    () =>
      gate && {
        state: (c) => (c.stepId ? gate.state(c.stepId) : IDLE_GATE),
        approve: (c) => c.stepId && gate.approve(c.stepId),
        reject: (c) => c.stepId && gate.reject(c.stepId),
      },
    [gate],
  );
  const terminal = useCallback(() => (ended ? status : undefined), [ended, status]);
  const isRunLive = useCallback(() => live, [live]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center justify-between px-1 pb-2">
        {connection ? <ConnectionBadge state={connection} /> : <span />}
        <span className="text-note tabular-nums text-ink-mute">
          {count} event{count === 1 ? '' : 's'}
        </span>
      </div>
      <EventRail
        className="flex-1 rounded-xl border border-border bg-panel shadow-card"
        cards={cards}
        scope="run"
        width="narrow"
        gate={eventGate}
        terminal={terminal}
        isRunLive={isRunLive}
        live={live}
        nowSub={live ? 'run live · streaming' : status ? `run ${status.replace(/_/g, ' ')}` : 'stream idle'}
        emptyText="Waiting for events…"
      />
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
      <span className={cn('inline-block h-1.5 w-1.5 rounded-full', m.dot, state !== 'live' && 'animate-pulse')} />
      {m.label}
    </span>
  );
}
