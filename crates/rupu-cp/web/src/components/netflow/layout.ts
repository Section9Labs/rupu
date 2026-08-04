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
}

const PAD_X = 140;
const PAD_Y = 24;
const MIN_W = 1;
const MAX_W = 8;

/** Two columns: sources left, endpoints right. Not a DAG layout —
 *  netflow topology is bipartite by construction.
 *
 *  Edge stroke width is always scaled by call count, never by bytes (Fix 4,
 *  netflow Plan 3 review round 3): a Coarse-fidelity flow's `bytes` sums to
 *  0 (see `GraphEdge.bytes` doc in `lib/netflow.ts`), so weighting by bytes
 *  drew those edges at the same minimum width as a genuinely tiny transfer
 *  — the one place in the UI where "unobservable" rendered as a quantity
 *  instead of an absence. `calls` has no such gap: every edge that exists
 *  had at least one call. */
export function layoutBipartite(graph: GraphView, opts: LayoutOpts): PositionedGraph {
  const { width, rowHeight } = opts;

  const sources = graph.nodes.filter((n) => n.side === 'source');
  const endpoints = graph.nodes.filter((n) => n.side === 'endpoint');

  const place = (list: GraphNode[], x: number): PositionedNode[] =>
    list.map((n, i) => ({ ...n, x, y: PAD_Y + i * rowHeight }));

  const positioned = [
    ...place(sources, PAD_X),
    ...place(endpoints, Math.max(width - PAD_X, PAD_X * 2)),
  ];
  const byId = new Map(positioned.map((n) => [n.id, n]));

  const maxWeight = graph.edges.reduce((m, e) => Math.max(m, e.calls), 0);

  const edges: PositionedEdge[] = graph.edges.flatMap((e) => {
    const a = byId.get(e.from);
    const b = byId.get(e.to);
    if (!a || !b) return [];
    const w = e.calls;
    // maxWeight is 0 when every edge has 0 calls (an empty graph's edge
    // list, or defensively for malformed data) — keep the minimum width
    // rather than dividing by zero / producing NaN.
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
