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
// broken out in red, density grid of colored squares. All colors are themed via
// `useThemeColors()` so the card reads on both light and dark.

import { memo } from 'react';
import { Handle, Position, type NodeProps, type Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { stateStyle, glyphBg } from './stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import { nodeSize, FANOUT_INLINE_THRESHOLD, FANOUT_INLINE_COLS } from '../../lib/nodeSize';
import { runKindAccent } from './kindBridge';
import type { WorkflowEditorUi } from '../../hooks/useWorkflowEditorUi';

export interface FanoutNodeData extends Record<string, unknown> {
  node: GraphNode;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
  /** 'next' turns on the kind-colored container tint; absent/'classic' keeps today's. */
  ui?: WorkflowEditorUi;
}

type FanoutFlowNode = Node<FanoutNodeData, 'fanout'>;

const INLINE_THRESHOLD = FANOUT_INLINE_THRESHOLD;
const PREVIEW_CELLS = 60;

function FanoutNodeView({ data }: NodeProps<FanoutFlowNode>) {
  const { node, onOpenUnit, onExpandFanout } = data;
  const colors = useThemeColors();
  const next = data.ui === 'next';
  // Container identity: in next this is the SAME accent the editor paints
  // 'for_each' with (brand.500 — violet); classic keeps the legacy
  // running-blue tint. The empty-units placeholder card below stays
  // state-driven (it has no fanout yet, so there is nothing to fan out with
  // a kind identity) and the per-unit squares/progress gradient stay
  // state-colored — only the container tint switches on kind.
  const accentKey = next ? runKindAccent(node.kind) : 'status.running';
  const handleStyle = {
    background: colors.alpha(accentKey, 0.4),
    width: 6,
    height: 6,
    border: 'none',
  } as const;
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
    const border = isErr
      ? colors.alpha('status.failed', 0.4)
      : isActive
        ? colors.alpha('status.running', 0.4)
        : colors.border;
    const bg = isErr
      ? colors.alpha('status.failed', 0.12)
      : isActive
        ? colors.alpha('status.running', 0.12)
        : colors.panel;
    const labelColor = isErr
      ? colors.status.failed
      : isActive
        ? colors.status.running
        : colors.inkMute;
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
        <div className="text-meta font-bold uppercase tracking-wide" style={{ color: labelColor }}>
          for_each · {node.id}
        </div>
        <div className="mt-1 text-note text-ink-mute">{message}</div>
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
        data-testid="rg-container"
        className={['relative rounded-[12px] border px-2 py-1.5', running ? 'rg-pulse-run' : ''].join(' ')}
        style={{
          borderColor: colors.alpha(accentKey, 0.4),
          background: colors.alpha(accentKey, 0.12),
          width: box.width,
          minHeight: box.height,
        }}
      >
        <Handle type="target" position={Position.Left} style={handleStyle} />
        <div
          className="mb-1 flex items-center justify-between gap-3 text-meta font-bold uppercase tracking-wide"
          style={{ color: colors.get(accentKey) }}
        >
          <span className="truncate">for_each · {node.id} · {total}</span>
          <span className="tabular-nums">
            {done} ✓
            {failed > 0 && (
              <span className="ml-1" style={{ color: colors.status.failed }}>· {failed} ✕</span>
            )}
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
              title={`${u.key} · ${stateStyle(colors, u.state).label}`}
              onClick={() => onOpenUnit?.(node.id, u.index)}
              className="h-[15px] w-[15px] rounded-[3px] transition-transform hover:scale-110"
              style={{ background: glyphBg(colors, u.state) }}
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
      data-testid="rg-container"
      className={['relative rounded-[12px] border bg-panel px-3 py-2.5 shadow-card', running ? 'rg-pulse-run' : ''].join(' ')}
      style={{ borderColor: colors.alpha(accentKey, 0.4), width: box.width, minHeight: box.height }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <div
        className="text-meta font-bold uppercase tracking-wide"
        style={{ color: colors.get(accentKey) }}
      >
        for_each · {node.id}
      </div>

      <div className="mt-1 flex items-baseline gap-2">
        <span className="text-[22px] font-bold leading-none text-ink tabular-nums">{done}</span>
        <span className="text-note text-ink-mute">/ {total} units</span>
        <span
          className="ml-auto text-lead font-bold tabular-nums"
          style={{ color: colors.get(accentKey) }}
        >
          {pct}%
        </span>
      </div>

      <div
        className="mt-1.5 h-[9px] overflow-hidden rounded-[5px]"
        style={{ background: colors.surface }}
      >
        <div
          className="h-full"
          style={{
            width: `${pct}%`,
            background: `linear-gradient(90deg, ${colors.status.running}, ${colors.status.done})`,
          }}
        />
      </div>

      <div className="mt-1.5 flex flex-wrap gap-3 text-note text-ink-dim">
        <span>
          <b className="text-ink">{done}</b> done
        </span>
        <span>
          <b className="text-ink">{runningUnits}</b> running
        </span>
        <span>
          <b className="text-ink">{pending}</b> pending
        </span>
        {failed > 0 && (
          <span className="font-bold" style={{ color: colors.status.failed }}>{failed} failed</span>
        )}
      </div>

      <div className="mt-2 grid gap-[2px]" style={{ gridTemplateColumns: 'repeat(20, 9px)' }}>
        {preview.map((u) => (
          <span
            key={u.index}
            className="block h-[9px] w-[9px] rounded-[2px]"
            style={{ background: glyphBg(colors, u.state) }}
          />
        ))}
      </div>

      <button
        type="button"
        onClick={() => onExpandFanout?.(node.id)}
        className="mt-2 text-note font-medium hover:underline"
        style={{ color: colors.get(accentKey) }}
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
