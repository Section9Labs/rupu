// workflowLayout — pure dagre auto-layout for the visual workflow editor.
//
// Positions graph nodes top-to-bottom from the {nodes, edges} model produced by
// workflowGraph.ts. Framework-free: no DOM, no React, no side effects. Mirrors
// the dagre usage in graphLayout.ts (graphlib Graph + dagre `layout`).

import { Graph } from '@dagrejs/graphlib';
import { layout } from '@dagrejs/dagre';
import type { GraphLabel, NodeLabel } from '@dagrejs/dagre';
import yaml from 'js-yaml';
import { yamlToGraph } from './workflowGraph';
import type { GraphNode, GraphEdge, WorkflowGraph } from './workflowGraph';

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

// ── reconcileGraph ────────────────────────────────────────────────────────────

/** Merge a freshly-parsed YAML graph (`next`, whose node positions are the
 *  placeholder {0,0} from yamlToGraph) onto the on-screen graph (`prev`) WITHOUT
 *  a full relayout:
 *   - surviving ids keep their existing on-screen position; their data/structure
 *     is taken from `next` (so YAML edits flow through);
 *   - new ids get a dagre position (so they land somewhere sensible) without
 *     disturbing survivors;
 *   - removed ids drop out;
 *   - edges + meta come wholesale from `next` (the YAML-derived graph).
 *  Pure: inputs are never mutated. */
export function reconcileGraph(prev: WorkflowGraph, next: WorkflowGraph): WorkflowGraph {
  const laid = autoLayout(next.nodes, next.edges);
  const laidPosById = new Map(laid.map((n) => [n.id, n.position]));
  const prevPosById = new Map(prev.nodes.map((n) => [n.id, n.position]));
  const nodes: GraphNode[] = next.nodes.map((n) => ({
    ...n,
    position: prevPosById.get(n.id) ?? laidPosById.get(n.id) ?? { x: 0, y: 0 },
  }));
  return { meta: next.meta, edges: next.edges, nodes };
}

// ── reconcileFromYaml ─────────────────────────────────────────────────────────

/** Parse `yamlText` and reconcile it against `prevGraph`. On a parse failure
 *  (yaml.load throws, or the document isn't a plain object) returns
 *  `{ paused: true }` and NO graph — callers keep the last good graph on screen.
 *  On success returns `{ graph, paused: false }`. Pure; no DOM. */
export function reconcileFromYaml(
  prevGraph: WorkflowGraph,
  yamlText: string,
): { graph?: WorkflowGraph; paused: boolean } {
  let loaded: unknown;
  try {
    loaded = yaml.load(yamlText);
  } catch {
    return { paused: true };
  }
  if (typeof loaded !== 'object' || loaded === null || Array.isArray(loaded)) {
    return { paused: true };
  }
  const next = yamlToGraph(loaded as Record<string, unknown>);
  return { graph: reconcileGraph(prevGraph, next), paused: false };
}
