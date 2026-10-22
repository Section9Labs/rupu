import { describe, expect, it } from 'vitest';
import { LANE, RAIL_LANE, routeArc, spreadOf, stubRoute } from './arcRoute';
import { cutAt, pointAt, roundedPath } from '../graph/polyline';

const card = (x: number, y: number) => ({ x, y, width: 200, height: 100 });

describe('arcRoute', () => {
  it('a_column_to_the_right_runs_along_the_row_then_the_gutter_before_it', () => {
    // A at column 0, B two columns right and lower.
    expect(routeArc(card(0, 0), card(512, 300))).toEqual([
      { x: 200, y: 50 },
      { x: 512 - LANE, y: 50 },
      { x: 512 - LANE, y: 350 },
      { x: 512, y: 350 },
    ]);
  });

  it('a_card_in_the_way_is_passed_in_the_nearest_row_gap', () => {
    // C sits in column 1 on A's row; B is in column 2. The run drops into the
    // gutter after A, crosses under C (12px below it) and climbs B's lane.
    const a = card(0, 0);
    const c = card(256, 0);
    const b = card(512, 300);
    expect(routeArc(a, b, 0, [a, c, b])).toEqual([
      { x: 200, y: 50 },
      { x: 200 + LANE, y: 50 },
      { x: 200 + LANE, y: 112 },
      { x: 512 - LANE, y: 112 },
      { x: 512 - LANE, y: 350 },
      { x: 512, y: 350 },
    ]);
    // Nothing in the way: the sender's own row.
    expect(routeArc(a, b, 0, [a, b])).toHaveLength(4);
  });

  it('a_column_to_the_left_leaves_from_the_left_side', () => {
    expect(routeArc(card(256, 400), card(0, 0))).toEqual([
      { x: 256, y: 450 },
      { x: 200 + LANE, y: 450 },
      { x: 200 + LANE, y: 50 },
      { x: 200, y: 50 },
    ]);
  });

  it('the_same_column_goes_out_and_back_on_the_right', () => {
    expect(routeArc(card(0, 0), card(0, 200))).toEqual([
      { x: 200, y: 50 },
      { x: 200 + LANE, y: 50 },
      { x: 200 + LANE, y: 250 },
      { x: 200, y: 250 },
    ]);
  });

  it('a_board_item_is_approached_from_its_left_by_a_short_horizontal', () => {
    const r = routeArc(card(0, 0), { x: 600, y: 20 });
    expect(r[r.length - 2]).toEqual({ x: 600 - RAIL_LANE, y: 20 });
    expect(r[r.length - 1]).toEqual({ x: 600, y: 20 });
    // The spread moves the lane only.
    expect(routeArc(card(0, 0), { x: 600, y: 20 }, 2)[1].x).toBe(600 - RAIL_LANE + 8);
  });

  it('spread_is_stable_and_small', () => {
    expect(spreadOf('msg_1')).toBe(spreadOf('msg_1'));
    for (const k of ['a', 'b', 'msg_001', 'msg_999', '']) expect(Math.abs(spreadOf(k))).toBeLessThanOrEqual(2);
  });

  it('stubs_stay_at_their_card', () => {
    const enter = stubRoute(card(100, 0), 'enter');
    // Ends in the card's left port, starts above-left of it, within 50px.
    expect(enter[enter.length - 1]).toEqual({ x: 100, y: 50 });
    expect(enter[0].x).toBeGreaterThanOrEqual(50);
    expect(enter[0].y).toBeLessThan(50);
    // The exit is the same hook run outward.
    expect(stubRoute(card(100, 0), 'exit')).toEqual([...enter].reverse());
    // A dead end leaves the right side and stops close by.
    const dead = stubRoute(card(100, 0), 'dead', 1);
    expect(dead[0]).toEqual({ x: 300, y: 50 });
    expect(dead[dead.length - 1].x).toBeLessThan(340);
    // Slots keep a card's stubs apart.
    expect(stubRoute(card(100, 0), 'dead', 2)[3].y).not.toBe(dead[3].y);
  });
});

describe('polyline', () => {
  const ps = [
    { x: 0, y: 0 },
    { x: 100, y: 0 },
    { x: 100, y: 100 },
  ];
  it('rounds_inner_corners_and_keeps_short_legs_straight', () => {
    expect(roundedPath(ps, 10)).toBe('M 0 0 L 90 0 Q 100 0 100 10 L 100 100');
    // A 6px leg rounds by 3 at most.
    expect(roundedPath([{ x: 0, y: 0 }, { x: 6, y: 0 }, { x: 6, y: 50 }], 10)).toBe('M 0 0 L 3 0 Q 6 0 6 3 L 6 50');
    expect(roundedPath([{ x: 0, y: 0 }, { x: 0, y: 0 }, { x: 5, y: 0 }], 10)).toBe('M 0 0 L 5 0');
  });
  it('points_and_cuts_along_the_length', () => {
    expect(pointAt(ps, 0.5)).toEqual({ x: 100, y: 0 });
    expect(pointAt(ps, 0.75)).toEqual({ x: 100, y: 50 });
    expect(cutAt(ps, 0.25)).toEqual([{ x: 0, y: 0 }, { x: 50, y: 0 }]);
    expect(cutAt(ps, 1)).toEqual(ps);
  });
});
