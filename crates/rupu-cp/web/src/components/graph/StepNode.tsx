// StepNode — the atomic run-graph card (kind: 'step' / 'panel'-less step / gate).
//
// Anatomy (per the graph-pro mockup): colored top-bar · status glyph · name ·
// duration · agent chip. A soft blue pulse ring while running. React Flow
// source/target handles are present but visually minimal (read-only graph).

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { stateStyle } from './stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import { STEP_W, STEP_H } from '../../lib/nodeSize';
import { runKindAccent, runKindIcon, runKindLabel } from './kindBridge';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';

export interface StepNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** 'next' turns on the kind-colored paint; absent/'classic' keeps today's. */
  ui?: WorkflowEditorUi;
}

type StepFlowNode = Node<StepNodeData, 'step'>;

function StepNodeView({ data }: NodeProps<StepFlowNode>) {
  const { node } = data;
  const colors = useThemeColors();
  const s = stateStyle(colors, node.state);
  const handleStyle = { background: colors.border, width: 6, height: 6, border: 'none' } as const;
  const running = node.state === 'running';
  const awaiting = node.state === 'awaiting_approval';
  const next = data.ui === 'next';
  // Kind channel (identity) vs status channel (overlay): in next the top-bar
  // and pill carry the step's KIND color while the glyph badge + label keep
  // the STATE color, so a failed step still reads as failed.
  const accent = runKindAccent(node.kind);
  const barColor = next ? colors.get(accent) : s.color;
  const KindIcon = runKindIcon(node.kind);

  return (
    <div
      className={[
        'relative rounded-[10px] border border-border bg-panel px-3 py-2 text-left shadow-card',
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
        style={{ background: barColor }}
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
        {next ? (
          <span
            data-testid="rg-kindpill"
            className="inline-flex items-center gap-1 rounded px-1.5 py-px text-meta font-medium"
            style={{ background: colors.alpha(accent, 0.14), color: colors.get(accent) }}
          >
            <KindIcon size={10} aria-hidden />
            {runKindLabel(node.kind)}
          </span>
        ) : (
          <span className="rounded bg-surface px-1.5 py-px text-meta text-ink-dim">
            {node.kind === 'panel' ? 'panel' : 'step'}
          </span>
        )}
        {node.agent && (
          <span className="truncate rounded bg-surface px-1.5 py-px text-meta text-ink-dim">
            {node.agent}
          </span>
        )}
      </div>

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(StepNodeView);
