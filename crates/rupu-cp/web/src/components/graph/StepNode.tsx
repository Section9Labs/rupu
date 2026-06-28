// StepNode — the atomic run-graph card (kind: 'step' / 'panel'-less step / gate).
//
// Anatomy (per the graph-pro mockup): colored top-bar · status glyph · name ·
// duration · agent chip. A soft blue pulse ring while running. React Flow
// source/target handles are present but visually minimal (read-only graph).

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { STATE_STYLE } from './stepStyle';
import { STEP_W, STEP_H } from '../../lib/nodeSize';

export interface StepNodeData extends Record<string, unknown> {
  node: GraphNode;
}

type StepFlowNode = Node<StepNodeData, 'step'>;

const handleStyle = {
  background: '#cbd5e1',
  width: 6,
  height: 6,
  border: 'none',
} as const;

function StepNodeView({ data }: NodeProps<StepFlowNode>) {
  const { node } = data;
  const s = STATE_STYLE[node.state];
  const running = node.state === 'running';
  const awaiting = node.state === 'awaiting_approval';

  return (
    <div
      className={[
        'relative rounded-[10px] border bg-white px-3 py-2 text-left shadow-card',
        running ? 'rg-pulse-run' : '',
        awaiting ? 'rg-pulse-await' : '',
        node.state === 'pending' ? 'opacity-75' : '',
      ].join(' ')}
      style={{ borderColor: '#e5e7eb', width: STEP_W, minHeight: STEP_H }}
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

      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded bg-slate-100 px-1.5 py-px text-meta text-slate-500">
          {node.kind === 'panel' ? 'panel' : 'step'}
        </span>
        {node.agent && (
          <span className="truncate rounded bg-slate-100 px-1.5 py-px text-meta text-slate-500">
            {node.agent}
          </span>
        )}
      </div>

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(StepNodeView);
