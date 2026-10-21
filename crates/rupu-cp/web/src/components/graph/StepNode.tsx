// StepNode — the atomic run-graph card (kind: 'step' / 'panel'-less step / gate).
//
// Anatomy (per the graph-pro mockup): colored top-bar · status glyph · name ·
// duration · kind pill · agent identity (name line + provider/model line). The frame (NodeFrame) glows
// in the state's color while running, waiting or failed. React Flow
// source/target handles are present but visually minimal (read-only graph).

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { stateStyle } from './stepStyle';
import { NodeFrame, KindBar, StateBadge } from './NodeFrame';
import { useThemeColors } from '../../lib/useThemeColors';
import { nodeSize } from '../../lib/nodeSize';
import { runKindAccent, runKindIcon, runKindLabel } from './kindBridge';
import { AgentIdentity } from '../codename/AgentIdentity';
import WarnMark from './WarnMark';

export interface StepNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Entrance slot (see `entranceSlots`). */
  slot?: number;
}

type StepFlowNode = Node<StepNodeData, 'step'>;

function StepNodeView({ data }: NodeProps<StepFlowNode>) {
  const { node } = data;
  const colors = useThemeColors();
  const s = stateStyle(colors, node.state);
  const handleStyle = { background: colors.border, width: 6, height: 6, border: 'none' } as const;
  // Kind channel (identity) vs status channel (overlay): the top-bar and pill
  // carry the step's KIND color while the glyph badge + label keep the STATE
  // color, so a failed step still reads as failed.
  const accent = runKindAccent(node.kind);
  const barColor = colors.get(accent);
  const KindIcon = runKindIcon(node.kind);
  const box = nodeSize(node);

  return (
    <NodeFrame
      state={node.state}
      slot={data.slot}
      className="rounded-[10px] border border-border bg-panel px-3 py-2 text-left shadow-card"
      style={{ width: box.width, minHeight: box.height }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <KindBar color={barColor} />

      <div className="flex items-center gap-2 pt-0.5">
        <StateBadge state={node.state} color={s.color} glyph={s.glyph} />
        <span className="flex-1 truncate text-ui font-semibold text-ink">{node.id}</span>
        <WarnMark warnings={node.warnings} />
        <span key={node.state} className="rgx-swap text-meta text-ink-mute tabular-nums">
          {node.state === 'pending' ? '—' : s.label}
        </span>
      </div>

      <div className="mt-1.5 flex items-center gap-1.5">
        <span
          data-testid="rg-kindpill"
          className="inline-flex items-center gap-1 rounded px-1.5 py-px text-meta font-medium"
          style={{ background: colors.alpha(accent, 0.14), color: colors.get(accent) }}
        >
          <KindIcon size={10} aria-hidden />
          {runKindLabel(node.kind)}
        </span>
      </div>

      {/* Identity gets its own rows — `leaf · agent`, then provider/model on a
          smaller second line — so the 170px card never clips provider/model
          away (nodeSize reserves STEP_AGENT_H for this). */}
      <AgentIdentity
        className="mt-1.5 text-meta text-ink-dim"
        codename={node.codename}
        derived={node.codenameDerived}
        agent={node.agent}
        provider={node.provider}
        model={node.model}
      />

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </NodeFrame>
  );
}

export default memo(StepNodeView);
