// The camera over a growing graph (the agentiflow map, the run graph): what a
// replay follows, where the viewport goes to show it, and the round
// navigator's ticks and span. Pure; no React (`useCamera` moves xyflow's
// viewport with these).

import type { StepState } from '../runGraphModel';
import type { TimelineItem } from '../timeline/timeline';

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Viewport {
  x: number;
  y: number;
  zoom: number;
}

/** A camera move's duration (eased in-out); a jump under reduced motion
 *  and for keyboard moves. */
export const CAMERA_MS = 350;

/** The newest item of `kinds` revealed at `t` (`sorted` by time; ties keep
 *  their order, the later one wins); `null` when none is. */
export function newestAt<T extends Pick<TimelineItem, 'at' | 'kind'>>(
  sorted: readonly T[],
  t: number,
  kinds: ReadonlySet<string>,
): T | null {
  let lo = 0;
  let hi = sorted.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid].at <= t) lo = mid + 1;
    else hi = mid;
  }
  for (let i = lo - 1; i >= 0; i--) if (kinds.has(sorted[i].kind)) return sorted[i];
  return null;
}

/** The viewport that brings `box` into view in a `pane` (screen px), or
 *  `null` when it already is, inside `margin` (and clear of `reserveRight`,
 *  the minimap's column). Only the axis the box is out of moves: it lands
 *  at `anchor` of the pane (0.5 = centred), at the same zoom. */
export function bringIntoView(
  box: Box,
  vp: Viewport,
  pane: { width: number; height: number },
  opts: { margin?: number; reserveRight?: number; anchor?: { x: number; y: number } } = {},
): Viewport | null {
  const margin = opts.margin ?? 32;
  const right = pane.width - (opts.reserveRight ?? 0) - margin;
  const anchor = opts.anchor ?? { x: 0.5, y: 0.4 };
  const sx = box.x * vp.zoom + vp.x;
  const sy = box.y * vp.zoom + vp.y;
  const sw = box.width * vp.zoom;
  const sh = box.height * vp.zoom;
  const outX = sx < margin || sx + sw > right;
  const outY = sy < margin || sy + sh > pane.height - margin;
  if (!outX && !outY) return null;
  const cx = box.x + box.width / 2;
  const cy = box.y + Math.min(box.height, pane.height / vp.zoom / 2) / 2;
  return {
    x: outX ? (right + margin) * anchor.x - cx * vp.zoom : vp.x,
    y: outY ? pane.height * anchor.y - cy * vp.zoom : vp.y,
    zoom: vp.zoom,
  };
}

/** The viewport with the flow-y `top` at `at` of the pane height. */
export function scrollTo(top: number, vp: Viewport, paneHeight: number, at = 0.2): Viewport {
  return { x: vp.x, y: paneHeight * at - top * vp.zoom, zoom: vp.zoom };
}

// --- the round navigator ------------------------------------------------------

/** One mark on the navigator: rounds `from`..`to` (indices, inclusive). */
export interface NavTick {
  from: number;
  to: number;
  state: StepState;
}

/** What a merged mark shows: the round that most needs a look. */
const RANK: Record<StepState, number> = {
  running: 6,
  failed: 5,
  awaiting_approval: 4,
  paused: 3,
  done: 2,
  pending: 1,
  skipped: 0,
};

/** The rounds as at most `slots` marks: consecutive rounds merge evenly
 *  (so 200 rounds on 80 slots are marks of 3 and 2), each mark the state
 *  that most needs a look among them. */
export function mergeTicks(states: readonly StepState[], slots: number): NavTick[] {
  const n = states.length;
  if (n === 0) return [];
  const marks = Math.max(1, Math.min(n, Math.floor(slots)));
  const out: NavTick[] = [];
  for (let k = 0; k < marks; k++) {
    const from = Math.floor((k * n) / marks);
    const to = Math.floor(((k + 1) * n) / marks) - 1;
    let state = states[from];
    for (let i = from + 1; i <= to; i++) if (RANK[states[i]] > RANK[state]) state = states[i];
    out.push({ from, to, state });
  }
  return out;
}

/** The rounds whose block (`tops[i]` to the next round's top, the last to
 *  `end`) meets the view `[top, bottom]`, as indices; `null` when none does. */
export function roundsInView(tops: readonly number[], end: number, top: number, bottom: number): [number, number] | null {
  let first = -1;
  let last = -1;
  for (let i = 0; i < tops.length; i++) {
    const a = tops[i];
    const b = i + 1 < tops.length ? tops[i + 1] : end;
    if (b > top && a < bottom) {
      if (first < 0) first = i;
      last = i;
    }
  }
  return first < 0 ? null : [first, last];
}

/** The round whose block holds flow-y `y` (the last top at or above it;
 *  the first round above them all). */
export function roundAt(tops: readonly number[], y: number): number {
  let i = 0;
  while (i + 1 < tops.length && tops[i + 1] <= y) i++;
  return i;
}

/** The round index at a fraction `f` (0..1) down a navigator of `n` rounds. */
export function indexAt(n: number, f: number): number {
  if (n <= 0) return 0;
  return Math.min(n - 1, Math.max(0, Math.floor(Math.max(0, Math.min(1, f)) * n)));
}
