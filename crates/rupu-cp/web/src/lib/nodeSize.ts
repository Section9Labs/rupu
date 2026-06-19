// nodeSize.ts — per-kind size estimates for the run-graph nodes.
//
// dagre reserves a box per node; if that box is smaller than the rendered
// component, big nodes (parallel / fanout / panel) get packed too tightly and
// render OVERLAPPING each other. This module returns a generous per-kind box
// derived from each node's CONTENT so dagre's reserved box ≥ the rendered box.
//
// The node components consume the SAME shared constants exported here (applied
// as `style={{ width, minHeight }}` on each node root) so render == reservation
// by construction.

import type { GraphNode } from './runGraphModel';

export interface NodeBox {
  width: number;
  height: number;
}

// ---------------------------------------------------------------------------
// Shared constants — imported by both nodeSize() and the node components.
// ---------------------------------------------------------------------------

/** Plain step / panel-less step. */
export const STEP_W = 170;
export const STEP_H = 72;

/** ParallelNode container. */
export const PARALLEL_W = 210;
export const PARALLEL_HEADER_H = 24; // uppercase roll-up header + mb-1.5
export const PARALLEL_SUBROW_H = 22; // bordered chip row (py-1 + 12px text + border) + gap-1
export const PARALLEL_PAD_V = 16; // px-2 py-1.5 (top+bottom) + slack

/** FanoutNode — small inline grid (total ≤ INLINE_THRESHOLD). */
export const FANOUT_INLINE_THRESHOLD = 12;
export const FANOUT_INLINE_COLS = 8; // matches Math.min(total, 8) in FanoutNode
export const FANOUT_INLINE_CELL = 18; // 15px cell + 3px gap
export const FANOUT_INLINE_W = 210;
export const FANOUT_INLINE_HEADER_H = 22; // uppercase header + mb-1
export const FANOUT_INLINE_PAD_V = 16; // px-2 py-1.5 + slack

/** FanoutNode — large collapsed X/N card (total > INLINE_THRESHOLD). */
export const FANOUT_CARD_W = 250;
export const FANOUT_CARD_H = 210;

/** PanelLoopNode container. */
export const PANEL_W = 200;
export const PANEL_H = 120;

// ---------------------------------------------------------------------------
// nodeSize — the per-kind box used by dagre AND applied to the rendered root.
// ---------------------------------------------------------------------------

export function nodeSize(node: GraphNode): NodeBox {
  switch (node.kind) {
    case 'parallel': {
      const subs = node.parallel?.length ?? 0;
      // Always reserve at least one row's worth of height so the "no sub-steps"
      // placeholder is bounded too.
      const rows = Math.max(subs, 1);
      const height = PARALLEL_HEADER_H + rows * PARALLEL_SUBROW_H + PARALLEL_PAD_V;
      return { width: PARALLEL_W, height };
    }

    case 'for_each': {
      const total = node.fanout?.total ?? 0;
      if (total > 0 && total <= FANOUT_INLINE_THRESHOLD) {
        const cols = Math.min(total, FANOUT_INLINE_COLS);
        const gridRows = Math.max(Math.ceil(total / cols), 1);
        const height =
          FANOUT_INLINE_HEADER_H + gridRows * FANOUT_INLINE_CELL + FANOUT_INLINE_PAD_V;
        return { width: FANOUT_INLINE_W, height };
      }
      // Large-N collapsed card, or the slim pending card (total === 0). The
      // card box generously bounds both.
      return { width: FANOUT_CARD_W, height: FANOUT_CARD_H };
    }

    case 'panel':
      return { width: PANEL_W, height: PANEL_H };

    default:
      return { width: STEP_W, height: STEP_H };
  }
}
