// The shared timeline engine's pure half: what is revealed at a time, the
// span an axis covers, and how long a replay takes. The hook (useTimeline)
// and the axis (components/timeline/TimeAxis) sit on it.

export type TimelineKind = 'round' | 'child' | 'step' | 'unit' | 'message' | 'board';

export interface TimelineItem {
  key: string;
  /** Epoch ms. */
  at: number;
  kind: TimelineKind;
  /** The axis colours a tick by this (a message's row kind). */
  sub?: string;
}

export interface TimelineBand {
  key: string;
  label: string;
  from: number;
  /** `null` = still open (runs to now). */
  to: number | null;
  /** Which row the axis draws it on (default `round`). */
  row?: 'round' | 'step';
  /** `from` is a lower bound, not a recorded start: the axis draws its
   *  leading edge faded and dashed. */
  inferred?: boolean;
}

/** The span covering every item, band and (when given) now. */
export function timeBounds(
  items: TimelineItem[],
  bands: TimelineBand[],
  nowMs: number | null,
): { start: number; end: number } {
  let start = Infinity;
  let end = -Infinity;
  const see = (v: number) => {
    if (v < start) start = v;
    if (v > end) end = v;
  };
  for (const i of items) see(i.at);
  for (const b of bands) {
    see(b.from);
    if (b.to !== null) see(b.to);
  }
  if (nowMs !== null) see(nowMs);
  return start === Infinity ? { start: 0, end: 0 } : { start, end };
}

/** Keys of the items at or before `t`; `null` = everything. */
export function revealed(items: TimelineItem[], t: number | null): Set<string> {
  const out = new Set<string>();
  for (const i of items) if (t === null || i.at <= t) out.add(i.key);
  return out;
}

/** How many of `sorted` (by `at`) are at or before `t`. A reveal keyed on
 *  this count changes only when a replay frame reveals something. */
export function countAtOrBefore(sorted: readonly TimelineItem[], t: number): number {
  let lo = 0;
  let hi = sorted.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (sorted[mid].at <= t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Under reduced motion a replay steps (no sweep): at most this many steps,
 *  this far apart, each revealing what happened by then (it fades in). */
export const REDUCED_STEPS = 12;
export const REDUCED_STEP_MS = 700;

/** The instants a reduced-motion replay shows, in order: the start, then up
 *  to `max - 1` of the items' times after it (evenly picked when there are
 *  more), ending on the last; a run with nothing after its start is the
 *  start alone. */
export function reducedSteps(times: readonly number[], start: number, end: number, max = REDUCED_STEPS): number[] {
  const after = [...new Set(times.filter((t) => t > start && t <= end))].sort((a, b) => a - b);
  if (after.length === 0) return [start];
  const room = Math.max(1, max - 1);
  if (after.length <= room) return [start, ...after];
  const picked: number[] = [];
  for (let i = 1; i <= room; i++) picked.push(after[Math.round((i * (after.length - 1)) / room)]);
  return [start, ...new Set(picked)];
}

/** 40 ms of replay per second of run, clamped to 6-16 s. */
export function replayDuration(start: number, end: number): number {
  return Math.min(16000, Math.max(6000, ((end - start) / 1000) * 40));
}

/** What a tap on the axis at `t` names: a message tick within `slack` of it
 *  (the nearest), else the band (of `row`, when given) that covers `t`;
 *  `null` over nothing. `slack` is in ms (a few pixels' worth of the axis). */
export function pickAt(
  items: readonly TimelineItem[],
  bands: readonly TimelineBand[],
  t: number,
  slack: number,
  end: number,
): { kind: 'message' | 'band'; key: string } | null {
  let best: TimelineItem | null = null;
  for (const i of items) {
    if (i.kind !== 'message' || Math.abs(i.at - t) > slack) continue;
    if (!best || Math.abs(i.at - t) < Math.abs(best.at - t)) best = i;
  }
  if (best) return { kind: 'message', key: best.key };
  for (let k = bands.length - 1; k >= 0; k--) {
    const b = bands[k];
    if (b.from <= t && t <= (b.to ?? end)) return { kind: 'band', key: b.key };
  }
  return null;
}
