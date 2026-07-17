// swimlane — layout math for the live activity hero.
//
// Pure and I/O-free: this is the code that decides whether the hero reads
// correctly, so it is unit-tested rather than eyeballed.
//
// Recharts has no Gantt, so the view is hand-rolled SVG (same call Okesu made
// for its war-room case timeline). This module owns the math; the component
// owns the paint.

import type { ActiveRunBar } from '../api';

export type LaneKey = 'workflow' | 'host';

export interface PositionedBar {
  bar: ActiveRunBar;
  /** Fraction 0..1 of the fitted range. Clamped — never negative. */
  x0: number;
  x1: number;
}

export interface Lane {
  key: string;
  bars: PositionedBar[];
}

/** Floor for the fitted window, so a handful of 3-second runs still read. */
const MIN_SPAN_MS = 60_000;

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  // Index by `floor(n * p)`, matching Okesu's `timeline/scale.ts:autoFitRange`
  // — NOT `floor(p * (n - 1))`. With n=20 and a single outlier occupying
  // exactly 5% of the data, the `(n - 1)` form floors to index 0, i.e. the
  // outlier itself, which defeats percentile-fitting entirely for the case
  // it exists to handle.
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.floor(sorted.length * p)));
  return sorted[idx];
}

/**
 * Fit the x-axis to the 5th percentile start, not the earliest start.
 *
 * Fitting to min/max lets a single 6-hour run crush every other bar into ~2% of
 * the width. Lifted from Okesu's `timeline/scale.ts:autoFitRange`. Bars outside
 * the fitted window are clamped by `assignLanes`, not dropped.
 */
export function autoFitRange(bars: ActiveRunBar[], now: number): { start: number; end: number } {
  if (bars.length === 0) {
    return { start: now - MIN_SPAN_MS * 15, end: now };
  }
  const starts = bars.map((b) => Date.parse(b.started_at)).sort((a, b) => a - b);
  const p5 = percentile(starts, 0.05);
  const span = Math.max(MIN_SPAN_MS, now - p5);
  return { start: now - span, end: now };
}

/**
 * Bucket bars into lanes and position them within the fitted range.
 *
 * Active runs have no end time — they are still running — so every bar's right
 * edge is `now`. That is not a placeholder: the bar genuinely extends to the
 * present.
 */
export function assignLanes(bars: ActiveRunBar[], groupBy: LaneKey, now: number): Lane[] {
  const { start, end } = autoFitRange(bars, now);
  const span = Math.max(1, end - start);

  const laneOf = (b: ActiveRunBar): string =>
    groupBy === 'host' ? (b.host_id ?? 'local') : b.workflow_name;

  const byLane = new Map<string, PositionedBar[]>();
  for (const b of bars) {
    const t0 = Date.parse(b.started_at);
    // Clamp rather than drop: a bar fitted out of the window is still real work
    // and must be visible, clipped to the left edge.
    const x0 = Math.min(1, Math.max(0, (t0 - start) / span));
    const key = laneOf(b);
    const arr = byLane.get(key) ?? [];
    arr.push({ bar: b, x0, x1: 1 });
    byLane.set(key, arr);
  }

  return [...byLane.entries()]
    .map(([key, laneBars]) => ({
      key,
      bars: laneBars.sort((a, b) => a.x0 - b.x0),
    }))
    .sort((a, b) => a.key.localeCompare(b.key));
}
