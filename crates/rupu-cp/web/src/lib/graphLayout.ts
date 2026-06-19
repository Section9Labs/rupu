import { Graph } from '@dagrejs/graphlib';
import { layout } from '@dagrejs/dagre';
import type { GraphLabel, NodeLabel } from '@dagrejs/dagre';
import type { RunGraphModel } from './runGraphModel';
import { nodeSize } from './nodeSize';

export interface Pos { x: number; y: number; width: number; height: number }

export function layoutGraph(m: RunGraphModel): Map<string, Pos> {
  const g = new Graph<GraphLabel, NodeLabel, Record<string, never>>();
  g.setGraph({ rankdir: 'LR', nodesep: 36, ranksep: 72 });
  g.setDefaultEdgeLabel(() => ({}));
  // Reserve each node's REAL rendered box so dagre never packs big nodes
  // (parallel / fanout / panel) tightly enough to overlap. See nodeSize.ts.
  const sizes = new Map<string, { width: number; height: number }>();
  for (const n of m.nodes) {
    const size = nodeSize(n);
    sizes.set(n.id, size);
    g.setNode(n.id, { width: size.width, height: size.height });
  }
  for (const e of m.edges) g.setEdge(e.from, e.to);
  layout(g);
  const out = new Map<string, Pos>();
  for (const n of m.nodes) {
    const d = g.node(n.id);
    const { width, height } = sizes.get(n.id)!;
    // dagre centers nodes; React Flow wants top-left:
    out.set(n.id, { x: d.x! - width / 2, y: d.y! - height / 2, width, height });
  }
  return out;
}
