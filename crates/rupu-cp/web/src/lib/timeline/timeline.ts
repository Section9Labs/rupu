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

/** 40 ms of replay per second of run, clamped to 6-16 s. */
export function replayDuration(start: number, end: number): number {
  return Math.min(16000, Math.max(6000, ((end - start) / 1000) * 40));
}
