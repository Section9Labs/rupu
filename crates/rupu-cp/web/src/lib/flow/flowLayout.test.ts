import { describe, expect, it } from 'vitest';
import type { Size } from '../graph/measured';
import type { Pos } from '../graphLayout';
import { GANNET_GRAPH, RUN, bigFlow, fixtureInput } from './fixtures';
import { buildFlowModel } from './flowModel';
import { CARD_H, CARD_W, COL_GAP, RAIL_GAP, RAIL_W, layoutFlow } from './flowLayout';

const overlap = (a: Pos, b: Pos) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

/** A frame's inner steps sit inside the frame by design; everything else is a card. */
function overlaps(positions: Map<string, Pos>): string[][] {
  const entries = [...positions];
  const bad: string[][] = [];
  for (let i = 0; i < entries.length; i++) {
    for (let j = i + 1; j < entries.length; j++) {
      const [a, pa] = entries[i];
      const [b, pb] = entries[j];
      const nested = a.startsWith(`${b}/`) || b.startsWith(`${a}/`);
      if (!nested && overlap(pa, pb)) bad.push([a, b]);
    }
  }
  return bad;
}

describe('layoutFlow', () => {
  // Round 0 is collapsed by default; open it so every fixture child is a card.
  const model = buildFlowModel(fixtureInput(), { expanded: new Set(['round:0']) });
  const lay = layoutFlow(model);
  const at = (id: string) => lay.positions.get(id)!;

  it('rounds_stack_down_in_order', () => {
    const ys = model.rounds.map((r) => at(`round:${r.round}`).y);
    expect(ys).toEqual([...ys].sort((a, b) => a - b));
    expect(new Set(ys).size).toBe(ys.length);
    for (const r of model.rounds) expect(at(`round:${r.round}`).x).toBe(0);
    expect(at('lead').x).toBe(0);
    expect(at('lead').y).toBeLessThan(ys[0]);
    expect(lay.operator.x).toBeLessThan(0);
  });

  it('depth_moves_right', () => {
    const byId = new Map(model.nodes.map((n) => [n.id, n]));
    for (const n of model.nodes) {
      if (n.kind !== 'child' && n.kind !== 'frame') continue;
      const parent = at(n.parentId);
      expect(at(n.id).x).toBeGreaterThan(parent.x + parent.width - 1);
      expect(byId.get(n.parentId)).toBeDefined();
    }
    expect(at(RUN.lynx1).x).toBe(CARD_W + COL_GAP);
  });

  it('frame_contains_its_inner_graph', () => {
    const frame = lay.frames.get(RUN.gannet1)!;
    expect(frame).toBeDefined();
    expect(frame.width).toBeGreaterThan(CARD_W * 2);
    for (const n of GANNET_GRAPH.nodes) {
      const p = at(`${RUN.gannet1}/${n.id}`);
      expect(p.x).toBeGreaterThanOrEqual(frame.x);
      expect(p.y).toBeGreaterThanOrEqual(frame.y);
      expect(p.x + p.width).toBeLessThanOrEqual(frame.x + frame.width);
      expect(p.y + p.height).toBeLessThanOrEqual(frame.y + frame.height);
    }
    // Unloaded: a placeholder frame, no inner positions.
    const input = fixtureInput();
    input.childGraphs = new Map();
    const bare = layoutFlow(buildFlowModel(input));
    expect(bare.frames.get(RUN.gannet1)!.width).toBeGreaterThan(0);
    expect([...bare.positions.keys()].some((k) => k.startsWith(`${RUN.gannet1}/`))).toBe(false);
  });

  it('measured_sizes_replace_defaults', () => {
    const l = layoutFlow(model, new Map([[RUN.lynx1, { width: 300, height: 90 }]]));
    expect(l.positions.get(RUN.lynx1)).toMatchObject({ width: 300, height: 90 });
  });

  it('a_collapsed_round_gets_one_summary_card', () => {
    const c = buildFlowModel(fixtureInput());
    const l = layoutFlow(c);
    expect(c.rounds[0].collapsed).toBe(true);
    expect(l.positions.get('summary:0')!.x).toBe(CARD_W + COL_GAP);
    expect(l.positions.has(RUN.lynx1)).toBe(false);
  });

  it('measured_sizes_larger_than_defaults_keep_cards_apart_and_inner_steps_inside', () => {
    const sizes = new Map<string, Size>([
      [RUN.lynx1, { width: 400, height: 220 }],
      [RUN.gannet1, { width: 1200, height: 700 }],
    ]);
    for (const n of GANNET_GRAPH.nodes) sizes.set(`${RUN.gannet1}/${n.id}`, { width: 500, height: 260 });
    const l = layoutFlow(model, sizes);
    expect(overlaps(l.positions)).toEqual([]);
    const frame = l.frames.get(RUN.gannet1)!;
    // The measured frame is a floor; the larger inner extent would grow it.
    expect(frame.width).toBeGreaterThanOrEqual(1200);
    expect(frame.height).toBeGreaterThanOrEqual(700);
    for (const n of GANNET_GRAPH.nodes) {
      const p = l.positions.get(`${RUN.gannet1}/${n.id}`)!;
      expect(p).toMatchObject({ width: 500, height: 260 });
      expect(p.x).toBeGreaterThanOrEqual(frame.x);
      expect(p.y).toBeGreaterThanOrEqual(frame.y);
      expect(p.x + p.width).toBeLessThanOrEqual(frame.x + frame.width);
      expect(p.y + p.height).toBeLessThanOrEqual(frame.y + frame.height);
    }
    // A small measured frame never clips big inner steps: it grows.
    sizes.set(RUN.gannet1, { width: 100, height: 100 });
    const g = layoutFlow(model, sizes);
    const f2 = g.frames.get(RUN.gannet1)!;
    for (const n of GANNET_GRAPH.nodes) {
      const p = g.positions.get(`${RUN.gannet1}/${n.id}`)!;
      expect(p.x + p.width).toBeLessThanOrEqual(f2.x + f2.width);
      expect(p.y + p.height).toBeLessThanOrEqual(f2.y + f2.height);
    }
    expect(overlaps(g.positions)).toEqual([]);
  });

  it('no_two_cards_overlap', () => {
    expect(overlaps(lay.positions)).toEqual([]);
    const big = buildFlowModel(bigFlow());
    const bl = layoutFlow(big);
    expect(overlaps(bl.positions)).toEqual([]);
  });

  it('layout_is_bounded_and_grows_down_and_right', () => {
    const big = buildFlowModel(bigFlow());
    expect(big.rounds).toHaveLength(200);
    const bl = layoutFlow(big);
    const collapsed = big.rounds.filter((r) => r.collapsed).length;
    expect(collapsed).toBeGreaterThan(150);
    // Linear in rounds: collapsed rounds cost one card row each.
    expect(bl.height).toBeLessThan(200 * (CARD_H + 56) + 30 * (CARD_H + 24) + CARD_H + 56);
    expect(bl.height).toBeGreaterThan(lay.height);
    const widest = Math.max(...[...bl.positions.values()].map((p) => p.x + p.width));
    expect(bl.railX).toBe(widest + RAIL_GAP);
    expect(bl.width).toBe(bl.railX + RAIL_W);
    expect(bl.width).toBeLessThan(4 * (CARD_W + 56) + RAIL_GAP + RAIL_W);
    // The fixture's frame widens the layout past the deepest plain column.
    expect(lay.railX).toBeGreaterThan(at(RUN.gannet1).x + at(RUN.gannet1).width);
  });
});
