import { describe, expect, it } from 'vitest';
import { turnWindow, withinSpan } from './turnWindow';
import type { UsageTimelinePoint } from '../api';

const p = (turn: number, label: string, extra: Partial<UsageTimelinePoint> = {}): UsageTimelinePoint => ({
  turn,
  label,
  tokens_in: 10,
  tokens_out: 1,
  tokens_cached: 0,
  ...extra,
});

const T = (s: string) => `2026-10-21T10:00:${s}Z`;
const ms = (s: string) => Date.parse(T(s));

// two units of one fan-out interleaved, then a report step
const POINTS = [
  p(1, 'plan', { at: T('00'), transcript: '/t/plan.jsonl', transcript_turn: 1 }),
  p(2, 'assess', { at: T('05'), transcript: '/t/u0.jsonl', transcript_turn: 1 }),
  p(3, 'assess', { at: T('07'), transcript: '/t/u1.jsonl', transcript_turn: 1 }),
  p(4, 'assess', { at: T('09'), transcript: '/t/u0.jsonl', transcript_turn: 2 }),
  p(5, 'assess', { at: T('12'), transcript: '/t/u1.jsonl', transcript_turn: 2 }),
  p(6, 'report', { at: T('20'), transcript: '/t/report.jsonl', transcript_turn: 1 }),
];

describe('turnWindow', () => {
  it('reads the selected turns as transcripts, their call ranges, steps and a time span', () => {
    const w = turnWindow(POINTS, 2, 4)!;
    expect(w.turns).toEqual([3, 5]);
    expect([...w.transcripts]).toEqual([
      ['/t/u1.jsonl', [1, 2]],
      ['/t/u0.jsonl', [2, 2]],
    ]);
    expect(w.labels).toEqual(['assess']);
    // from the end of the call before the window (turn 2) to its last call's end
    expect(w.span).toEqual([ms('05'), ms('12')]);
    expect(w.partlyTimed).toBe(false);
  });

  it('clamps and orders the range, and starts the first window at its own first call', () => {
    const w = turnWindow(POINTS, 9, -3)!;
    expect([w.from, w.to]).toEqual([0, 5]);
    expect(w.span).toEqual([ms('00'), ms('20')]);
    expect(w.labels).toEqual(['plan', 'assess', 'report']);
  });

  it('has no span for a run that recorded no times, but still knows its transcripts', () => {
    const old = POINTS.map(({ at: _at, ...rest }) => rest);
    const w = turnWindow(old, 1, 2)!;
    expect(w.span).toBeNull();
    expect([...w.transcripts.keys()]).toEqual(['/t/u0.jsonl', '/t/u1.jsonl']);
  });

  it('flags a span built from only some of the turns', () => {
    const mixed = [...POINTS.slice(0, 3), p(4, 'assess', { transcript: '/t/u0.jsonl', transcript_turn: 2 })];
    const w = turnWindow(mixed, 1, 3)!;
    expect(w.span).toEqual([ms('00'), ms('07')]);
    expect(w.partlyTimed).toBe(true);
  });

  it('is null for an empty series', () => {
    expect(turnWindow([], 0, 3)).toBeNull();
  });
});

describe('withinSpan', () => {
  it('takes ms or ISO times, inclusive at both ends, and refuses a missing one', () => {
    const span: [number, number] = [ms('05'), ms('12')];
    expect(withinSpan(span, T('05'))).toBe(true);
    expect(withinSpan(span, ms('12'))).toBe(true);
    expect(withinSpan(span, ms('13'))).toBe(false);
    expect(withinSpan(span, null)).toBe(false);
    expect(withinSpan(span, 'not a time')).toBe(false);
  });
});
