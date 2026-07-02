import { it, expect } from 'vitest';
import { layoutGraph, type Pos } from './graphLayout';
import type { GraphNode, RunGraphModel } from './runGraphModel';

const m = {
  nodes: [
    { id: 'a', kind: 'step', state: 'done' },
    { id: 'b', kind: 'step', state: 'pending' },
  ],
  edges: [{ from: 'a', to: 'b' }],
  nodeById: () => undefined,
} as unknown as RunGraphModel;

it('positions a linear chain left to right, deterministically', () => {
  const p1 = layoutGraph(m);
  const p2 = layoutGraph(m);
  expect(p1.get('b')!.x).toBeGreaterThan(p1.get('a')!.x);     // LR: b right of a
  expect(p1.get('a')!.y).toBeCloseTo(p1.get('b')!.y, 0);       // same rank row
  expect(p1.get('a')).toEqual(p2.get('a'));                    // deterministic
  expect(p1.get('b')).toEqual(p2.get('b'));
});

it('returns a position for every node', () => {
  expect(layoutGraph(m).size).toBe(2);
});

it('carries each node’s own width/height in the returned Pos', () => {
  const p = layoutGraph(m);
  const a = p.get('a')!;
  expect(a.width).toBeGreaterThan(0);
  expect(a.height).toBeGreaterThan(0);
});

// AABB no-overlap predicate: true when the two boxes do NOT overlap.
function disjoint(a: Pos, b: Pos): boolean {
  return (
    a.x + a.width <= b.x ||
    b.x + b.width <= a.x ||
    a.y + a.height <= b.y ||
    b.y + b.height <= a.y
  );
}

it('lays out mixed-size nodes with no pairwise overlap', () => {
  const units = Array.from({ length: 50 }, (_, i) => ({
    index: i,
    key: `u${i}`,
    state: 'done' as const,
  }));
  const nodes: GraphNode[] = [
    { id: 'plain', kind: 'step', state: 'done' },
    {
      id: 'par',
      kind: 'parallel',
      state: 'running',
      parallel: [
        { id: 's1', state: 'done' },
        { id: 's2', state: 'running' },
        { id: 's3', state: 'pending' },
        { id: 's4', state: 'pending' },
      ],
    },
    {
      id: 'fan',
      kind: 'for_each',
      state: 'running',
      fanout: {
        total: 50,
        byState: {
          pending: 0,
          running: 0,
          awaiting_approval: 0,
          paused: 0,
          done: 50,
          failed: 0,
          skipped: 0,
        },
        units,
      },
    },
    { id: 'pan', kind: 'panel', state: 'pending' },
  ];
  const model = {
    nodes,
    edges: [
      { from: 'plain', to: 'par' },
      { from: 'par', to: 'fan' },
      { from: 'fan', to: 'pan' },
    ],
    nodeById: (id: string) => nodes.find((n) => n.id === id),
  } as unknown as RunGraphModel;

  const pos = layoutGraph(model);
  expect(pos.size).toBe(nodes.length);

  const ids = nodes.map((n) => n.id);
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      const a = pos.get(ids[i])!;
      const b = pos.get(ids[j])!;
      expect(
        disjoint(a, b),
        `nodes ${ids[i]} and ${ids[j]} overlap: ${JSON.stringify(a)} vs ${JSON.stringify(b)}`,
      ).toBe(true);
    }
  }
});

// ---------------------------------------------------------------------------
// Discriminating branched-graph no-overlap test.
//
// A linear chain always places every node in its own dagre rank (column in LR
// layout) so the AABB no-overlap assertion passes trivially — even the old
// buggy fixed-150×64 sizing would have passed it.
//
// This test uses a DIAMOND topology:
//
//   start ──► big_parallel ──► end
//         └─► big_fanout   ──►
//
// `big_parallel` (5 sub-steps) and `big_fanout` (50 units, large-card path)
// land in the SAME dagre rank, i.e. they are vertical siblings.  Their
// reserved heights are:
//   big_parallel : PARALLEL_HEADER_H(24) + 5×PARALLEL_SUBROW_H(22) + PARALLEL_PAD_V(16) = 150 px
//   big_fanout   : FANOUT_CARD_H = 210 px
// Under the old fixed 64 px height the two 64-px boxes with nodesep=36 needed
// only 100 px of centre-to-centre separation — but the rendered boxes were
// 150 and 210 px, producing a ~(150+210)/2 − 50 = 130 px overlap.
// With correct sizing dagre reserves the real heights and keeps them apart.
//
// The test therefore FAILS with the old sizing and PASSES with the fix — it is
// genuinely discriminating.
// ---------------------------------------------------------------------------
it('branched graph: same-rank tall siblings do not overlap in y (discriminating)', () => {
  const fanoutUnits = Array.from({ length: 50 }, (_, i) => ({
    index: i,
    key: `item${i}`,
    state: 'done' as const,
  }));

  const nodes: GraphNode[] = [
    { id: 'start', kind: 'step', state: 'done' },
    {
      // 5 sub-steps → height ≈ 150 px (PARALLEL_HEADER_H + 5×PARALLEL_SUBROW_H + PARALLEL_PAD_V)
      id: 'big_parallel',
      kind: 'parallel',
      state: 'running',
      parallel: [
        { id: 'ps1', state: 'done' },
        { id: 'ps2', state: 'done' },
        { id: 'ps3', state: 'running' },
        { id: 'ps4', state: 'pending' },
        { id: 'ps5', state: 'pending' },
      ],
    },
    {
      // total=50 > FANOUT_INLINE_THRESHOLD → large-card path → height = FANOUT_CARD_H (210 px)
      id: 'big_fanout',
      kind: 'for_each',
      state: 'running',
      fanout: {
        total: 50,
        byState: {
          pending: 0,
          running: 2,
          awaiting_approval: 0,
          paused: 0,
          done: 48,
          failed: 0,
          skipped: 0,
        },
        units: fanoutUnits,
      },
    },
    { id: 'end', kind: 'step', state: 'pending' },
  ];

  // Diamond: start fans to both tall siblings, both converge to end.
  const model = {
    nodes,
    edges: [
      { from: 'start', to: 'big_parallel' },
      { from: 'start', to: 'big_fanout' },
      { from: 'big_parallel', to: 'end' },
      { from: 'big_fanout', to: 'end' },
    ],
    nodeById: (id: string) => nodes.find((n) => n.id === id),
  } as unknown as RunGraphModel;

  const pos = layoutGraph(model);
  expect(pos.size).toBe(nodes.length);

  // Primary assertion: no pair of nodes overlaps (AABB).
  const ids = nodes.map((n) => n.id);
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      const a = pos.get(ids[i])!;
      const b = pos.get(ids[j])!;
      expect(
        disjoint(a, b),
        `nodes ${ids[i]} and ${ids[j]} overlap: ${JSON.stringify(a)} vs ${JSON.stringify(b)}`,
      ).toBe(true);
    }
  }

  // Sub-assertion: confirm big_parallel and big_fanout are vertical siblings
  // (their x-ranges overlap) yet their y-bands are disjoint — proving that
  // dagre used their real heights to separate them rather than a trivially
  // small fixed size.
  const bp = pos.get('big_parallel')!;
  const bf = pos.get('big_fanout')!;

  // They share a rank: one node's x-interval must overlap the other's.
  const xOverlap = bp.x < bf.x + bf.width && bf.x < bp.x + bp.width;
  expect(xOverlap).toBe(true); // same-rank vertical siblings

  // Their y-bands must be disjoint (the core regression guard).
  const yDisjoint = bp.y + bp.height <= bf.y || bf.y + bf.height <= bp.y;
  expect(
    yDisjoint,
    `big_parallel and big_fanout overlap vertically: ` +
      `big_parallel y=[${bp.y}, ${bp.y + bp.height}), big_fanout y=[${bf.y}, ${bf.y + bf.height})`,
  ).toBe(true);
});
