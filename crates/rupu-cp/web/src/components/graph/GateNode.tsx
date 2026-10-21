// GateNode — a standalone approval-gate node in the run graph (kind: 'gate').
//
// Distinct from `StepNode` via a DASHED border and a "◇ gate" kind pill (a
// bordered-pill treatment rather than a literal rotated-square silhouette —
// keeps the glyph legible at graph zoom without a CSS-transform shape). Same
// state-driven glyph vocabulary as StepNode (via `stepStyle.ts`): ⏸ awaiting,
// ✓ done, ✕ failed. `auto_approve` renders a small "auto" tag; `has_on_reject`
// renders a "↳ on reject" caption — declarative only, per Task 3: no new
// edges/topology are added here (a follow-up may turn this into a real second
// handle once the reject branch is actually wired).
//
// NOTE: `data.node.approval_gate` is the standalone approval-gate node's own
// config — NOT `data.node.gate`, which is the *panel* step's iteration-loop
// gate (see `PanelLoopNode`). Do not conflate the two.

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { stateStyle } from './stepStyle';
import { NodeFrame, KindBar, StateBadge } from './NodeFrame';
import { useThemeColors } from '../../lib/useThemeColors';
import { STEP_W, STEP_H } from '../../lib/nodeSize';
import { runKindAccent, runKindIcon, runKindLabel } from './kindBridge';

export interface GateNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Entrance slot (see `entranceSlots`). */
  slot?: number;
}

export type GateFlowNode = Node<GateNodeData, 'gate'>;

function GateNodeView({ data }: NodeProps<GateFlowNode>) {
  const { node } = data;
  const colors = useThemeColors();
  const s = stateStyle(colors, node.state);
  const handleStyle = { background: colors.border, width: 6, height: 6, border: 'none' } as const;
  const gate = node.approval_gate;
  // Kind channel (identity) vs status channel (overlay): the top-bar and pill
  // carry the gate's KIND color while the glyph badge + label keep the STATE
  // color, so a failed gate still reads as failed.
  const accent = runKindAccent(node.kind);
  const barColor = colors.get(accent);
  const KindIcon = runKindIcon(node.kind);

  return (
    <NodeFrame
      state={node.state}
      slot={data.slot}
      className="rounded-[10px] border border-dashed border-border bg-panel px-3 py-2 text-left shadow-card"
      style={{ width: STEP_W, minHeight: STEP_H }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <KindBar color={barColor} />

      <div className="flex items-center gap-2 pt-0.5">
        <StateBadge state={node.state} color={s.color} glyph={s.glyph} />
        <span className="flex-1 truncate text-ui font-semibold text-ink">{node.id}</span>
        <span key={node.state} className="rgx-swap text-meta text-ink-mute tabular-nums">
          {node.state === 'pending' ? '—' : s.label}
        </span>
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
        <span
          data-testid="rg-kindpill"
          className="inline-flex items-center gap-1 rounded px-1.5 py-px text-meta font-medium"
          style={{ background: colors.alpha(accent, 0.14), color: colors.get(accent) }}
        >
          <KindIcon size={10} aria-hidden />
          {runKindLabel(node.kind)}
        </span>
        {gate?.auto_approve && (
          <span className="rounded bg-surface px-1.5 py-px text-meta text-ink-dim">auto</span>
        )}
      </div>

      {gate?.has_on_reject && (
        <div className="mt-1 truncate text-meta text-ink-mute">↳ on reject</div>
      )}

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </NodeFrame>
  );
}

export default memo(GateNodeView);
