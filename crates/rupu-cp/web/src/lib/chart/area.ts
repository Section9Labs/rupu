// Pure geometry for the living stacked-area chart (the `Chart` component).
// Kept free of React/DOM so the numbers are unit-testable on their own: the
// component is a thin SVG renderer over these functions.
//
// Coordinate convention: SVG space, y grows DOWN. `Plot.y0` is the baseline
// (bottom of the plot, value 0); `Plot.yTop` is the top (value === scale max).
// `x0`/`x1` are the left/right plot edges.

export interface Plot {
  x0: number;
  x1: number;
  /** y of value 0 (the baseline, at the bottom). */
  y0: number;
  /** y of the scale max (the top). */
  yTop: number;
}

export interface Scale {
  /** The nice axis maximum (>= the data max). */
  max: number;
  /** Tick spacing. */
  step: number;
  /** Tick values, ascending, starting at 0 and ending at `max`. */
  ticks: number[];
}

/**
 * A "nice" axis scale for a non-negative max: a round step and a max at or
 * above the data, giving roughly `intervals` gridlines. Falls back to [0,1]
 * for a non-positive/empty max so a flat/empty chart still has an axis.
 */
export function niceScale(maxValue: number, intervals = 3): Scale {
  if (!Number.isFinite(maxValue) || maxValue <= 0) {
    return { max: 1, step: 1, ticks: [0, 1] };
  }
  const rawStep = maxValue / intervals;
  const mag = Math.pow(10, Math.floor(Math.log10(rawStep)));
  const norm = rawStep / mag;
  let niceNorm: number;
  if (norm <= 1) niceNorm = 1;
  else if (norm <= 2) niceNorm = 2;
  else if (norm <= 2.5) niceNorm = 2.5;
  else if (norm <= 5) niceNorm = 5;
  else niceNorm = 10;
  const step = niceNorm * mag;
  const max = Math.ceil(maxValue / step) * step;
  const ticks: number[] = [];
  // Guard the float accumulation with a half-step epsilon.
  for (let v = 0; v <= max + step / 2; v += step) ticks.push(Math.round(v * 1e6) / 1e6);
  return { max, step, ticks };
}

/** Evenly spaced x for `n` points across the plot. A single point sits at x0. */
export function xsFor(n: number, p: Plot): number[] {
  if (n <= 0) return [];
  if (n === 1) return [p.x0];
  const span = p.x1 - p.x0;
  return Array.from({ length: n }, (_, i) => p.x0 + (span * i) / (n - 1));
}

/** y for a value under a given scale max. */
export function yOf(value: number, max: number, p: Plot): number {
  const clamped = max <= 0 ? 0 : value / max;
  return p.y0 - clamped * (p.y0 - p.yTop);
}

/**
 * Per-series cumulative tops for a stacked chart: `out[s][i]` is the sum of
 * series `0..s` at index `i`. `out[-1]` (the bottom of series 0) is the
 * baseline (zeros), handled by the caller.
 */
export function cumulativeTops(series: number[][]): number[][] {
  const out: number[][] = [];
  const n = series[0]?.length ?? 0;
  const running = new Array(n).fill(0);
  for (const s of series) {
    for (let i = 0; i < n; i++) running[i] += s[i] ?? 0;
    out.push(running.slice());
  }
  return out;
}

/** The largest single value across every series — the y-scale max for an
 *  unstacked (overlaid line) chart, where series are NOT summed. */
export function seriesMax(series: number[][]): number {
  let m = 0;
  for (const s of series) for (const v of s) if (v > m) m = v;
  return m;
}

/** Column centres: `n` equal slots across the plot, each bar centred in its
 *  slot so the first and last bars sit fully inside the plot (unlike line/area
 *  points, which sit on the plot edges). */
export function bandCenters(n: number, p: Plot): number[] {
  if (n <= 0) return [];
  const slot = (p.x1 - p.x0) / n;
  return Array.from({ length: n }, (_, i) => p.x0 + slot * (i + 0.5));
}

/** A column's width: a fraction of its slot (never below 2px). */
export function bandWidth(n: number, p: Plot, frac = 0.62): number {
  if (n <= 0) return 0;
  return Math.max(2, ((p.x1 - p.x0) / n) * frac);
}

/** A rounded-top, square-bottom rect as an SVG path (`d`) — the column cap
 *  (app-screens §Charts: "4px top radius with a square bottom on the
 *  baseline"). Radius is clamped to half the width and the height. */
export function roundedTopRect(x: number, y: number, w: number, h: number, r: number): string {
  const rr = Math.max(0, Math.min(r, w / 2, h));
  return (
    `M ${round(x)} ${round(y + h)} L ${round(x)} ${round(y + rr)} ` +
    `Q ${round(x)} ${round(y)} ${round(x + rr)} ${round(y)} ` +
    `L ${round(x + w - rr)} ${round(y)} Q ${round(x + w)} ${round(y)} ${round(x + w)} ${round(y + rr)} ` +
    `L ${round(x + w)} ${round(y + h)} Z`
  );
}

/** Totals per index (top of the stack) — for the peak label and the y scale. */
export function totals(series: number[][]): number[] {
  const n = series[0]?.length ?? 0;
  const out = new Array(n).fill(0);
  for (const s of series) for (let i = 0; i < n; i++) out[i] += s[i] ?? 0;
  return out;
}

/** Index of the maximum total (first on a tie). -1 for empty. */
export function peakIndex(totalsArr: number[]): number {
  let best = -1;
  let bestV = -Infinity;
  for (let i = 0; i < totalsArr.length; i++) {
    if (totalsArr[i] > bestV) {
      bestV = totalsArr[i];
      best = i;
    }
  }
  return best;
}

/** `"x,y x,y …"` polyline points along a series' values. */
export function linePoints(vals: number[], xa: number[], max: number, p: Plot): string {
  return vals.map((v, i) => `${round(xa[i])},${round(yOf(v, max, p))}`).join(' ');
}

/**
 * A filled stacked band: forward along `topVals`, back along `bottomVals`
 * (or the baseline when omitted), closed. Empty string for no points.
 */
export function areaPath(
  topVals: number[],
  bottomVals: number[] | null,
  xa: number[],
  max: number,
  p: Plot,
): string {
  const n = topVals.length;
  if (n === 0) return '';
  let d = '';
  for (let i = 0; i < n; i++) {
    d += `${i === 0 ? 'M' : 'L'} ${round(xa[i])} ${round(yOf(topVals[i], max, p))} `;
  }
  if (bottomVals) {
    for (let i = n - 1; i >= 0; i--) {
      d += `L ${round(xa[i])} ${round(yOf(bottomVals[i], max, p))} `;
    }
  } else {
    d += `L ${round(xa[n - 1])} ${round(p.y0)} L ${round(xa[0])} ${round(p.y0)} `;
  }
  return `${d}Z`;
}

function round(v: number): number {
  return Math.round(v * 100) / 100;
}

/**
 * The tightest "nice" scale above `maxValue` with between `minTicks` and
 * `maxTicks` intervals: every round step (1, 2, 2.5, 5 × 10ⁿ) is tried and the
 * one whose top sits closest above the data wins, so a 402k peak gets a 500k
 * top (100k steps), not 600k. Ties go to fewer gridlines.
 */
export function tightScale(maxValue: number, minTicks = 3, maxTicks = 5): Scale {
  if (!Number.isFinite(maxValue) || maxValue <= 0) {
    return { max: 1, step: 1, ticks: [0, 1] };
  }
  const mag = Math.pow(10, Math.floor(Math.log10(maxValue)));
  let best: { max: number; step: number; count: number } | null = null;
  for (const m of [0.1, 0.2, 0.25, 0.5, 1, 2, 2.5, 5, 10]) {
    const step = m * mag;
    const count = Math.ceil(maxValue / step - 1e-9);
    if (count < minTicks || count > maxTicks) continue;
    const max = count * step;
    if (!best || max < best.max - 1e-9 || (Math.abs(max - best.max) < 1e-9 && count < best.count)) {
      best = { max, step, count };
    }
  }
  if (!best) return niceScale(maxValue, minTicks);
  const ticks: number[] = [];
  for (let k = 0; k <= best.count; k++) ticks.push(Math.round(k * best.step * 1e6) / 1e6);
  return { max: best.max, step: best.step, ticks };
}
