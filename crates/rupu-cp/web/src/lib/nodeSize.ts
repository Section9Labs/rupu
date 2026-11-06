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
//
// Every card has the same frame (styles/graph-motion.css, `.gx-card`): a 1px
// border, 11px top / 10px bottom padding, a 20px head row (kind tile · name ·
// status capsule) and a 21px foot (9px gap + the 12px kind label). Bodies sit
// between them after a 9px gap.
// ---------------------------------------------------------------------------

/** Border + vertical padding + head + foot: the frame every card shares. */
export const CARD_FRAME_H = 2 + 21 + 20 + 21;
/** The gap between the head and a body. */
export const CARD_BODY_GAP = 9;
/** A two-line agent identity body (`leaf · agent` 16px, provider/model 16px). */
export const IDENTITY_H = 32;

/** Plain step / gate / action card width. */
export const STEP_W = 216;
/** A step with no agent identity: just the frame. */
export const STEP_H = CARD_FRAME_H;
/** A step that runs an agent: the frame + its identity body. Keyed off
 *  `agent`/`codename` — `agent` comes from the DAG, so the box is stable
 *  across the run instead of jumping when agent_started lands. */
export const STEP_AGENT_H = CARD_FRAME_H + CARD_BODY_GAP + IDENTITY_H;
/** An action card: the frame + the tool name and its `connector` caption. */
export const ACTION_H = CARD_FRAME_H + CARD_BODY_GAP + 30;
/** A gate card: the frame + one 28px row (Approve / Reject while it waits,
 *  its outcome or config otherwise) — reserved always, so the layout never
 *  jumps when the gate parks. */
export const GATE_H = CARD_FRAME_H + CARD_BODY_GAP + 28;

/** True when a node / sub-step / unit carries an agent identity to render. */
export function hasIdentity(x: { agent?: string; codename?: string }): boolean {
  return Boolean(x.agent || x.codename);
}

/** ParallelNode container. */
export const PARALLEL_W = 232;
/** Frame + the 8px gap above the rows. */
export const PARALLEL_HEADER_H = CARD_FRAME_H + 8;
/** A sub-step row: name + status capsule. */
export const PARALLEL_SUBROW_H = 32;
/** A sub-step row that also carries its agent identity (two extra lines). */
export const PARALLEL_SUBROW_ID_H = 62;
/** The 4px gap between rows. */
export const PARALLEL_ROW_GAP = 4;

/** FanoutNode — small inline grid (total ≤ INLINE_THRESHOLD). */
export const FANOUT_INLINE_THRESHOLD = 12;
export const FANOUT_INLINE_W = 232;
export const FANOUT_CELL_H = 24;
export const FANOUT_CELL_GAP = 5;
/** Columns of the inline unit grid: a row of up to 4, then rows of 4, then 6. */
export function fanoutInlineCols(total: number): number {
  return total <= 4 ? Math.max(total, 1) : total <= 8 ? 4 : 6;
}
/** A fan-out with no units yet (or none at all): the frame + a message line. */
export const FANOUT_EMPTY_H = CARD_FRAME_H + CARD_BODY_GAP + 16;

/** FanoutNode — large collapsed X/N card (total > INLINE_THRESHOLD). */
export const FANOUT_CARD_W = 264;
export const FANOUT_CARD_H = 224;

/** PanelLoopNode container: the frame + the gate-condition chip. */
export const PANEL_W = 244;
export const PANEL_H = CARD_FRAME_H + CARD_BODY_GAP + 24;
/** One panelist / fixer unit row (two-line identity + padding + gap). */
export const PANEL_UNIT_ROW_H = 44;

// ---------------------------------------------------------------------------
// nodeSize — the per-kind box used by dagre AND applied to the rendered root.
// ---------------------------------------------------------------------------

export function nodeSize(node: GraphNode): NodeBox {
  switch (node.kind) {
    case 'parallel': {
      const subs = node.parallel ?? [];
      // Always reserve at least one row's worth of height so the "no sub-steps"
      // placeholder is bounded too.
      const rowsH =
        subs.length === 0
          ? PARALLEL_SUBROW_H
          : subs.reduce((h, sub) => h + (hasIdentity(sub) ? PARALLEL_SUBROW_ID_H : PARALLEL_SUBROW_H), 0) +
            (subs.length - 1) * PARALLEL_ROW_GAP;
      return { width: PARALLEL_W, height: PARALLEL_HEADER_H + rowsH };
    }

    case 'for_each': {
      const total = node.fanout?.total ?? 0;
      if (total === 0) return { width: FANOUT_INLINE_W, height: FANOUT_EMPTY_H };
      if (total <= FANOUT_INLINE_THRESHOLD) {
        const rows = Math.ceil(total / fanoutInlineCols(total));
        const grid = rows * FANOUT_CELL_H + (rows - 1) * FANOUT_CELL_GAP;
        return { width: FANOUT_INLINE_W, height: CARD_FRAME_H + CARD_BODY_GAP + grid };
      }
      return { width: FANOUT_CARD_W, height: FANOUT_CARD_H };
    }

    case 'panel': {
      const units = node.fanout?.units.length ?? 0;
      return { width: PANEL_W, height: PANEL_H + (units > 0 ? 6 + units * PANEL_UNIT_ROW_H : 0) };
    }

    case 'gate':
      return { width: STEP_W, height: GATE_H };

    case 'action':
      return { width: STEP_W, height: ACTION_H };

    default:
      return { width: STEP_W, height: hasIdentity(node) ? STEP_AGENT_H : STEP_H };
  }
}
