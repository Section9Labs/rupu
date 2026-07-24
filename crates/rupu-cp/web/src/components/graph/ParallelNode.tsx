// ParallelNode — a bordered container for a `parallel` step. Header shows the
// aggregate roll-up (`parallel · {done}/{total}`); children render as stacked
// chips, each colored by its own state (graph-pro mockup: the purple-bordered
// container with side-by-side sub-steps).

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { stateStyle } from './stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import { nodeSize } from '../../lib/nodeSize';
import { runKindAccent } from './kindBridge';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';

export interface ParallelNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** 'next' turns on the kind-colored container tint; absent/'classic' keeps today's. */
  ui?: WorkflowEditorUi;
}

type ParallelFlowNode = Node<ParallelNodeData, 'parallel'>;

function ParallelNodeView({ data }: NodeProps<ParallelFlowNode>) {
  const { node } = data;
  const colors = useThemeColors();
  const next = data.ui === 'next';
  // Container identity: in next this is the SAME accent the editor paints
  // 'parallel' with (sev.critical); classic keeps the legacy brand tint.
  const accentKey = next ? runKindAccent(node.kind) : 'brand.500';
  const handleStyle = {
    background: colors.alpha(accentKey, 0.5),
    width: 6,
    height: 6,
    border: 'none',
  } as const;
  const subs = node.parallel ?? [];
  const total = subs.length;
  const done = subs.filter((s) => s.state === 'done').length;
  const running = node.state === 'running';
  const box = nodeSize(node);

  return (
    <div
      data-testid="rg-container"
      className={['relative rounded-[12px] border px-2 py-1.5', running ? 'rg-pulse-run' : ''].join(' ')}
      style={{
        borderColor: colors.alpha(accentKey, 0.4),
        background: colors.alpha(accentKey, 0.08),
        width: box.width,
        minHeight: box.height,
      }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <div
        className={[
          'mb-1.5 flex items-center justify-between gap-3 text-meta font-bold uppercase tracking-wide',
          next ? '' : 'text-brand-500',
        ].join(' ')}
        style={next ? { color: colors.get(accentKey) } : undefined}
      >
        <span className="truncate">parallel · {node.id}</span>
        <span className="tabular-nums">
          {done}/{total} ✓
        </span>
      </div>

      <div className="flex flex-col gap-1">
        {subs.map((sub) => {
          const ss = stateStyle(colors, sub.state);
          return (
            <div
              key={sub.id}
              className="flex items-center gap-1.5 rounded-[6px] border border-border bg-panel px-1.5 py-1"
            >
              <span
                className="inline-flex h-3 w-3 shrink-0 items-center justify-center rounded-[3px] text-[8px] font-bold leading-none text-white"
                style={{ background: ss.color }}
                aria-hidden
              >
                {ss.glyph}
              </span>
              <span className="truncate text-note text-ink">{sub.id}</span>
            </div>
          );
        })}
        {total === 0 && <div className="px-1 py-0.5 text-meta text-ink-mute">no sub-steps</div>}
      </div>

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(ParallelNodeView);
