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
import { useThemeColors, type ThemeColors } from '../../../lib/useThemeColors';
import type { WorkflowEditorUi } from '../../../hooks/useWorkflowEditorUi';
import { KIND_ACCENT, KIND_ICON } from '../kindVisuals';

// Node data carried on the xyflow node. Exported so WorkflowEditorGraph projects
// the exact same shape when it derives the flow `nodes`.
export interface NodeData extends Record<string, unknown> {
  node: GraphNode;
  problems: string[];
  /** Workflow-editor-UI flag, threaded through so the node can restyle itself
   *  (Task 2+). Defaults to 'classic' when absent — no behavior/visual change
   *  in this task beyond the `data-ui` marker. */
  workflowEditorUi?: WorkflowEditorUi;
}

type EditableFlowNode = Node<NodeData, 'editable'>;

/** Inline style for a kind chip — soft accent tint bg + accent text. Accent
 *  resolved from the shared `kindVisuals.KIND_ACCENT` (also used by
 *  NodePalette) so the kind chips paint with an inline alpha tint of the
 *  accent (no fixed `bg-*-50` classes that wash out on near-black). */
function kindChipStyle(colors: ThemeColors, kind: StepKind): React.CSSProperties {
  return { background: colors.alpha(KIND_ACCENT[kind], 0.14), color: colors.get(KIND_ACCENT[kind]) };
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

/** connector tool name chip — classic look for an action step. */
function ActionBody({ d, colors }: { d: StepNodeData; colors: ThemeColors }) {
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded px-1.5 py-px text-meta font-medium" style={kindChipStyle(colors, 'action')}>
          action
        </span>
        <span className="truncate rounded bg-surface px-1.5 py-px text-meta text-ink-dim font-mono">
          {d.action || '(no tool)'}
        </span>
      </div>
    </>
  );
}

/** approval-gate roll-up — prompt snippet + auto tag; classic look. */
function GateBody({ d, colors }: { d: StepNodeData; colors: ThemeColors }) {
  return (
    <>
      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded px-1.5 py-px text-meta font-medium" style={kindChipStyle(colors, 'approval_gate')}>
          gate
        </span>
        {d.approvalAutoApprove ? (
          <span className="text-meta text-ink-mute tabular-nums">· auto</span>
        ) : null}
      </div>
      <div className="mt-1 truncate text-meta text-ink-mute">{d.approvalPrompt || 'awaiting approval'}</div>
    </>
  );
}

// ── "next" (instrument) look — mockup-ported bodies ────────────────────────
// Ported from flow-designer.html's `.node`/`.kindpill`/`.nid`/`.expr`/`.port`.
// Namespaced `.wfx-*` (Task 2 — CSS block in styles.css). Same underlying
// StepNodeData fields as the classic bodies above; only the markup/classes
// differ, so both looks stay driven by the exact same data.

/** mono agent line + optional for_each expr chip — next look for step/for_each. */
function StepBodyNext({ d }: { d: StepNodeData }) {
  return (
    <>
      <div className="wfx-agent">▸ {d.agent ?? '(no agent)'}</div>
      {d.kind === 'for_each' && <div className="wfx-expr">for_each: {d.for_each ?? ''}</div>}
    </>
  );
}

/** sub-step count + stacked pill rows — next look for parallel. */
function ParallelBodyNext({ d }: { d: StepNodeData }) {
  const subs = d.parallel ?? [];
  return (
    <>
      <div className="wfx-meta">
        {subs.length} sub-step{subs.length === 1 ? '' : 's'}
      </div>
      <div className="wfx-sublist">
        {subs.map((sub, i) => (
          <div key={sub.id || i} className="wfx-subrow">
            <span className="wfx-subid">{sub.id || `#${i}`}</span>
            <span className="wfx-subagent">{sub.agent || '(no agent)'}</span>
          </div>
        ))}
        {subs.length === 0 && <div className="wfx-empty">no sub-steps</div>}
      </div>
    </>
  );
}

/** panelist port pills + optional gate chip — next look for panel. */
function PanelBodyNext({ d }: { d: StepNodeData }) {
  const panelists = d.panel?.panelists ?? [];
  const gate = d.panel?.gate;
  return (
    <>
      <div className="wfx-meta">
        {panelists.length} panelist{panelists.length === 1 ? '' : 's'}
      </div>
      <div className="wfx-ports">
        {panelists.map((p) => (
          <span key={p} className="wfx-port">
            {p}
          </span>
        ))}
      </div>
      {gate && (
        <div className="wfx-gate">gate ≥ {gate.until_no_findings_at_severity_or_above ?? '—'}</div>
      )}
    </>
  );
}

/** condition expr chip + true/false port pills — next look for branch. */
function BranchBodyNext({ d }: { d: StepNodeData }) {
  const thenTargets = d.thenTargets ?? [];
  const elseTargets = d.elseTargets ?? [];
  return (
    <>
      <div className="wfx-expr">if {d.condition || '(no condition)'}</div>
      <div className="wfx-ports">
        <span className="wfx-port wfx-port-true">
          true{thenTargets.length > 0 ? ` → ${thenTargets.join(', ')}` : ''}
        </span>
        <span className="wfx-port wfx-port-false">
          false{elseTargets.length > 0 ? ` → ${elseTargets.join(', ')}` : ''}
        </span>
      </div>
    </>
  );
}

/** connector tool name chip — next look for an action step. */
function ActionBodyNext({ d }: { d: StepNodeData }) {
  return <div className="wfx-agent">⚡ {d.action || '(no tool)'}</div>;
}

/** approval-gate prompt snippet + optional auto chip — next look. */
function GateBodyNext({ d }: { d: StepNodeData }) {
  return (
    <>
      <div className="wfx-expr">{d.approvalPrompt || 'awaiting approval'}</div>
      {d.approvalAutoApprove ? <div className="wfx-meta">auto-approve</div> : null}
    </>
  );
}

function EditableStepNode({ data, selected }: NodeProps<EditableFlowNode>) {
  const { node, problems } = data;
  const ui = data.workflowEditorUi ?? 'classic';
  const d = node.data;
  const colors = useThemeColors();
  const handleStyle = { background: colors.border, width: 7, height: 7, border: 'none' } as const;
  const color = colors.get(KIND_ACCENT[d.kind]);
  const KindIcon = KIND_ICON[d.kind];
  const box = editorNodeSize(d);
  const hasProblems = problems.length > 0;

  // "next" (instrument) look — a wholly separate render path so the classic
  // markup below stays byte-identical. Same data (`d`/`colors`/`box`/handles),
  // new `.wfx-*` classes only.
  if (ui === 'next') {
    // Selection ring/glow computed from the SAME kind accent as the border —
    // one coherent color signal instead of accent-border + brand-purple ring.
    const selBoxShadow = selected
      ? `0 0 0 2px ${colors.alpha(KIND_ACCENT[d.kind], 0.3)}, 0 6px 20px ${colors.alpha(KIND_ACCENT[d.kind], 0.14)}`
      : undefined;
    return (
      <div
        data-ui={ui}
        className="wfx-node"
        style={{
          borderColor: selected ? color : undefined,
          boxShadow: selBoxShadow,
          width: box.width,
          minHeight: box.height,
        }}
      >
        <Handle type="target" position={Position.Left} style={handleStyle} />

        {/* .wfx-clip clips the bar/head/body to the card's radius — a 3px-tall
            absolutely-positioned bar can't hold its own 12px corner radius, so
            it must be clipped by an ancestor instead of rounding itself.
            Handles stay OUTSIDE the clip (siblings, on the card border). */}
        <div className="wfx-clip">
          {/* colored top-bar — by KIND (no run-state) */}
          <div className="wfx-bar" style={{ background: color }} />

          <div className="wfx-head">
            <span className="wfx-kindpill" style={kindChipStyle(colors, d.kind)}>
              <KindIcon className="wfx-kindicon" size={12} strokeWidth={2} aria-hidden />
              {d.kind}
            </span>
            <span className="wfx-nid">{d.id}</span>
            {hasProblems && (
              <span className="wfx-problem" title={problems.join('\n')} aria-label="has problems" />
            )}
          </div>

          <div className="wfx-body">
            {d.kind === 'parallel' ? (
              <ParallelBodyNext d={d} />
            ) : d.kind === 'panel' ? (
              <PanelBodyNext d={d} />
            ) : d.kind === 'branch' ? (
              <BranchBodyNext d={d} />
            ) : d.kind === 'action' ? (
              <ActionBodyNext d={d} />
            ) : d.kind === 'approval_gate' ? (
              <GateBodyNext d={d} />
            ) : (
              <StepBodyNext d={d} />
            )}
          </div>
        </div>

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

  return (
    <div
      data-ui={ui}
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
      ) : d.kind === 'action' ? (
        <ActionBody d={d} colors={colors} />
      ) : d.kind === 'approval_gate' ? (
        <GateBody d={d} colors={colors} />
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
