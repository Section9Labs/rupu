import { Graph } from '@dagrejs/graphlib';
import { layout } from '@dagrejs/dagre';
import type { GraphLabel, NodeLabel } from '@dagrejs/dagre';
import type { RunGraphModel } from './runGraphModel';

export interface Pos { x: number; y: number; width: number; height: number }

const NODE_W = 150;
const NODE_H = 64;

export function layoutGraph(m: RunGraphModel): Map<string, Pos> {
  const g = new Graph<GraphLabel, NodeLabel, Record<string, never>>();
  g.setGraph({ rankdir: 'LR', nodesep: 28, ranksep: 60 });
  g.setDefaultEdgeLabel(() => ({}));
  for (const n of m.nodes) g.setNode(n.id, { width: NODE_W, height: NODE_H });
  for (const e of m.edges) g.setEdge(e.from, e.to);
  layout(g);
  const out = new Map<string, Pos>();
  for (const n of m.nodes) {
    const d = g.node(n.id);
    // dagre centers nodes; React Flow wants top-left:
    out.set(n.id, { x: d.x! - NODE_W / 2, y: d.y! - NODE_H / 2, width: NODE_W, height: NODE_H });
  }
  return out;
}
