// A range of the Token flow's turns, read as a window over the run: the turns
// it covers, the transcripts (and which of their model calls) they come from,
// and when it happened. The run page narrows its Transcript, Events, Findings
// and Messages to it.

import type { UsageTimelinePoint } from '../api';

export interface TurnWindow {
  /** The selected point indices, inclusive. */
  from: number;
  to: number;
  /** The 1-based run-wide turn numbers of those points. */
  turns: [number, number];
  /** Each transcript the window touches → the 1-based range of its model
   *  calls inside the window, in the order the window first meets them. */
  transcripts: Map<string, [number, number]>;
  /** The step labels the window touches, in order. */
  labels: string[];
  /** When the window ran, in ms since the epoch: from the end of the call just
   *  before it (so the first turn's own work is in) to the end of its last
   *  call. Null when the run did not record times for these turns. */
  span: [number, number] | null;
  /** Some (but not all) of the window's turns carry no time: the span is from
   *  the ones that do. */
  partlyTimed: boolean;
}

function timeOf(p: UsageTimelinePoint | undefined): number | null {
  if (!p?.at) return null;
  const t = Date.parse(p.at);
  return Number.isFinite(t) ? t : null;
}

/** The window over `points[from..to]` (clamped to the series). */
export function turnWindow(points: readonly UsageTimelinePoint[], from: number, to: number): TurnWindow | null {
  if (points.length === 0) return null;
  const a = Math.max(0, Math.min(from, to));
  const b = Math.min(points.length - 1, Math.max(from, to));
  if (a > b) return null;

  const transcripts = new Map<string, [number, number]>();
  const labels: string[] = [];
  let first: number | null = null;
  let last: number | null = null;
  let untimed = 0;
  for (let i = a; i <= b; i++) {
    const p = points[i];
    if (!labels.includes(p.label)) labels.push(p.label);
    if (p.transcript && p.transcript_turn != null) {
      const r = transcripts.get(p.transcript);
      if (r) transcripts.set(p.transcript, [Math.min(r[0], p.transcript_turn), Math.max(r[1], p.transcript_turn)]);
      else transcripts.set(p.transcript, [p.transcript_turn, p.transcript_turn]);
    }
    const t = timeOf(p);
    if (t === null) {
      untimed++;
      continue;
    }
    first = first === null ? t : Math.min(first, t);
    last = last === null ? t : Math.max(last, t);
  }
  let span: [number, number] | null = null;
  if (first !== null && last !== null) {
    // The window's first turn began when the call before it finished.
    const before = a > 0 ? timeOf(points[a - 1]) : null;
    span = [before !== null && before <= first ? before : first, last];
  }
  return {
    from: a,
    to: b,
    turns: [points[a].turn, points[b].turn],
    transcripts,
    labels,
    span,
    partlyTimed: span !== null && untimed > 0,
  };
}

/** Whether a time (ms, or an ISO string) falls inside the window's span. */
export function withinSpan(span: [number, number], at: number | string | null | undefined): boolean {
  const t = typeof at === 'string' ? Date.parse(at) : at;
  if (t == null || !Number.isFinite(t)) return false;
  return t >= span[0] && t <= span[1];
}
