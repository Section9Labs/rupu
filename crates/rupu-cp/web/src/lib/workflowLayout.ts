// workflowLayout — pure dagre auto-layout for the visual workflow editor.
//
// Positions graph nodes top-to-bottom from the {nodes, edges} model produced by
// workflowGraph.ts. Framework-free: no DOM, no React, no side effects. Mirrors
// the dagre usage in graphLayout.ts (graphlib Graph + dagre `layout`).

import { Graph } from '@dagrejs/graphlib';
import { layout } from '@dagrejs/dagre';
import type { GraphLabel, NodeLabel } from '@dagrejs/dagre';
import type { GraphNode, GraphEdge } from './workflowGraph';

// Node box reserved in the layout — exported so the canvas renderer can match.
export const NODE_W = 220;
export const NODE_H = 80;

/** Position workflow nodes top-to-bottom. Returns a NEW array; inputs are never
 *  mutated. dagre centers nodes; we convert to the top-left corner the canvas
 *  expects. */
export function autoLayout(nodes: GraphNode[], edges: GraphEdge[]): GraphNode[] {
  if (nodes.length === 0) return [];

  const g = new Graph<GraphLabel, NodeLabel, Record<string, never>>();
  g.setGraph({ rankdir: 'TB', nodesep: 40, ranksep: 70 });
  g.setDefaultEdgeLabel(() => ({}));

  for (const n of nodes) g.setNode(n.id, { width: NODE_W, height: NODE_H });
  for (const e of edges) g.setEdge(e.source, e.target);

  layout(g);

  return nodes.map((n) => {
    const d = g.node(n.id);
    return { ...n, position: { x: d.x! - NODE_W / 2, y: d.y! - NODE_H / 2 } };
  });
}
