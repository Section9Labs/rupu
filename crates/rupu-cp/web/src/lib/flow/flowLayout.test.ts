import { describe, expect, it } from 'vitest';
import type { AgentiflowEvent, RunChild, SpaceParticipant } from '../api';
import type { Pos } from '../graphLayout';
import { GANNET_GRAPH, DETAIL, FLOW_ID, RUN, fixtureInput } from './fixtures';
import { buildFlowModel, type FlowInput } from './flowModel';
import { CARD_H, CARD_W, COL_GAP, RAIL_GAP, RAIL_W, layoutFlow } from './flowLayout';

const overlap = (a: Pos, b: Pos) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

/** A frame's inner steps sit inside the frame by design; everything else is a card. */
function overlaps(positions: Map<string, Pos>, frames: Map<string, Pos>): string[][] {
  const entries = [...positions].filter(([id]) => !frames.has(id.split('/')[0]) || id.includes('/') || frames.has(id));
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

function bigFlow(): FlowInput {
  const MIN = 60_000;
  const t0 = Date.UTC(2026, 9, 1);
  const iso = (t: number) => new Date(t).toISOString();
  const events: AgentiflowEvent[] = [{ kind: 'run_started', ts: iso(t0) }];
  for (let r = 0; r < 199; r++) {
    events.push({
      kind: 'round', round: r, started_at: iso(t0 + r * 10 * MIN), ts: iso(t0 + (r * 10 + 9) * MIN),
      goals_met: 0, goals_total: 1, spent_usd: r, spent_tokens: r * 1000, outcome: 'yielded',
    });
  }
  const children: RunChild[] = [];
  const participants: SpaceParticipant[] = [];
  const add = (run_id: string, round: number, start: number, status: RunChild['status']) => {
    const address = `${run_id}#1`;
    children.push({
      handle: address, kind: 'agent', name: 'worker', run_id, parent_run_id: FLOW_ID, depth: 1, codename: null,
      address, status, success: null, error: null, tokens: null, duration_ms: null, started_at: iso(start),
      finished_at: null, transcript_path: null, legacy: false, round,
    });
  };
  for (let r = 0; r < 200; r++) {
    if (r === 120) {
      for (let i = 0; i < 23; i++) {
        add(`run_u120_${i}`, r, t0 + (r * 10 + 1) * MIN + i * 1000, i === 9 ? 'failed' : i === 15 ? 'running' : 'done');
      }
    } else add(`run_r${r}`, r, t0 + (r * 10 + 1) * MIN, r === 199 ? 'running' : 'done');
  }
  return {
    detail: { ...DETAIL, record: { ...DETAIL.record, started_at: iso(t0) }, events, lead_transcripts: [] },
    children,
    spaces: [{ id: 'flow', participants, messages: [] }],
    boards: new Map(),
    childGraphs: new Map(),
  };
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

  it('no_two_cards_overlap', () => {
    expect(overlaps(lay.positions, lay.frames)).toEqual([]);
    const big = buildFlowModel(bigFlow());
    const bl = layoutFlow(big);
    expect(overlaps(bl.positions, bl.frames)).toEqual([]);
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
