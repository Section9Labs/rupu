import { describe, expect, it } from 'vitest';
import {
  cumulativeTops,
  niceScale,
  peakIndex,
  seriesMax,
  tightScale,
  totals,
  xsFor,
  yOf,
  type Plot,
} from './area';

const P: Plot = { x0: 0, x1: 100, y0: 100, yTop: 0 };

describe('unstacked geometry', () => {
  it('seriesMax is the largest single value across series (never summed)', () => {
    expect(seriesMax([[1, 5, 2], [3, 2, 4]])).toBe(5);
    expect(seriesMax([])).toBe(0);
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

describe('tightScale', () => {
  it('picks the round top closest above the data', () => {
    // a 402k peak gets a 500k top in 100k steps, not 600k
    expect(tightScale(414_000, 3, 5)).toMatchObject({ max: 500_000, step: 100_000 });
    expect(tightScale(414_000, 3, 5).ticks).toEqual([0, 100_000, 200_000, 300_000, 400_000, 500_000]);
  });
  it('keeps the interval count within bounds', () => {
    const s = tightScale(19, 2, 3);
    expect(s.ticks.length - 1).toBeGreaterThanOrEqual(2);
    expect(s.ticks.length - 1).toBeLessThanOrEqual(3);
    expect(s.max).toBeGreaterThanOrEqual(19);
  });
  it('falls back for a zero/empty max', () => {
    expect(tightScale(0)).toEqual({ max: 1, step: 1, ticks: [0, 1] });
  });
});
