import { it, expect } from 'vitest';
import { layoutGraph } from './graphLayout';
import type { RunGraphModel } from './runGraphModel';

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
