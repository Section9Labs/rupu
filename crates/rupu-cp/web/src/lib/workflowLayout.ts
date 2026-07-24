// workflowLayout — pure dagre auto-layout for the visual workflow editor.
//
// Positions graph nodes left-to-right from the {nodes, edges} model produced by
// workflowGraph.ts — mirroring the Runs graph (graphLayout.ts: rankdir 'LR',
// nodesep 36, ranksep 72). Framework-free: no DOM, no React, no side effects.
//
// Each node reserves a per-KIND box (editorNodeSize) so big container nodes
// (parallel / panel) never get packed tightly enough to overlap. The editor
// node components consume the SAME constants/function (applied as
// `style={{ width, minHeight }}`) so render == reservation by construction —
// the same discipline nodeSize.ts brings to the read-only Runs graph.

import { Graph } from '@dagrejs/graphlib';
import { layout } from '@dagrejs/dagre';
import type { GraphLabel, NodeLabel } from '@dagrejs/dagre';
import yaml from 'js-yaml';
import { yamlToGraph } from './workflowGraph';
import type { GraphNode, GraphEdge, StepNodeData, WorkflowGraph } from './workflowGraph';

// ── Per-kind size constants (shared with EditableStepNode) ───────────────────
// Exported so the node components apply the identical width/minHeight, keeping
// the rendered box ≥ dagre's reserved box (no overlap). NODE_W/NODE_H are kept
// as the base step box for backwards-compat with existing importers.

/** Base step card box (also the back-compat NODE_W/NODE_H export). */
export const NODE_W = 210;
export const NODE_H = 80;

/** for_each carries an extra `for_each: <expr>` line. */
export const FOR_EACH_H = 100;

/** branch paints a diamond — a diamond's usable width collapses toward its
 *  tips, so its safe rect can only ever use a fraction of the box (here,
 *  half the width, a band centred at 28%-72% of the height — see the diamond
 *  case in nodeShapes.ts). Widening is cheaper than heightening for a
 *  diamond's safe area (the safe rect's usable half-width grows linearly with
 *  BOTH the box width and the y-fraction, but a taller box also pushes ranks
 *  apart more under dagre's `rankdir: 'LR'`, where node HEIGHT is the
 *  cross-axis extent) — so BRANCH_W is wider than a step's 210 (despite a
 *  branch carrying more content: header + condition + two then/else port
 *  pills), while BRANCH_H stays modest. At 280x200 the safe rect is a
 *  140x88 band — comfortable room for a realistic branch body (measured in
 *  headless Chrome; see the diamond case in nodeShapes.ts). */
export const BRANCH_W = 280;
export const BRANCH_H = 200;

/** action (parallelogram) and approval_gate (trapezoid) both lose horizontal
 *  room to slanted sides; the box grows so the text band stays step-sized. */
export const ACTION_W = 214;
export const GATE_W = 214;

/** for_each (hexagon) loses room to its left/right points. Height unchanged. */
export const FOR_EACH_W = 214;

/** parallel container: header + N stacked sub-step rows. */
export const PARALLEL_W = 220;
export const PARALLEL_HEADER_H = 54;
export const PARALLEL_SUBROW_H = 26;
export const PARALLEL_PAD_V = 18;

/** panel container: header + panelists row + optional gate block. */
export const PANEL_W = 220;
export const PANEL_BASE_H = 84;
export const PANEL_GATE_H = 34;

export interface NodeBox {
  width: number;
  height: number;
}

/** Per-kind box for an editor node — used by dagre AND applied to the rendered
 *  root (`style={{ width, minHeight: height }}`). Mirrors the spirit of
 *  lib/nodeSize.ts for the editor's run-state-free nodes. */
export function editorNodeSize(d: StepNodeData): NodeBox {
  switch (d.kind) {
    case 'parallel': {
      const rows = Math.max(d.parallel?.length ?? 0, 1);
      return { width: PARALLEL_W, height: PARALLEL_HEADER_H + rows * PARALLEL_SUBROW_H + PARALLEL_PAD_V };
    }
    case 'panel':
      return { width: PANEL_W, height: PANEL_BASE_H + (d.panel?.gate ? PANEL_GATE_H : 0) };
    case 'branch':
      return { width: BRANCH_W, height: BRANCH_H };
    case 'action':
      return { width: ACTION_W, height: NODE_H };
    case 'approval_gate':
      return { width: GATE_W, height: NODE_H };
    case 'for_each':
      return { width: FOR_EACH_W, height: FOR_EACH_H };
    default:
      return { width: NODE_W, height: NODE_H };
  }
}

/** Position workflow nodes left-to-right. Returns a NEW array; inputs are never
 *  mutated. dagre centers nodes; we convert to the top-left corner the canvas
 *  expects. */
export function autoLayout(nodes: GraphNode[], edges: GraphEdge[]): GraphNode[] {
  if (nodes.length === 0) return [];

  const g = new Graph<GraphLabel, NodeLabel, Record<string, never>>();
  g.setGraph({ rankdir: 'LR', nodesep: 36, ranksep: 72 });
  g.setDefaultEdgeLabel(() => ({}));

  const sizes = new Map<string, NodeBox>();
  for (const n of nodes) {
    const size = editorNodeSize(n.data);
    sizes.set(n.id, size);
    g.setNode(n.id, { width: size.width, height: size.height });
  }
  for (const e of edges) g.setEdge(e.source, e.target);

  layout(g);

  return nodes.map((n) => {
    const d = g.node(n.id);
    const { width, height } = sizes.get(n.id)!;
    return { ...n, position: { x: d.x! - width / 2, y: d.y! - height / 2 } };
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
