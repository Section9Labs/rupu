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
import { stateStyle, glyphBg } from './stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import { PANEL_W, PANEL_H } from '../../lib/nodeSize';

export interface PanelLoopNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Select a panelist/fixer unit's transcript — same callback FanoutNode uses. */
  onOpenUnit?: (stepId: string, index: number) => void;
}

type PanelFlowNode = Node<PanelLoopNodeData, 'panel'>;

function PanelLoopNodeView({ data }: NodeProps<PanelFlowNode>) {
  const { node, onOpenUnit } = data;
  const colors = useThemeColors();
  const handleStyle = {
    background: colors.alpha('brand.500', 0.5),
    width: 6,
    height: 6,
    border: 'none',
  } as const;
  const s = stateStyle(colors, node.state);
  const running = node.state === 'running';
  const gate = node.gate;
  const round = node.round;
  // Panelist/fixer runs surface as fanout units (folded by step_id). Each is a
  // clickable chip that selects its transcript.
  const units = node.fanout?.units ?? [];

  return (
    <div
      className={['relative rounded-[12px] border px-2.5 py-2', running ? 'rg-pulse-await' : ''].join(' ')}
      style={{
        borderColor: colors.alpha('brand.500', 0.4),
        background: colors.alpha('brand.500', 0.08),
        width: PANEL_W,
        minHeight: PANEL_H,
      }}
    >
      <Handle type="target" position={Position.Left} style={handleStyle} />

      <div className="flex items-center justify-between gap-2 text-meta font-bold uppercase tracking-wide text-brand-500">
        <span className="truncate">
          panel · {node.id}
          {round && <span className="ml-1 tabular-nums">· round {round.current}/{round.max}</span>}
        </span>
        {running && (
          <span
            className="rg-loop-spin text-ui leading-none"
            style={{ color: colors.status.awaiting }}
            aria-label="looping"
          >
            ↻
          </span>
        )}
      </div>

      {/* gate condition block */}
      <div
        className="mt-1.5 flex items-center gap-1.5 rounded-[8px] border px-1.5 py-1"
        style={{
          borderColor: colors.alpha('status.awaiting', 0.45),
          background: colors.alpha('status.awaiting', 0.12),
        }}
      >
        <span
          className="inline-flex h-3 w-3 shrink-0 items-center justify-center rounded-[3px] text-[8px] font-bold leading-none text-white"
          style={{ background: s.color }}
          aria-hidden
        >
          {s.glyph}
        </span>
        <span className="text-meta font-medium" style={{ color: colors.status.awaiting }}>
          {gate ? (
            <>
              gate ≥ {gate.until_severity} · max {gate.max_iterations}
            </>
          ) : (
            'gate'
          )}
        </span>
      </div>

      {/* panelist / fixer chips — each selects its transcript on click */}
      {units.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {units.map((u) => (
            <button
              key={u.index}
              type="button"
              title={`${u.key} · ${stateStyle(colors, u.state).label}`}
              onClick={() => onOpenUnit?.(node.id, u.index)}
              className="inline-flex items-center gap-1 rounded bg-panel/80 px-1.5 py-px text-meta text-ink-dim ring-1 ring-brand-100 transition-colors hover:bg-panel hover:text-brand-700"
            >
              <span
                className="inline-block h-2 w-2 shrink-0 rounded-[2px]"
                style={{ background: glyphBg(colors, u.state) }}
                aria-hidden
              />
              <span className="max-w-[88px] truncate">{u.key}</span>
            </button>
          ))}
        </div>
      )}

      <div className="mt-1.5 flex items-center gap-1.5">
        <span className="rounded bg-panel/70 px-1.5 py-px text-meta text-brand-700 ring-1 ring-brand-100">
          {s.label}
        </span>
        {node.agent && (
          <span className="truncate rounded bg-panel/70 px-1.5 py-px text-meta text-ink-dim">
            {node.agent}
          </span>
        )}
      </div>

      <Handle type="source" position={Position.Right} style={handleStyle} />
    </div>
  );
}

export default memo(PanelLoopNodeView);
