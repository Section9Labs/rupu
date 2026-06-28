// FanoutNode — a `for_each` step. Two presentations driven by unit count:
//
//   N ≤ 12 : inline grid of clickable unit squares (click → that unit's
//            transcript via onOpenUnit).
//   N > 12 : a collapsed card leading with `{done}/{total}` + a single
//            % bar, `{failed} failed` in red when any failed, a small
//            density-preview grid, and an "expand" affordance that opens
//            the step's unit file-browser via onExpandFanout.
//
// Faithful to the fanout-loop mockup: big X / N, blue→green % bar, failures
// broken out in red, density grid of colored squares.

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { STATE_STYLE, glyphBg } from './stepStyle';
import { nodeSize, FANOUT_INLINE_THRESHOLD, FANOUT_INLINE_COLS } from '../../lib/nodeSize';

export interface FanoutNodeData extends Record<string, unknown> {
  node: GraphNode;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
}

type FanoutFlowNode = Node<FanoutNodeData, 'fanout'>;

const handleStyle = { background: '#bfdbfe', width: 6, height: 6, border: 'none' } as const;

const INLINE_THRESHOLD = FANOUT_INLINE_THRESHOLD;
const PREVIEW_CELLS = 60;

function FanoutNodeView({ data }: NodeProps<FanoutFlowNode>) {
  const { node, onOpenUnit, onExpandFanout } = data;
  const fo = node.fanout;
  const running = node.state === 'running';
  const box = nodeSize(node);

  // No units to show. This is NOT always "awaiting" — a for_each over an empty
  // list completes with zero units, and a failed/skipped step never fans out.
  // Render a card that reflects the step's actual state instead of a permanent
  // blue "awaiting units…".
  if (!fo || fo.total === 0) {
    const state = node.state;
    const isErr = state === 'failed';
    const isActive = state === 'running' || state === 'pending' || state === 'awaiting_approval';
    const border = isErr ? '#fecaca' : isActive ? '#bfdbfe' : '#e5e7eb';
    const bg = isErr ? '#fef2f2' : isActive ? '#eff6ff' : '#ffffff';
    const labelColor = isErr ? 'text-[#ef4444]' : isActive ? 'text-[#3b82f6]' : 'text-ink-mute';
    const message =
      state === 'running'
        ? 'starting units…'
        : state === 'done'
          ? 'no units — nothing to fan out'
          : state === 'failed'
            ? 'failed before fan-out'
            : state === 'skipped'
              ? 'skipped'
              : 'awaiting units…';
    return (
      <div
        className={['relative rounded-[12px] border px-3 py-2', running ? 'rg-pulse-run' : ''].join(' ')}
        style={{ borderColor: border, background: bg, width: box.width, minHeight: box.height }}
      >
        <Handle type="target" position={Position.Left} style={handleStyle} />
        <div className={['text-[10px] font-bold uppercase tracking-wide', labelColor].join(' ')}>
          for_each · {node.id}
        </div>
        <div className="mt-1 text-[11px] text-ink-mute">{message}</div>
        <Handle type="source" position={Position.Right} style={handleStyle} />
      </div>
    );
  }

  const total = fo.total;
  const done = fo.byState.done;
  const failed = fo.byState.failed;
  const runningUnits = fo.byState.running;
  const pending = fo.byState.pending + fo.byState.awaiting_approval + fo.byState.skipped;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  // ---- Small fan-out: inline clickable grid -------------------------------
  if (total <= INLINE_THRESHOLD) {
    const cols = Math.min(total, FANOUT_INLINE_COLS);
    return (
      <div
        className={['relative rounded-[12px] border px-2 py-1.5', running ? 'rg-pulse-run' : ''].join(' ')}
        style={{ borderColor: '#bfdbfe', background: '#eff6ff', width: box.width, minHeight: box.height }}
      >
        <Handle type="target" position={Position.Left} style={handleStyle} />
        <div className="mb-1 flex items-center justify-between gap-3 text-[10px] font-bold uppercase tracking-wide text-[#3b82f6]">
          <span className="truncate">for_each · {node.id} · {total}</span>
          <span className="tabular-nums">
            {done} ✓{failed > 0 && <span className="ml-1 text-[#ef4444]">· {failed} ✕</span>}
          </span>
        </div>
        <div
          className="grid gap-[3px]"
          style={{ gridTemplateColumns: `repeat(${cols}, 15px)` }}
        >
          {fo.units.map((u) => (
            <button
              key={u.index}
              type="button"
              title={`${u.key} · ${STATE_STYLE[u.state].label}`}
              onClick={() => onOpenUnit?.(node.id, u.index)}
              className="h-[15px] w-[15px] rounded-[3px] transition-transform hover:scale-110"
              style={{ background: glyphBg(u.state) }}
            />
          ))}
        </div>
        <Handle type="source" position={Position.Right} style={handleStyle} />
      </div>
    );
  }

  // ---- Large fan-out: collapsed X/N card ----------------------------------
  const preview = fo.units.slice(0, PREVIEW_CELLS);
  return (
    <div
      className={['relative rounded-[12px] border bg-white px-3 py-2.5 shadow-card', running ? 'rg-pulse-run' : ''].join(' ')}
      style={{ borderColor: '#bfdbfe', width: box.width, minHeight: box.height }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <div className="text-[10px] font-bold uppercase tracking-wide text-[#3b82f6]">
        for_each · {node.id}
      </div>

      <div className="mt-1 flex items-baseline gap-2">
        <span className="text-[22px] font-bold leading-none text-ink tabular-nums">{done}</span>
        <span className="text-[11px] text-ink-mute">/ {total} units</span>
        <span className="ml-auto text-[13px] font-bold text-[#3b82f6] tabular-nums">{pct}%</span>
      </div>

      <div className="mt-1.5 h-[9px] overflow-hidden rounded-[5px]" style={{ background: '#e2e8f0' }}>
        <div
          className="h-full"
          style={{ width: `${pct}%`, background: 'linear-gradient(90deg,#3b82f6,#22c55e)' }}
        />
      </div>

      <div className="mt-1.5 flex flex-wrap gap-3 text-[11px] text-ink-dim">
        <span>
          <b className="text-ink">{done}</b> done
        </span>
        <span>
          <b className="text-ink">{runningUnits}</b> running
        </span>
        <span>
          <b className="text-ink">{pending}</b> pending
        </span>
        {failed > 0 && <span className="font-bold text-[#ef4444]">{failed} failed</span>}
      </div>

      <div className="mt-2 grid gap-[2px]" style={{ gridTemplateColumns: 'repeat(20, 9px)' }}>
        {preview.map((u) => (
          <span
            key={u.index}
            className="block h-[9px] w-[9px] rounded-[2px]"
            style={{ background: glyphBg(u.state) }}
          />
        ))}
      </div>

      <button
        type="button"
        onClick={() => onExpandFanout?.(node.id)}
        className="mt-2 text-[11px] font-medium text-[#3b82f6] hover:underline"
      >
        ▸ expand all {total}
        {failed > 0 && ` · failed (${failed})`}
        {runningUnits > 0 && ` · running (${runningUnits})`}
      </button>

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(FanoutNodeView);
