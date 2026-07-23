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
import { useThemeColors } from '../../lib/useThemeColors';
import { STEP_W, STEP_H } from '../../lib/nodeSize';

export interface GateNodeData extends Record<string, unknown> {
  node: GraphNode;
}

export type GateFlowNode = Node<GateNodeData, 'gate'>;

function GateNodeView({ data }: NodeProps<GateFlowNode>) {
  const { node } = data;
  const colors = useThemeColors();
  const s = stateStyle(colors, node.state);
  const handleStyle = { background: colors.border, width: 6, height: 6, border: 'none' } as const;
  const running = node.state === 'running';
  const awaiting = node.state === 'awaiting_approval';
  const gate = node.approval_gate;

  return (
    <div
      className={[
        'relative rounded-[10px] border border-dashed border-border bg-panel px-3 py-2 text-left shadow-card',
        running ? 'rg-pulse-run' : '',
        awaiting ? 'rg-pulse-await' : '',
        node.state === 'pending' ? 'opacity-75' : '',
      ].join(' ')}
      style={{ width: STEP_W, minHeight: STEP_H }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      {/* colored top-bar */}
      <div
        className="absolute left-0 right-0 top-0 h-[3px] rounded-t-[10px]"
        style={{ background: s.color }}
      />

      <div className="flex items-center gap-2 pt-0.5">
        <span
          className="inline-flex h-[15px] w-[15px] shrink-0 items-center justify-center rounded text-meta font-bold leading-none text-white"
          style={{ background: s.color }}
          aria-hidden
        >
          {s.glyph}
        </span>
        <span className="flex-1 truncate text-ui font-semibold text-ink">{node.id}</span>
        <span className="text-meta text-ink-mute tabular-nums">
          {node.state === 'pending' ? '—' : s.label}
        </span>
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
        <span className="rounded-full border border-border px-1.5 py-px text-meta text-ink-dim">
          ◇ gate
        </span>
        {gate?.auto_approve && (
          <span className="rounded bg-surface px-1.5 py-px text-meta text-ink-dim">auto</span>
        )}
      </div>

      {gate?.has_on_reject && (
        <div className="mt-1 truncate text-meta text-ink-mute">↳ on reject</div>
      )}

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(GateNodeView);
