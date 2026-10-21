import { describe, expect, it } from 'vitest';
import type { Node, NodeChange } from '@xyflow/react';
import { mergeMeasured } from './measured';

const dims = (id: string, width: number, height: number): NodeChange<Node> => ({
  type: 'dimensions',
  id,
  dimensions: { width, height },
});

describe('mergeMeasured', () => {
  it('records measured sizes', () => {
    const next = mergeMeasured(new Map(), [dims('a', 170, 72), dims('b', 210, 90)]);
    expect(Object.fromEntries(next)).toEqual({ a: { width: 170, height: 72 }, b: { width: 210, height: 90 } });
  });

  it('returns the same map when nothing changed, so the graph does not re-render', () => {
    const prev = new Map([['a', { width: 170, height: 72 }]]);
    expect(mergeMeasured(prev, [dims('a', 170, 72)])).toBe(prev);
    expect(mergeMeasured(prev, [{ type: 'select', id: 'a', selected: true }])).toBe(prev);
  });

  it('takes a new size for a node that grew', () => {
    const prev = new Map([['a', { width: 170, height: 72 }]]);
    const next = mergeMeasured(prev, [dims('a', 170, 100)]);
    expect(next).not.toBe(prev);
    expect(next.get('a')).toEqual({ width: 170, height: 100 });
  });
});
