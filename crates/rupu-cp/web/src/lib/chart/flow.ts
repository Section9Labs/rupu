// Pure data shaping for the token flow chart (components/charts/TokenFlow):
// turns binned to the pixels available, the stages as contiguous lanes, and
// the x ticks. No React, so it is unit-testable on its own.
//
// Token semantics (rupu_providers::Usage): `tokens_in` is the WHOLE prompt a
// turn sent, cache reads included; `tokens_cached` (cache reads) and
// `tokens_cache_write` are subsets of it. So a turn's context is
// `tokens_in`, and the cache band sits inside it, never on top of it.

import type { UsageTimelinePoint } from '../api';

/** One plotted bucket: a run of consecutive turns `[i0, i1]` (indices into the
 *  points). Peaks are kept (a bucket's context is its largest turn's) so a
 *  spike never disappears into an average; `typical` is the median. */
export interface FlowRow {
  i0: number;
  i1: number;
  /** The x position in point-index space (the bucket's centre). */
  at: number;
  /** The stage of the bucket's largest turn. */
  stage: string;
  context: number;
  /** Cache reads of that largest turn (≤ its context). */
  cached: number;
  out: number;
  typical: number;
  sumIn: number;
  sumOut: number;
  sumCached: number;
}

/** Turns per bucket so `count` turns fit about `maxBins` buckets, rounded up to
 *  1, 2, 5, 10, 20, 50… so bucket boundaries stay put while a view pans. */
export function turnsPerBin(count: number, maxBins: number): number {
  if (count <= maxBins || maxBins <= 0) return 1;
  const raw = count / maxBins;
  const mag = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 5, 10]) if (m * mag >= raw) return m * mag;
  return 10 * mag;
}

function median(vals: number[]): number {
  const s = [...vals].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)] ?? 0;
}

/** Bin `points[from..to]` (inclusive, clamped) into buckets of `per` turns.
 *  Buckets are aligned to multiples of `per` in the full series, so the same
 *  turns share a bucket whatever window is shown. */
export function binFlow(points: UsageTimelinePoint[], from: number, to: number, per: number): FlowRow[] {
  const n = points.length;
  if (n === 0) return [];
  const a = Math.max(0, Math.min(n - 1, Math.floor(from)));
  const b = Math.max(a, Math.min(n - 1, Math.ceil(to)));
  const step = Math.max(1, Math.floor(per));
  const rows: FlowRow[] = [];
  for (let s = Math.floor(a / step) * step; s <= b; s += step) {
    const e = Math.min(n - 1, s + step - 1);
    let best = s;
    let out = 0;
    let sumIn = 0;
    let sumOut = 0;
    let sumCached = 0;
    const ctx: number[] = [];
    for (let i = s; i <= e; i++) {
      const p = points[i];
      if (p.tokens_in > points[best].tokens_in) best = i;
      out = Math.max(out, p.tokens_out);
      sumIn += p.tokens_in;
      sumOut += p.tokens_out;
      sumCached += Math.min(p.tokens_cached, p.tokens_in);
      ctx.push(p.tokens_in);
    }
    const top = points[best];
    rows.push({
      i0: s,
      i1: e,
      at: (s + e) / 2,
      stage: top.label,
      context: top.tokens_in,
      cached: Math.min(top.tokens_cached, top.tokens_in),
      out,
      typical: median(ctx),
      sumIn,
      sumOut,
      sumCached,
    });
  }
  return rows;
}

/** A stage as a contiguous lane over point indices `[from, to]`. A stage that
 *  runs twice (resumed, or interleaved) gets a lane per stretch. */
export interface StageLane {
  label: string;
  from: number;
  to: number;
  /** Index of the stage in first-appearance order (for stable styling). */
  order: number;
}

export function stageLanes(points: UsageTimelinePoint[]): StageLane[] {
  const lanes: StageLane[] = [];
  const order = new Map<string, number>();
  points.forEach((p, i) => {
    if (!order.has(p.label)) order.set(p.label, order.size);
    const cur = lanes[lanes.length - 1];
    if (cur && cur.label === p.label) cur.to = i;
    else lanes.push({ label: p.label, from: i, to: i, order: order.get(p.label) as number });
  });
  return lanes;
}

/** Per stage, in first-appearance order: turns, context sent, output written. */
export interface StageTotal {
  label: string;
  turns: number;
  tokensIn: number;
  tokensOut: number;
}

export function stageTotals(points: UsageTimelinePoint[]): StageTotal[] {
  const by = new Map<string, StageTotal>();
  for (const p of points) {
    const t = by.get(p.label) ?? { label: p.label, turns: 0, tokensIn: 0, tokensOut: 0 };
    t.turns += 1;
    t.tokensIn += p.tokens_in;
    t.tokensOut += p.tokens_out;
    by.set(p.label, t);
  }
  return [...by.values()];
}

/** Whole-series totals. */
export function flowTotals(points: UsageTimelinePoint[]) {
  let tokensIn = 0;
  let tokensOut = 0;
  let cached = 0;
  for (const p of points) {
    tokensIn += p.tokens_in;
    tokensOut += p.tokens_out;
    cached += Math.min(p.tokens_cached, p.tokens_in);
  }
  return { tokensIn, tokensOut, cached };
}

/** Turn-number ticks (1-based ordinals) for the window `[from, to]` of point
 *  indices drawn `pxPerTurn` apart: one even step of 1, 2, 5, 10, 20, 50…
 *  with labels at least `minGap` px apart. The window's first turn is always
 *  a tick. Returns point indices. */
export function turnTicks(from: number, to: number, pxPerTurn: number, minGap = 72): number[] {
  const a = Math.ceil(from);
  const b = Math.floor(to);
  if (b < a) return [];
  let step = 1;
  outer: for (let mag = 1; mag < 1e9; mag *= 10) {
    for (const m of [1, 2, 5]) {
      if (m * mag * pxPerTurn >= minGap) {
        step = m * mag;
        break outer;
      }
    }
  }
  const out = [a];
  // ordinals are 1-based: ticks on turns step, 2·step… → indices step-1, …
  for (let t = Math.ceil((a + 1) / step) * step; t - 1 <= b; t += step) {
    const i = t - 1;
    if (i > a && (i - a) * pxPerTurn >= minGap * 0.75) out.push(i);
  }
  return out;
}

/** The point-index window a drag between pixel `x0` and `x1` selects, given
 *  the current window and plot edges. `null` for a drag too small to mean a
 *  zoom (under `minTurns` turns or 6px). */
export function brushWindow(
  x0: number,
  x1: number,
  view: [number, number],
  plot: { left: number; right: number },
  minTurns = 3,
): [number, number] | null {
  const lo = Math.min(x0, x1);
  const hi = Math.max(x0, x1);
  if (hi - lo < 6) return null;
  const w = plot.right - plot.left;
  const at = (x: number) => view[0] + ((Math.max(plot.left, Math.min(plot.right, x)) - plot.left) / w) * (view[1] - view[0]);
  const a = Math.floor(at(lo));
  const b = Math.ceil(at(hi));
  return b - a + 1 >= minTurns ? [a, b] : null;
}
