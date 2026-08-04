// Pure bipartite layout for the netflow topology graph. Deliberately NOT the
// DAG layout engine in `components/graph` (dagre-based, single-column-flow
// topology) — a netflow graph is bipartite by construction (sources always
// point at endpoints, never at each other), so a two-column placement is the
// correct shape, not an approximation of the DAG one.
//
// Kept in its own file, no React/DOM import, so it's testable without
// rendering (`layout.test.ts`).

import type { GraphEdge, GraphNode, GraphView } from '../../lib/netflow';

export interface PositionedNode extends GraphNode {
  x: number;
  y: number;
}

export interface PositionedEdge extends GraphEdge {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  /** Stroke width, MIN_W–MAX_W, scaled against the heaviest edge in the
   *  graph by the chosen metric. NOT a byte/call count in its own right —
   *  purely a rendering weight. */
  width: number;
  hasErrors: boolean;
}

export interface PositionedGraph {
  nodes: PositionedNode[];
  edges: PositionedEdge[];
  height: number;
}

export interface LayoutOpts {
  width: number;
  rowHeight: number;
  /** Which `GraphEdge` field drives stroke width. Defaults to `calls`
   *  because `bytes` is 0 for Coarse-fidelity flows (see `GraphEdge.bytes`
   *  doc in `lib/netflow.ts`) — `calls` stays meaningful even when a scope
   *  is entirely Coarse. */
  weightBy?: 'calls' | 'bytes';
}

const PAD_X = 140;
const PAD_Y = 24;
const MIN_W = 1;
const MAX_W = 8;

/** Two columns: sources left, endpoints right. Not a DAG layout —
 *  netflow topology is bipartite by construction. */
export function layoutBipartite(graph: GraphView, opts: LayoutOpts): PositionedGraph {
  const { width, rowHeight, weightBy = 'calls' } = opts;

  const sources = graph.nodes.filter((n) => n.side === 'source');
  const endpoints = graph.nodes.filter((n) => n.side === 'endpoint');

  const place = (list: GraphNode[], x: number): PositionedNode[] =>
    list.map((n, i) => ({ ...n, x, y: PAD_Y + i * rowHeight }));

  const positioned = [
    ...place(sources, PAD_X),
    ...place(endpoints, Math.max(width - PAD_X, PAD_X * 2)),
  ];
  const byId = new Map(positioned.map((n) => [n.id, n]));

  const maxWeight = graph.edges.reduce(
    (m, e) => Math.max(m, weightBy === 'bytes' ? e.bytes : e.calls),
    0,
  );

  const edges: PositionedEdge[] = graph.edges.flatMap((e) => {
    const a = byId.get(e.from);
    const b = byId.get(e.to);
    if (!a || !b) return [];
    const w = weightBy === 'bytes' ? e.bytes : e.calls;
    // maxWeight is 0 when every edge is 0 (a real case: an all-Coarse scope
    // has bytes=0 on every edge, see GraphEdge.bytes) — keep the minimum
    // width rather than dividing by zero / producing NaN.
    const scaled = maxWeight > 0 ? MIN_W + (w / maxWeight) * (MAX_W - MIN_W) : MIN_W;
    return [
      {
        ...e,
        x1: a.x,
        y1: a.y,
        x2: b.x,
        y2: b.y,
        width: scaled,
        hasErrors: e.errors > 0,
      },
    ];
  });

  const rows = Math.max(sources.length, endpoints.length, 1);
  return { nodes: positioned, edges, height: PAD_Y * 2 + rows * rowHeight };
}
