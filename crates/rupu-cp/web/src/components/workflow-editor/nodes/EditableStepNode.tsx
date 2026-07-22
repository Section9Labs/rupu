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
import { useThemeColors, type ColorKey, type ThemeColors } from '../../../lib/useThemeColors';

// Node data carried on the xyflow node. Exported so WorkflowEditorGraph projects
// the exact same shape when it derives the flow `nodes`.
export interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  problems: string[];
}

type EditableFlowNode = Node<NodeData, 'editable'>;

// Per-kind accent → a THEMED palette token: step/blue (running), for_each/violet
// (brand), parallel/purple (sev-critical), panel/amber (awaiting), branch/green
// (done — a routing decision, distinct from every other kind). Resolved from
// the hook at render so the kind coloring matches the rest of the UI and stays
// legible on dark — the kind chips paint with an inline alpha tint of the accent
// (no fixed `bg-*-50` classes that wash out on near-black).
const KIND_KEY: Record<StepKind, ColorKey> = {
  step: 'status.running',
  for_each: 'brand.500',
  parallel: 'sev.critical',
  panel: 'status.awaiting',
  branch: 'status.done',
};

/** Inline style for a kind chip — soft accent tint bg + accent text. */
function kindChipStyle(colors: ThemeColors, kind: StepKind): React.CSSProperties {
  return { background: colors.alpha(KIND_KEY[kind], 0.14), color: colors.get(KIND_KEY[kind]) };
}

/** kind chip + agent chip — shared by step / for_each. */
function StepBody({ d, colors }: { d: StepNodeData; colors: ThemeColors }) {
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded px-1.5 py-px text-meta font-medium" style={kindChipStyle(colors, d.kind)}>
          {d.kind}
        </span>
        <span className="truncate rounded bg-surface px-1.5 py-px text-meta text-ink-dim">
          {d.agent ?? '(no agent)'}
        </span>
      </div>
      {d.kind === 'for_each' && (
        <div className="mt-1 truncate text-meta text-ink-mute">for_each: {d.for_each ?? ''}</div>
      )}
    </>
  );
}

/** header roll-up + stacked sub-step rows — mirrors ParallelNode. */
function ParallelBody({ d, colors }: { d: StepNodeData; colors: ThemeColors }) {
  const subs = d.parallel ?? [];
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded px-1.5 py-px text-meta font-medium" style={kindChipStyle(colors, 'parallel')}>
          parallel
        </span>
        <span className="text-meta text-ink-mute tabular-nums">· {subs.length}</span>
      </div>
      <div className="mt-1.5 flex flex-col gap-1">
        {subs.map((sub, i) => (
          <div
            key={sub.id || i}
            className="flex items-center gap-1.5 rounded-[6px] border border-border bg-panel px-1.5 py-1"
          >
            <span className="truncate text-note text-ink">{sub.id || `#${i}`}</span>
            <span className="ml-auto truncate text-meta text-ink-mute">{sub.agent || '(no agent)'}</span>
          </div>
        ))}
        {subs.length === 0 && <div className="px-1 py-0.5 text-meta text-ink-mute">no sub-steps</div>}
      </div>
    </>
  );
}

/** panelists count + optional gate block — mirrors PanelLoopNode. */
function PanelBody({ d, colors }: { d: StepNodeData; colors: ThemeColors }) {
  const panelists = d.panel?.panelists ?? [];
  const gate = d.panel?.gate;
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded px-1.5 py-px text-meta font-medium" style={kindChipStyle(colors, 'panel')}>
          panel
        </span>
        <span className="text-meta text-ink-mute tabular-nums">· {panelists.length} panelists</span>
      </div>
      {gate && (
        <div
          className="mt-1.5 flex items-center gap-1.5 rounded-[8px] border px-1.5 py-1"
          style={{
            borderColor: colors.alpha('status.awaiting', 0.45),
            background: colors.alpha('status.awaiting', 0.12),
          }}
        >
          <span className="text-meta font-medium" style={{ color: colors.status.awaiting }}>
            gate ≥ {gate.until_no_findings_at_severity_or_above ?? '—'}
          </span>
        </div>
      )}
    </>
  );
}

/** condition + then/else target summary — mirrors PanelBody's roll-up style.
 *  A branch step carries no agent/prompt; routing is entirely condition + the
 *  then/else target lists (edited via StepForm's BranchFields). */
function BranchBody({ d, colors }: { d: StepNodeData; colors: ThemeColors }) {
  const thenTargets = d.thenTargets ?? [];
  const elseTargets = d.elseTargets ?? [];
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded px-1.5 py-px text-meta font-medium" style={kindChipStyle(colors, 'branch')}>
          branch
        </span>
      </div>
      <div className="mt-1 truncate text-meta text-ink-mute">if: {d.condition || '(no condition)'}</div>
      <div className="mt-1.5 flex flex-col gap-1">
        <div className="flex items-center gap-1.5 rounded-[6px] border border-border bg-panel px-1.5 py-1">
          <span className="text-meta font-medium" style={{ color: colors.status.done }}>
            true
          </span>
          <span className="ml-auto truncate text-meta text-ink-mute">
            {thenTargets.length > 0 ? thenTargets.join(', ') : '—'}
          </span>
        </div>
        <div className="flex items-center gap-1.5 rounded-[6px] border border-border bg-panel px-1.5 py-1">
          <span className="text-meta font-medium" style={{ color: colors.status.failed }}>
            false
          </span>
          <span className="ml-auto truncate text-meta text-ink-mute">
            {elseTargets.length > 0 ? elseTargets.join(', ') : '—'}
          </span>
        </div>
      </div>
    </>
  );
}

function EditableStepNode({ data, selected }: NodeProps<EditableFlowNode>) {
  const { node, problems } = data;
  const d = node.data;
  const colors = useThemeColors();
  const handleStyle = { background: colors.border, width: 7, height: 7, border: 'none' } as const;
  const color = colors.get(KIND_KEY[d.kind]);
  const box = editorNodeSize(d);
  const hasProblems = problems.length > 0;

  return (
    <div
      className={[
        'relative rounded-[10px] border bg-panel px-3 py-2 text-left shadow-card',
        selected ? 'ring-2 ring-brand-500' : '',
      ].join(' ')}
      style={{ borderColor: selected ? color : colors.border, width: box.width, minHeight: box.height }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      {/* colored top-bar — by KIND (no run-state) */}
      <div
        className="absolute left-0 right-0 top-0 h-[3px] rounded-t-[10px]"
        style={{ background: color }}
      />

      <div className="flex items-center gap-2 pt-0.5">
        <span className="flex-1 truncate text-ui font-semibold text-ink">{d.id}</span>
        {hasProblems && (
          <span
            className="inline-block h-2.5 w-2.5 shrink-0 rounded-full bg-status-failed"
            title={problems.join('\n')}
            aria-label="has problems"
          />
        )}
      </div>

      {d.kind === 'parallel' ? (
        <ParallelBody d={d} colors={colors} />
      ) : d.kind === 'panel' ? (
        <PanelBody d={d} colors={colors} />
      ) : d.kind === 'branch' ? (
        <BranchBody d={d} colors={colors} />
      ) : (
        <StepBody d={d} colors={colors} />
      )}

      {/* branch nodes get TWO labeled source handles (one per arm) instead of
          the single default source handle every other kind uses. */}
      {d.kind === 'branch' ? (
        <>
          <Handle
            type="source"
            position={Position.Right}
            id="then"
            style={{ ...handleStyle, top: '38%', background: colors.status.done }}
          />
          <Handle
            type="source"
            position={Position.Right}
            id="else"
            style={{ ...handleStyle, top: '68%', background: colors.status.failed }}
          />
        </>
      ) : (
        <Handle type="source" position={Position.Right} style={handleStyle} />
      )}
    </div>
  );
}

export default memo(EditableStepNode);
