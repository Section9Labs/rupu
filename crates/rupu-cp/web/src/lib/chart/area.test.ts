import { describe, expect, it } from 'vitest';
import {
  areaPath,
  bandCenters,
  bandWidth,
  cumulativeTops,
  linePoints,
  niceScale,
  peakIndex,
  roundedTopRect,
  seriesMax,
  totals,
  xsFor,
  yOf,
  type Plot,
} from './area';

const P: Plot = { x0: 0, x1: 100, y0: 100, yTop: 0 };

describe('unstacked + column geometry', () => {
  it('seriesMax is the largest single value across series (never summed)', () => {
    expect(seriesMax([[1, 5, 2], [3, 2, 4]])).toBe(5);
    expect(seriesMax([])).toBe(0);
  });
  it('bandCenters centres n bars in n equal slots, inside the plot edges', () => {
    // slot = 100/2 = 50 → centres at 25, 75 (not on the 0/100 edges)
    expect(bandCenters(2, P)).toEqual([25, 75]);
    expect(bandCenters(0, P)).toEqual([]);
  });
  it('bandWidth is a fraction of the slot, never below 2px', () => {
    expect(bandWidth(2, P, 0.62)).toBeCloseTo(31);
    expect(bandWidth(200, P)).toBe(2); // clamped
  });
  it('roundedTopRect is a rounded-top, square-bottom path', () => {
    const d = roundedTopRect(0, 0, 10, 20, 3);
    expect(d.startsWith('M')).toBe(true);
    expect(d.trim().endsWith('Z')).toBe(true);
    expect(d).toContain('Q'); // the two top corners
  });
});

describe('niceScale', () => {
  it('rounds up to a nice max with ~3 intervals', () => {
    const s = niceScale(214);
    expect(s.max).toBeGreaterThanOrEqual(214);
    expect(s.ticks[0]).toBe(0);
    expect(s.ticks[s.ticks.length - 1]).toBe(s.max);
    // ticks are evenly stepped
    expect(s.ticks[1] - s.ticks[0]).toBeCloseTo(s.step);
  });

  it('handles small maxima', () => {
    const s = niceScale(6);
    expect(s.max).toBeGreaterThanOrEqual(6);
    expect(s.ticks[0]).toBe(0);
  });

  it('falls back for a zero/empty max', () => {
    expect(niceScale(0)).toEqual({ max: 1, step: 1, ticks: [0, 1] });
    expect(niceScale(-5).max).toBe(1);
  });
});

describe('xsFor', () => {
  it('spreads n points across the plot', () => {
    expect(xsFor(3, P)).toEqual([0, 50, 100]);
  });
  it('single point sits at x0; empty is empty', () => {
    expect(xsFor(1, P)).toEqual([0]);
    expect(xsFor(0, P)).toEqual([]);
  });
});

describe('yOf', () => {
  it('maps 0 to the baseline and max to the top', () => {
    expect(yOf(0, 200, P)).toBe(100);
    expect(yOf(200, 200, P)).toBe(0);
    expect(yOf(100, 200, P)).toBe(50);
  });
});

describe('cumulativeTops / totals / peakIndex', () => {
  const series = [
    [1, 5, 2],
    [2, 1, 2],
  ];
  it('stacks cumulatively per index', () => {
    expect(cumulativeTops(series)).toEqual([
      [1, 5, 2],
      [3, 6, 4],
    ]);
  });
  it('totals the stack', () => {
    expect(totals(series)).toEqual([3, 6, 4]);
  });
  it('finds the peak index (first on tie)', () => {
    expect(peakIndex(totals(series))).toBe(1);
    expect(peakIndex([])).toBe(-1);
  });
});

describe('paths', () => {
  it('linePoints lists x,y pairs', () => {
    expect(linePoints([0, 200], [0, 100], 200, P)).toBe('0,100 100,0');
  });
  it('areaPath to the baseline opens with M and closes with Z', () => {
    const d = areaPath([200, 200], null, [0, 100], 200, P);
    expect(d.startsWith('M')).toBe(true);
    expect(d.trim().endsWith('Z')).toBe(true);
    // returns to baseline (y0 = 100)
    expect(d).toContain('100');
  });
  it('areaPath with a bottom band walks back along it', () => {
    const d = areaPath([200, 200], [100, 100], [0, 100], 200, P);
    // top at y=0, bottom at y=50 (value 100 of max 200)
    expect(d).toContain('0 0');
    expect(d).toContain('50');
  });
  it('empty input yields empty path', () => {
    expect(areaPath([], null, [], 10, P)).toBe('');
  });
});
