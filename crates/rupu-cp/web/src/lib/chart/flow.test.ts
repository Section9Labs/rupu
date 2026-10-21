import { describe, expect, it } from 'vitest';
import type { UsageTimelinePoint } from '../api';
import { binFlow, brushWindow, flowTotals, stageLanes, stageTotals, turnTicks, turnsPerBin } from './flow';
import { cubicBezier, EASE_OUT, lerpSeries, stylusAt } from './motion';

const pt = (label: string, tokens_in: number, tokens_out: number, tokens_cached = 0): UsageTimelinePoint => ({
  turn: 0,
  label,
  tokens_in,
  tokens_out,
  tokens_cached,
});

describe('token flow data', () => {
  it('rounds turns per bucket up to 1, 2, 5, 10…', () => {
    expect(turnsPerBin(100, 200)).toBe(1);
    expect(turnsPerBin(2531, 200)).toBe(20);
    expect(turnsPerBin(2531, 600)).toBe(5);
  });

  it('keeps each bucket’s largest turn, its stage and its cache reads', () => {
    const points = [pt('a', 100, 5, 20), pt('a', 900, 7, 300), pt('b', 400, 50), pt('b', 200, 3)];
    const rows = binFlow(points, 0, 3, 2);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ i0: 0, i1: 1, context: 900, cached: 300, out: 7, stage: 'a', sumIn: 1000 });
    expect(rows[1]).toMatchObject({ i0: 2, i1: 3, context: 400, out: 50, stage: 'b', sumOut: 53 });
  });

  it('aligns buckets to the full series whatever the window', () => {
    const points = Array.from({ length: 20 }, (_, i) => pt('a', i, 1));
    const rows = binFlow(points, 7, 13, 5);
    expect(rows.map((r) => [r.i0, r.i1])).toEqual([
      [5, 9],
      [10, 14],
    ]);
  });

  it('never lets cache reads exceed the context they are part of', () => {
    const rows = binFlow([pt('a', 100, 1, 500)], 0, 0, 1);
    expect(rows[0].cached).toBe(100);
    expect(flowTotals([pt('a', 100, 1, 500)]).cached).toBe(100);
  });

  it('splits stages into contiguous lanes, a repeated stage getting its own', () => {
    const lanes = stageLanes([pt('a', 1, 1), pt('a', 1, 1), pt('b', 1, 1), pt('a', 1, 1)]);
    expect(lanes).toEqual([
      { label: 'a', from: 0, to: 1, order: 0 },
      { label: 'b', from: 2, to: 2, order: 1 },
      { label: 'a', from: 3, to: 3, order: 0 },
    ]);
    expect(stageTotals([pt('a', 10, 1), pt('b', 5, 2), pt('a', 1, 1)])).toEqual([
      { label: 'a', turns: 2, tokensIn: 11, tokensOut: 2 },
      { label: 'b', turns: 1, tokensIn: 5, tokensOut: 2 },
    ]);
  });

  it('ticks turn numbers on one round step, labels apart', () => {
    // 2,531 turns over 1,000px: 0.4px a turn → a 200-turn step
    const ticks = turnTicks(0, 2530, 1000 / 2530);
    expect(ticks[0]).toBe(0);
    expect(ticks.slice(1, 4)).toEqual([199, 399, 599]);
    // zoomed to 30 turns: every 5th turn
    expect(turnTicks(100, 130, 1000 / 30).slice(0, 3)).toEqual([100, 104, 109]);
  });

  it('turns a drag into a window of point indices, ignoring slivers', () => {
    expect(brushWindow(100, 300, [0, 1000], { left: 0, right: 1000 })).toEqual([100, 300]);
    expect(brushWindow(300, 100, [0, 1000], { left: 0, right: 1000 })).toEqual([100, 300]);
    expect(brushWindow(100, 103, [0, 1000], { left: 0, right: 1000 })).toBeNull();
    expect(brushWindow(100, 120, [0, 10], { left: 0, right: 1000 })).toBeNull();
  });
});

describe('chart motion helpers', () => {
  it('solves a cubic-bezier timing curve', () => {
    const ease = cubicBezier(EASE_OUT);
    expect(ease(0)).toBe(0);
    expect(ease(1)).toBe(1);
    // a strong ease-out is well past halfway at a quarter of the time
    expect(ease(0.25)).toBeGreaterThan(0.6);
    expect(cubicBezier([0, 0, 1, 1])(0.5)).toBeCloseTo(0.5, 3);
  });

  it('morphs only between same-shaped series', () => {
    expect(lerpSeries([[0, 10]], [[10, 20]], 0.5)).toEqual([[5, 15]]);
    expect(lerpSeries([[0]], [[1, 2]], 0.5)).toBeNull();
    expect(lerpSeries([[0]], [[1], [2]], 0.5)).toBeNull();
  });

  it('puts the stylus on the trace at the sweep edge', () => {
    const linear = (t: number) => t;
    const p = stylusAt(0.5, [0, 100], [10, 30], 0, 100, linear);
    expect(p).toEqual({ x: 50, y: 20 });
    expect(stylusAt(0.5, [], [], 0, 100, linear)).toBeNull();
  });
});
