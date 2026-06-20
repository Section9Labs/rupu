// PanelLoopNode — a `panel` step with a gate fix-loop. Renders the panel
// container, a `round {current}/{max}` counter when known, the gate condition
// (`until_severity` / `max_iterations`), and an animated "↻ looping" loop cue
// while running — matching the fanout-loop mockup's panel + gate block.
//
// The model's GraphNode does not carry panelist names, so we surface the gate
// + round rather than fabricating reviewers.

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { STATE_STYLE } from './stepStyle';
import { PANEL_W, PANEL_H } from '../../lib/nodeSize';

export interface PanelLoopNodeData extends Record<string, unknown> {
  node: GraphNode;
}

type PanelFlowNode = Node<PanelLoopNodeData, 'panel'>;

const handleStyle = { background: '#c4b5fd', width: 6, height: 6, border: 'none' } as const;

function PanelLoopNodeView({ data }: NodeProps<PanelFlowNode>) {
  const { node } = data;
  const s = STATE_STYLE[node.state];
  const running = node.state === 'running';
  const gate = node.gate;
  const round = node.round;

  return (
    <div
      className={['relative rounded-[12px] border px-2.5 py-2', running ? 'rg-pulse-await' : ''].join(' ')}
      style={{ borderColor: '#c4b5fd', background: '#faf5ff', width: PANEL_W, minHeight: PANEL_H }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <div className="flex items-center justify-between gap-2 text-[10px] font-bold uppercase tracking-wide text-brand-500">
        <span className="truncate">
          panel · {node.id}
          {round && <span className="ml-1 tabular-nums">· round {round.current}/{round.max}</span>}
        </span>
        {running && (
          <span className="rg-loop-spin text-[12px] leading-none text-[#f59e0b]" aria-label="looping">
            ↻
          </span>
        )}
      </div>

      {/* gate condition block */}
      <div
        className="mt-1.5 flex items-center gap-1.5 rounded-[8px] border px-1.5 py-1"
        style={{ borderColor: '#fde68a', background: '#fffbeb' }}
      >
        <span
          className="inline-flex h-3 w-3 shrink-0 items-center justify-center rounded-[3px] text-[8px] font-bold leading-none text-white"
          style={{ background: s.color }}
          aria-hidden
        >
          {s.glyph}
        </span>
        <span className="text-[10px] font-medium text-[#92400e]">
          {gate ? (
            <>
              gate ≥ {gate.until_severity} · max {gate.max_iterations}
            </>
          ) : (
            'gate'
          )}
        </span>
      </div>

      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded bg-white/70 px-1.5 py-px text-[10px] text-brand-700 ring-1 ring-brand-100">
          {s.label}
        </span>
        {node.agent && (
          <span className="truncate rounded bg-white/70 px-1.5 py-px text-[10px] text-slate-500">
            {node.agent}
          </span>
        )}
      </div>

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(PanelLoopNodeView);
