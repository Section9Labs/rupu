// The unfolding map's layout (spec `2026-10-21-rupu-agentiflow-flow-design.md`).
// Pure; no React. The lead's spine is at x 0 and round cards sit in column 0
// stacked downward in order; a round's children branch right, one column per
// depth, each subtree as tall as its tallest branch; a workflow child is a
// frame wide enough for its inner run graph (`layoutGraph`), and what it
// launched continues to its right. The board rail sits right of the widest
// block. Card sizes come from `sizes` (measured, as `RunGraph` does with
// `mergeMeasured`) else from the run graph's own card constants.

import { layoutGraph, type Pos } from '../graphLayout';
import { STEP_AGENT_H, STEP_W } from '../nodeSize';
import type { Size } from '../graph/measured';
import type { FlowNodeRef, FlowStructure } from './flowModel';

export const CARD_W = STEP_W;
export const CARD_H = STEP_AGENT_H;
/** Horizontal gap between columns, vertical gap between siblings. */
export const COL_GAP = 56;
export const ROW_GAP = 24;
/** Vertical gap between one round's block and the next. */
export const ROUND_GAP = 56;
/** A frame's padding around its inner graph, and its header strip. */
export const FRAME_PAD = 24;
export const FRAME_HEAD = 40;
/** A frame whose graph has not loaded. */
export const FRAME_PLACEHOLDER: Size = { width: 480, height: 220 };
/** The board rail's width and its gap from the widest block. */
export const RAIL_W = 260;
export const RAIL_GAP = 96;
/** How far left of the spine the map reaches (the operator's entry stubs
 *  and their labels at the round cards). */
export const OPERATOR_GAP = 96;

export interface FlowLayout {
  /** Top-left boxes by node id: `lead`, `round:N`, `summary:N` (a collapsed
   *  round's one summary card), `more:N`, children, frames and, for a loaded
   *  frame, each inner step as `<frameId>/<stepId>`. */
  positions: Map<string, Pos>;
  /** Each frame's box. */
  frames: Map<string, Pos>;
  /** The board rail's left edge. */
  railX: number;
  /** The map's left reach: where the operator's stubs and labels end. */
  operator: { x: number; y: number };
  width: number;
  height: number;
}

interface Frame {
  size: Size;
  inner: { id: string; pos: Pos }[];
}

export function layoutFlow(model: Pick<FlowStructure, 'nodes' | 'rounds'>, sizes: ReadonlyMap<string, Size> = new Map()): FlowLayout {
  const positions = new Map<string, Pos>();
  const frames = new Map<string, Pos>();
  const sizeOf = (id: string): Size => sizes.get(id) ?? { width: CARD_W, height: CARD_H };

  const kids = new Map<string, FlowNodeRef[]>();
  for (const n of model.nodes) {
    if (n.kind === 'lead' || n.kind === 'round') continue;
    const list = kids.get(n.parentId);
    if (list) list.push(n);
    else kids.set(n.parentId, [n]);
  }

  // A frame's box: its inner graph laid out with the measured inner sizes
  // (`<frameId>/<stepId>` keys), grown to contain it; a measured frame size
  // is a floor, never a clip.
  const frameOf = (n: FlowNodeRef): Frame => {
    const measured = sizes.get(n.id);
    if (!n.graph) return { size: measured ?? FRAME_PLACEHOLDER, inner: [] };
    const inner = new Map<string, Size>();
    for (const node of n.graph.nodes) {
      const s = sizes.get(`${n.id}/${node.id}`);
      if (s) inner.set(node.id, s);
    }
    const laid = layoutGraph(n.graph, inner);
    let w = 0;
    let h = 0;
    for (const p of laid.values()) {
      w = Math.max(w, p.x + p.width);
      h = Math.max(h, p.y + p.height);
    }
    const size = {
      width: Math.max(measured?.width ?? 0, w + 2 * FRAME_PAD),
      height: Math.max(measured?.height ?? 0, h + 2 * FRAME_PAD + FRAME_HEAD),
    };
    return { size, inner: [...laid].map(([id, pos]) => ({ id, pos })) };
  };

  let right = 0;
  const place = (id: string, x: number, y: number, s: Size): Pos => {
    const p = { x, y, width: s.width, height: s.height };
    positions.set(id, p);
    right = Math.max(right, x + s.width);
    return p;
  };

  /** Lays `n` and what hangs off it with its top at `y`; returns the height. */
  const subtree = (n: FlowNodeRef, x: number, y: number): number => {
    let box: Size;
    if (n.kind === 'frame') {
      const f = frameOf(n);
      box = f.size;
      const p = place(n.id, x, y, box);
      frames.set(n.id, p);
      for (const { id, pos } of f.inner) {
        place(`${n.id}/${id}`, x + FRAME_PAD + pos.x, y + FRAME_HEAD + FRAME_PAD + pos.y, pos);
      }
    } else {
      box = sizeOf(n.id);
      place(n.id, x, y, box);
    }
    const next = x + box.width + COL_GAP;
    return Math.max(box.height, stack(kids.get(n.id) ?? [], next, y));
  };

  /** Siblings stacked down from `y` in the column at `x`; their total height. */
  const stack = (list: FlowNodeRef[], x: number, y: number): number => {
    let used = 0;
    for (const n of list) {
      if (used > 0) used += ROW_GAP;
      used += subtree(n, x, y + used);
    }
    return used;
  };

  let y = 0;
  const lead = model.nodes.find((n) => n.kind === 'lead');
  const leadBox = sizeOf('lead');
  if (lead) {
    place('lead', 0, 0, leadBox);
    y = leadBox.height + ROUND_GAP;
  }
  const operator = { x: -OPERATOR_GAP, y: leadBox.height / 2 };

  const column1 = CARD_W + COL_GAP;
  for (const r of model.rounds) {
    const id = `round:${r.round}`;
    const card = sizeOf(id);
    place(id, 0, y, card);
    let blockH = card.height;
    if (r.collapsed) {
      const summary = sizeOf(`summary:${r.round}`);
      place(`summary:${r.round}`, column1, y, summary);
      blockH = Math.max(blockH, summary.height);
    } else {
      blockH = Math.max(blockH, stack(kids.get(id) ?? [], column1, y));
    }
    y += blockH + ROUND_GAP;
  }

  const height = Math.max(0, y - ROUND_GAP);
  const railX = right + RAIL_GAP;
  return { positions, frames, railX, operator, width: railX + RAIL_W, height };
}
