import { describe, expect, it } from 'vitest';
import { layoutBipartite } from './layout';
import type { GraphView } from '../../lib/netflow';

const graph: GraphView = {
  nodes: [
    { id: 'run-1', label: 'run-1', side: 'source' },
    { id: 'run-2', label: 'run-2', side: 'source' },
    { id: 'api.anthropic.com:443', label: 'api.anthropic.com', side: 'endpoint' },
  ],
  edges: [
    { from: 'run-1', to: 'api.anthropic.com:443', calls: 5, bytes: 500, errors: 0 },
    { from: 'run-2', to: 'api.anthropic.com:443', calls: 1, bytes: 100, errors: 1 },
  ],
};

describe('layoutBipartite', () => {
  it('puts sources in a left column and endpoints in a right column', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    const sources = out.nodes.filter((n) => n.side === 'source');
    const endpoints = out.nodes.filter((n) => n.side === 'endpoint');
    expect(new Set(sources.map((n) => n.x)).size).toBe(1);
    expect(new Set(endpoints.map((n) => n.x)).size).toBe(1);
    expect(sources[0].x).toBeLessThan(endpoints[0].x);
  });

  it('gives each node in a column a distinct y', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    const ys = out.nodes.filter((n) => n.side === 'source').map((n) => n.y);
    expect(new Set(ys).size).toBe(ys.length);
  });

  it('scales edge weight by call count', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    const heavy = out.edges.find((e) => e.from === 'run-1')!;
    const light = out.edges.find((e) => e.from === 'run-2')!;
    expect(heavy.width).toBeGreaterThan(light.width);
  });

  it('marks an edge with errors so it can be coloured', () => {
    const out = layoutBipartite(graph, { width: 800, rowHeight: 40 });
    expect(out.edges.find((e) => e.from === 'run-2')!.hasErrors).toBe(true);
    expect(out.edges.find((e) => e.from === 'run-1')!.hasErrors).toBe(false);
  });

  it('handles an empty graph without dividing by zero', () => {
    const out = layoutBipartite({ nodes: [], edges: [] }, { width: 800, rowHeight: 40 });
    expect(out.nodes).toEqual([]);
    expect(out.height).toBeGreaterThan(0);
  });

  it('falls back to the minimum stroke width when every edge weighs zero', () => {
    // Defensive: `calls` is normally >= 1 for any edge that exists at all,
    // but maxWeight must still not become a divisor if it somehow isn't.
    const zeroGraph: GraphView = {
      nodes: [
        { id: 'run-1', label: 'run-1', side: 'source' },
        { id: 'host:443', label: 'host', side: 'endpoint' },
      ],
      edges: [{ from: 'run-1', to: 'host:443', calls: 0, bytes: 0, errors: 0 }],
    };
    const out = layoutBipartite(zeroGraph, { width: 800, rowHeight: 40 });
    expect(out.edges[0].width).toBe(1);
    expect(Number.isFinite(out.edges[0].width)).toBe(true);
  });
});
