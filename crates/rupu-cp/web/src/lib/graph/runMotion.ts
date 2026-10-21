// Run-graph motion helpers (the CSS half lives in styles/graph-motion.css).
// Pure functions are exported for tests; the hook is thin.

import { useRef } from 'react';
import type { StepState } from '../runGraphModel';

/** Entrance stagger slots: nodes arrive column by column, left to right, so the
 *  graph reads in the order it runs. A slot is a node's column; nodes in one
 *  column share it, and the count is capped so a wide graph never waits long
 *  for its last column. */
export const MAX_ENTRANCE_SLOT = 10;

/** Each node id's entrance slot: the index of its column (distinct x, sorted),
 *  capped at {@link MAX_ENTRANCE_SLOT}. */
export function entranceSlots(positions: ReadonlyMap<string, { x: number }>): Map<string, number> {
  const columns = [...new Set([...positions.values()].map((p) => Math.round(p.x)))].sort((a, b) => a - b);
  const slotOf = new Map(columns.map((x, i) => [x, Math.min(i, MAX_ENTRANCE_SLOT)]));
  const out = new Map<string, number>();
  for (const [id, p] of positions) out.set(id, slotOf.get(Math.round(p.x)) ?? 0);
  return out;
}

/** Entrance slots that hold still: a node keeps the slot it had when it was
 *  first seen (so dragging it to another column never re-delays its
 *  entrance), and a node that appears later gets slot 0 — it arrives at once,
 *  not after a stagger meant for the first paint. */
export function useEntranceSlots(positions: ReadonlyMap<string, { x: number }>): ReadonlyMap<string, number> {
  const ref = useRef<Map<string, number> | null>(null);
  if (ref.current === null) {
    ref.current = entranceSlots(positions);
  } else {
    for (const id of positions.keys()) if (!ref.current.has(id)) ref.current.set(id, 0);
  }
  return ref.current;
}

/** An id that is safe inside `url(#…)` (edge ids carry `->` and `:`). */
export function svgId(prefix: string, id: string): string {
  return `${prefix}-${id.replace(/[^A-Za-z0-9_-]/g, '_')}`;
}

/** A one-shot cue a node plays when its state lands: `done` settles with a
 *  short confirm, `failed` flashes once. Only a change seen live plays one — a
 *  node first rendered already done or failed is quiet. */
export type StateCue = 'done' | 'failed';

export function cueFor(prev: StepState | undefined, next: StepState): StateCue | null {
  if (prev === undefined || prev === next) return null;
  if (next === 'done') return 'done';
  if (next === 'failed') return 'failed';
  return null;
}

/** The cue for the latest state change and a key that changes with each one,
 *  so the cue element remounts and plays once per change. */
export function useStateCue(state: StepState): { cue: StateCue | null; key: number } {
  const ref = useRef<{ state: StepState | undefined; cue: StateCue | null; key: number }>({
    state: undefined,
    cue: null,
    key: 0,
  });
  const r = ref.current;
  if (r.state !== state) {
    const cue = cueFor(r.state, state);
    if (cue) {
      r.cue = cue;
      r.key += 1;
    } else if (r.state !== undefined) {
      r.cue = null;
    }
    r.state = state;
  }
  return { cue: r.cue, key: r.key };
}

/** How an edge reads: `live` flows into a running step, `await` into a step
 *  that waits on the operator, `done` has been traversed, `idle` not yet. */
export type EdgeTone = 'idle' | 'done' | 'live' | 'await';

export function edgeTone(target: StepState | undefined): EdgeTone {
  switch (target) {
    case 'running':
      return 'live';
    case 'awaiting_approval':
      return 'await';
    case 'done':
    case 'failed':
    case 'paused':
      return 'done';
    default:
      return 'idle';
  }
}
