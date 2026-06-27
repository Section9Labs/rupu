// EditableStepNode — one editable canvas node, parameterized by `data.node.data.kind`.
//
// Mirrors the read-only graph node visual language (colored top-bar · kind chip ·
// id · summary line) from components/graph/*, but is purely declarative from the
// editor's GraphNode and a per-node `problems` list (drives the red validity dot).
// Source/target handles are present so edges can be drawn on the editable canvas.

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode, StepKind, StepNodeData } from '../../../lib/workflowGraph';

// Node data carried on the xyflow node. Exported so WorkflowEditorGraph projects
// the exact same shape when it derives the flow `nodes`.
export interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  problems: string[];
}

type EditableFlowNode = Node<NodeData, 'editable'>;

const handleStyle = { background: '#cbd5e1', width: 7, height: 7, border: 'none' } as const;

// Colored top-bar + kind chip per kind: step/blue, for_each/violet,
// parallel/purple, panel/amber (echoing the read-only node palette). All classes
// are static literals so Tailwind's content scanner keeps them.
const KIND_STYLE: Record<StepKind, { bar: string; chip: string }> = {
  step: { bar: '#1860f2', chip: 'bg-blue-50 text-blue-600' },
  for_each: { bar: '#8b5cf6', chip: 'bg-violet-50 text-violet-600' },
  parallel: { bar: '#9333ea', chip: 'bg-purple-50 text-purple-600' },
  panel: { bar: '#f59e0b', chip: 'bg-amber-50 text-amber-600' },
};

/** One-line summary of a step, by kind. */
function summarize(d: StepNodeData): string {
  switch (d.kind) {
    case 'for_each':
      return `for_each: ${d.for_each ?? ''}`;
    case 'parallel':
      return `parallel · ${(d.parallel ?? []).length} sub-steps`;
    case 'panel':
      return `panel · ${(d.panel?.panelists ?? []).length} panelists`;
    default:
      return d.agent ?? '(no agent)';
  }
}

function EditableStepNode({ data }: NodeProps<EditableFlowNode>) {
  const { node, problems } = data;
  const d = node.data;
  const style = KIND_STYLE[d.kind];
  const hasProblems = problems.length > 0;

  return (
    <div
      className="relative w-[220px] rounded-[10px] border border-border bg-white px-3 py-2 text-left shadow-card"
      style={{ minHeight: 72 }}
    >
      <Handle type="target" position={Position.Top} style={handleStyle} />

      {/* colored top-bar */}
      <div
        className="absolute left-0 right-0 top-0 h-[3px] rounded-t-[10px]"
        style={{ background: style.bar }}
      />

      <div className="flex items-center gap-2 pt-0.5">
        <span className={`rounded px-1.5 py-px text-[10px] font-medium ${style.chip}`}>{d.kind}</span>
        <span className="flex-1 truncate text-[12px] font-semibold text-ink">{d.id}</span>
        {hasProblems && (
          <span
            className="inline-block h-2.5 w-2.5 shrink-0 rounded-full bg-red-500"
            title={problems.join('\n')}
            aria-label="has problems"
          />
        )}
      </div>

      <div className="mt-1.5 truncate text-[11px] text-ink-mute">{summarize(d)}</div>

      <Handle type="source" position={Position.Bottom} style={handleStyle} />
    </div>
  );
}

export default memo(EditableStepNode);
