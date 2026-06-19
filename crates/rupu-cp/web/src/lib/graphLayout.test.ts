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
