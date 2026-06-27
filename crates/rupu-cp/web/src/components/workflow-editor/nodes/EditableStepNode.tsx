// EditableStepNode — one editable canvas node, parameterized by `node.data.kind`.
//
// Mirrors the read-only Runs graph cards (components/graph/StepNode ·
// ParallelNode · PanelLoopNode): a `rounded-[10px] border bg-white px-3 py-2`
// card with a 3px colored top-bar, the id, a kind chip, and Left(target)/
// Right(source) handles for the LR flow. Editor nodes have NO run-state, so the
// top-bar / chip are colored by KIND instead. Per-kind bodies echo the run
// containers (parallel → stacked sub-step rows, panel → panelists + gate block).
//
// Colors are JS values consumed via inline `style` (NOT Tailwind class
// interpolation) so dynamic coloring stays static at the Tailwind level —
// mirroring StepNode's approach. The red ⚠ dot surfaces a node's `problems`.

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode, StepKind, StepNodeData } from '../../../lib/workflowGraph';
import { editorNodeSize } from '../../../lib/workflowLayout';

// Node data carried on the xyflow node. Exported so WorkflowEditorGraph projects
// the exact same shape when it derives the flow `nodes`.
export interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  problems: string[];
}

type EditableFlowNode = Node<NodeData, 'editable'>;

const handleStyle = { background: '#cbd5e1', width: 7, height: 7, border: 'none' } as const;

// Per-kind accent color (top-bar) + kind-chip classes: step/blue, for_each/
// violet, parallel/purple, panel/amber. Chip classes are static literals so
// Tailwind's content scanner keeps them.
const KIND_COLOR: Record<StepKind, string> = {
  step: '#1860f2',
  for_each: '#8b5cf6',
  parallel: '#9333ea',
  panel: '#f59e0b',
};
const KIND_CHIP: Record<StepKind, string> = {
  step: 'bg-blue-50 text-blue-600',
  for_each: 'bg-violet-50 text-violet-600',
  parallel: 'bg-purple-50 text-purple-600',
  panel: 'bg-amber-50 text-amber-600',
};

/** kind chip + agent chip — shared by step / for_each. */
function StepBody({ d }: { d: StepNodeData }) {
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className={`rounded px-1.5 py-px text-[10px] font-medium ${KIND_CHIP[d.kind]}`}>{d.kind}</span>
        <span className="truncate rounded bg-slate-100 px-1.5 py-px text-[10px] text-slate-500">
          {d.agent ?? '(no agent)'}
        </span>
      </div>
      {d.kind === 'for_each' && (
        <div className="mt-1 truncate text-[10px] text-ink-mute">for_each: {d.for_each ?? ''}</div>
      )}
    </>
  );
}

/** header roll-up + stacked sub-step rows — mirrors ParallelNode. */
function ParallelBody({ d }: { d: StepNodeData }) {
  const subs = d.parallel ?? [];
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className={`rounded px-1.5 py-px text-[10px] font-medium ${KIND_CHIP.parallel}`}>parallel</span>
        <span className="text-[10px] text-ink-mute tabular-nums">· {subs.length}</span>
      </div>
      <div className="mt-1.5 flex flex-col gap-1">
        {subs.map((sub, i) => (
          <div
            key={sub.id || i}
            className="flex items-center gap-1.5 rounded-[6px] border border-border bg-white px-1.5 py-1"
          >
            <span className="truncate text-[11px] text-ink">{sub.id || `#${i}`}</span>
            <span className="ml-auto truncate text-[10px] text-ink-mute">{sub.agent || '(no agent)'}</span>
          </div>
        ))}
        {subs.length === 0 && <div className="px-1 py-0.5 text-[10px] text-ink-mute">no sub-steps</div>}
      </div>
    </>
  );
}

/** panelists count + optional gate block — mirrors PanelLoopNode. */
function PanelBody({ d }: { d: StepNodeData }) {
  const panelists = d.panel?.panelists ?? [];
  const gate = d.panel?.gate;
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className={`rounded px-1.5 py-px text-[10px] font-medium ${KIND_CHIP.panel}`}>panel</span>
        <span className="text-[10px] text-ink-mute tabular-nums">· {panelists.length} panelists</span>
      </div>
      {gate && (
        <div
          className="mt-1.5 flex items-center gap-1.5 rounded-[8px] border px-1.5 py-1"
          style={{ borderColor: '#fde68a', background: '#fffbeb' }}
        >
          <span className="text-[10px] font-medium text-[#92400e]">
            gate ≥ {gate.until_no_findings_at_severity_or_above ?? '—'}
          </span>
        </div>
      )}
    </>
  );
}

function EditableStepNode({ data, selected }: NodeProps<EditableFlowNode>) {
  const { node, problems } = data;
  const d = node.data;
  const color = KIND_COLOR[d.kind];
  const box = editorNodeSize(d);
  const hasProblems = problems.length > 0;

  return (
    <div
      className={[
        'relative rounded-[10px] border bg-white px-3 py-2 text-left shadow-card',
        selected ? 'ring-2 ring-brand-500' : '',
      ].join(' ')}
      style={{ borderColor: selected ? color : '#e5e7eb', width: box.width, minHeight: box.height }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      {/* colored top-bar — by KIND (no run-state) */}
      <div
        className="absolute left-0 right-0 top-0 h-[3px] rounded-t-[10px]"
        style={{ background: color }}
      />

      <div className="flex items-center gap-2 pt-0.5">
        <span className="flex-1 truncate text-[12px] font-semibold text-ink">{d.id}</span>
        {hasProblems && (
          <span
            className="inline-block h-2.5 w-2.5 shrink-0 rounded-full bg-red-500"
            title={problems.join('\n')}
            aria-label="has problems"
          />
        )}
      </div>

      {d.kind === 'parallel' ? (
        <ParallelBody d={d} />
      ) : d.kind === 'panel' ? (
        <PanelBody d={d} />
      ) : (
        <StepBody d={d} />
      )}

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(EditableStepNode);
