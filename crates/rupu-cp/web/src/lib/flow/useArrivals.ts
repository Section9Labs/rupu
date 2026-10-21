// The packets: a message that lands after its space's log was read whole (a
// live one, or one a replay reveals) travels once. What a space's opening
// read brings, page by page or as a stream's replay, is the starting state,
// not news. Shared by the agentiflow map and the run graph's layer.

import { useEffect, useRef, useState } from 'react';
import type { FlowMessage } from './flowModel';

/** How long a newly revealed message's packet travels (and a bit). */
export const ARRIVE_MS = 1500;

/** The messages arriving now (id → until when), given the messages shown
 *  and the ids of the spaces that have caught up (comma-joined). */
export function useArrivals(shown: FlowMessage[], caughtUpKey: string): ReadonlyMap<string, number> {
  const seen = useRef(new Set<string>());
  /** The spaces that had caught up when the last messages were taken in. */
  const caughtUpBefore = useRef(new Set<string>());
  const [arrivals, setArrivals] = useState<ReadonlyMap<string, number>>(() => new Map());
  useEffect(() => {
    const fresh: string[] = [];
    for (const m of shown) {
      if (!seen.current.has(m.id) && caughtUpBefore.current.has(m.space)) fresh.push(m.id);
    }
    seen.current = new Set(shown.map((m) => m.id));
    caughtUpBefore.current = new Set(caughtUpKey ? caughtUpKey.split(',') : []);
    if (fresh.length === 0) return;
    const until = Date.now() + ARRIVE_MS;
    setArrivals((a) => {
      const next = new Map(a);
      for (const id of fresh) next.set(id, until);
      return next;
    });
  }, [shown, caughtUpKey]);
  useEffect(() => {
    if (arrivals.size === 0) return;
    let first = Infinity;
    for (const until of arrivals.values()) first = Math.min(first, until);
    const timer = setTimeout(
      () =>
        setArrivals((a) => {
          const now = Date.now();
          const next = new Map([...a].filter(([, until]) => until > now));
          return next.size === a.size ? a : next;
        }),
      Math.max(0, first - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [arrivals]);
  return arrivals;
}
