// ParallelNode — a bordered container for a `parallel` step. Header shows the
// aggregate roll-up (`parallel · {done}/{total}`); children render as stacked
// chips, each colored by its own state (graph-pro mockup: the purple-bordered
// container with side-by-side sub-steps).

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { STATE_STYLE } from './stepStyle';
import { nodeSize } from '../../lib/nodeSize';

export interface ParallelNodeData extends Record<string, unknown> {
  node: GraphNode;
}

type ParallelFlowNode = Node<ParallelNodeData, 'parallel'>;

const handleStyle = { background: '#c4b5fd', width: 6, height: 6, border: 'none' } as const;

function ParallelNodeView({ data }: NodeProps<ParallelFlowNode>) {
  const { node } = data;
  const subs = node.parallel ?? [];
  const total = subs.length;
  const done = subs.filter((s) => s.state === 'done').length;
  const running = node.state === 'running';
  const box = nodeSize(node);

  return (
    <div
      className={['relative rounded-[12px] border px-2 py-1.5', running ? 'rg-pulse-run' : ''].join(' ')}
      style={{ borderColor: '#c4b5fd', background: '#faf5ff', width: box.width, minHeight: box.height }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <div className="mb-1.5 flex items-center justify-between gap-3 text-meta font-bold uppercase tracking-wide text-brand-500">
        <span className="truncate">parallel · {node.id}</span>
        <span className="tabular-nums">
          {done}/{total} ✓
        </span>
      </div>

      <div className="flex flex-col gap-1">
        {subs.map((sub) => {
          const ss = STATE_STYLE[sub.state];
          return (
            <div
              key={sub.id}
              className="flex items-center gap-1.5 rounded-[6px] border bg-white px-1.5 py-1"
              style={{ borderColor: '#e5e7eb' }}
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
