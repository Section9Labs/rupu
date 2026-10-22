import { describe, expect, it } from 'vitest';
import { bringIntoView, indexAt, mergeTicks, newestAt, roundAt, roundsInView, scrollTo } from './camera';
import { pickAt, type TimelineItem } from '../timeline/timeline';
import type { StepState } from '../runGraphModel';

const item = (key: string, at: number, kind: TimelineItem['kind']): TimelineItem => ({ key, at, kind });

describe('camera', () => {
  it('follows_the_newest_round_or_child_revealed_at_t', () => {
    const sorted = [
      item('round:0', 0, 'round'),
      item('run_a', 5, 'child'),
      item('msg_1', 6, 'message'),
      item('round:1', 10, 'round'),
      item('run_b', 12, 'child'),
      item('run_c', 12, 'child'),
      item('msg_2', 20, 'message'),
    ];
    const flow = new Set(['round', 'child']);
    expect(newestAt(sorted, -1, flow)).toBeNull();
    expect(newestAt(sorted, 0, flow)?.key).toBe('round:0');
    // A message landing is no card: the newest card stays followed.
    expect(newestAt(sorted, 7, flow)?.key).toBe('run_a');
    expect(newestAt(sorted, 11, flow)?.key).toBe('round:1');
    // Ties: the later one in order.
    expect(newestAt(sorted, 12, flow)?.key).toBe('run_c');
    expect(newestAt(sorted, 99, flow)?.key).toBe('run_c');
    // The run page follows steps.
    expect(newestAt([item('plan', 0, 'step'), item('u', 1, 'unit'), item('review', 3, 'step')], 2, new Set(['step']))?.key).toBe('plan');
  });

  it('moves_only_on_the_axis_a_box_is_out_of', () => {
    const pane = { width: 1000, height: 600 };
    const vp = { x: 0, y: 0, zoom: 1 };
    // In view: no move.
    expect(bringIntoView({ x: 100, y: 100, width: 200, height: 100 }, vp, pane)).toBeNull();
    // Below the view: only y moves, the box lands at 40% of the height.
    expect(bringIntoView({ x: 100, y: 2000, width: 200, height: 100 }, vp, pane)).toEqual({ x: 0, y: 240 - 2050, zoom: 1 });
    // Right of the view (and of the minimap's column): only x moves.
    const right = bringIntoView({ x: 900, y: 100, width: 200, height: 100 }, vp, pane, { reserveRight: 180 })!;
    expect(right.y).toBe(0);
    expect(right.x).toBeLessThan(0);
    // At a zoom: screen coordinates.
    expect(bringIntoView({ x: 400, y: 400, width: 200, height: 100 }, { x: 0, y: 0, zoom: 0.5 }, pane)).toBeNull();
    expect(scrollTo(1000, { x: 7, y: 0, zoom: 0.5 }, 600)).toEqual({ x: 7, y: 120 - 500, zoom: 0.5 });
  });

  it('merges_rounds_evenly_and_keeps_what_needs_a_look', () => {
    const states: StepState[] = Array.from({ length: 200 }, (_, i) => (i === 57 ? 'failed' : i === 199 ? 'running' : 'done'));
    const ticks = mergeTicks(states, 80);
    expect(ticks).toHaveLength(80);
    // Every round is in exactly one mark, in order.
    expect(ticks[0].from).toBe(0);
    expect(ticks[79].to).toBe(199);
    for (let k = 1; k < ticks.length; k++) expect(ticks[k].from).toBe(ticks[k - 1].to + 1);
    expect(new Set(ticks.map((t) => t.to - t.from + 1))).toEqual(new Set([2, 3]));
    // The failed round and the running one keep their colour.
    expect(ticks.find((t) => t.from <= 57 && 57 <= t.to)!.state).toBe('failed');
    expect(ticks[79].state).toBe('running');
    // Room for each: one mark per round.
    expect(mergeTicks(['done', 'running'], 80)).toEqual([
      { from: 0, to: 0, state: 'done' },
      { from: 1, to: 1, state: 'running' },
    ]);
    expect(mergeTicks([], 80)).toEqual([]);
  });

  it('finds_the_rounds_in_view_and_the_round_under_a_point', () => {
    const tops = [0, 300, 600, 900];
    expect(roundsInView(tops, 1100, 250, 650)).toEqual([0, 2]);
    expect(roundsInView(tops, 1100, 950, 2000)).toEqual([3, 3]);
    expect(roundsInView(tops, 1100, 1200, 2000)).toBeNull();
    expect(roundAt(tops, -50)).toBe(0);
    expect(roundAt(tops, 300)).toBe(1);
    expect(roundAt(tops, 899)).toBe(2);
    expect(roundAt(tops, 5000)).toBe(3);
    expect(indexAt(200, 0)).toBe(0);
    expect(indexAt(200, 0.5)).toBe(100);
    expect(indexAt(200, 1)).toBe(199);
    expect(indexAt(200, 1.4)).toBe(199);
  });

  it('a_tap_on_the_axis_names_a_tick_else_its_band', () => {
    const items = [item('msg_1', 100, 'message'), item('msg_2', 140, 'message'), item('run_a', 101, 'child')];
    const bands = [
      { key: 'round:0', label: 'Round 0', from: 0, to: 120 },
      { key: 'round:1', label: 'Round 1', from: 120, to: null },
    ];
    expect(pickAt(items, bands, 103, 5, 300)).toEqual({ kind: 'message', key: 'msg_1' });
    expect(pickAt(items, bands, 110, 5, 300)).toEqual({ kind: 'band', key: 'round:0' });
    expect(pickAt(items, bands, 250, 5, 300)).toEqual({ kind: 'band', key: 'round:1' });
    expect(pickAt(items, bands, 400, 5, 300)).toBeNull();
  });
});
