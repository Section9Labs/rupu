// LoopGroupNode — the dashed group-boundary rendered BEHIND a loop's member
// nodes (workflow.rs `Workflow.loops`, Phase 3 Task 5, spec §5).
//
// There's no `type: 'group'`/`parentId` structure anywhere else in this editor
// (member nodes keep their own independent, absolute canvas positions — a real
// xyflow parent/child relationship would force relative positioning and touch
// every drag/layout code path). Instead this is a plain extra xyflow node,
// computed to cover the member nodes' bounding box and given a negative
// `zIndex` so it paints behind them — the "bordered container behind the
// nodes" fallback the spec explicitly allows when no existing group-boundary
// primitive exists (there isn't one). Not draggable/selectable/connectable —
// it's a read-derived visual, not something the canvas lets you grab.

import { memo } from 'react';
import type { Node, NodeProps } from '@xyflow/react';
import type { GraphNode, WorkflowLoop } from '../../../lib/workflowGraph';
import { editorNodeSize } from '../../../lib/workflowLayout';
import { useThemeColors } from '../../../lib/useThemeColors';

/** Padding (px) around the tightest bounding box of the loop's member
 *  nodes — enough to read as a distinct enclosing boundary rather than a
 *  shrink-wrap outline flush against the member cards. */
const PAD = 22;
/** Extra headroom above the box so the floating name/badge label (which
 *  sits astride the top edge, `-top-3`) never clips a member node's own
 *  top-bar directly underneath it. */
const LABEL_HEADROOM = 20;

export interface LoopGroupBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The loop's member nodes' bounding box (position + kind-appropriate size,
 *  padded) — `null` when the loop has fewer than 1 resolvable member (e.g.
 *  every member id was deleted from the canvas; validation already flags
 *  this, rendering nothing is the right degrade). Pure — no DOM, so it's
 *  usable both for node placement and for hit-testing/tests. */
export function loopGroupBox(nodes: GraphNode[], loop: WorkflowLoop): LoopGroupBox | null {
  const members = nodes.filter((n) => loop.nodes.includes(n.id));
  if (members.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const n of members) {
    const { width, height } = editorNodeSize(n.data);
    minX = Math.min(minX, n.position.x);
    minY = Math.min(minY, n.position.y);
    maxX = Math.max(maxX, n.position.x + width);
    maxY = Math.max(maxY, n.position.y + height);
  }
  return {
    x: minX - PAD,
    y: minY - PAD - LABEL_HEADROOM,
    width: maxX - minX + PAD * 2,
    height: maxY - minY + PAD * 2 + LABEL_HEADROOM,
  };
}

/** Truncate `until` for the badge — the full expression is available via the
 *  `title` tooltip and the loop edit form; the badge is a glance, not the
 *  source of truth. */
function truncateUntil(until: string, max = 28): string {
  const trimmed = until.trim();
  if (trimmed.length <= max) return trimmed || '(no condition)';
  return `${trimmed.slice(0, max - 1)}…`;
}

export interface LoopGroupNodeData extends Record<string, unknown> {
  loop: WorkflowLoop;
  width: number;
  height: number;
  /** Loop-level problems (`problemsById[loopProblemKey(loop.name)]`) — the
   *  Task 5 "reuse the Phase-1 inline validation surface" requirement: a
   *  loop with a backend-mirrored validation error (overlap, cyclic
   *  sub-DAG, member-escape, …) shows it right on its own boundary instead
   *  of failing silently on save. */
  problems: string[];
  /** Live run overlay (Task 5 §5 "runtime overlay") — `undefined` when no
   *  run is open OR the run doesn't carry per-iteration loop data. Rendered
   *  ADDITIVELY next to the static until/max badge; never fabricated when
   *  absent (see the file-level note in WorkflowEditorGraph on why this is
   *  usually absent today). */
  runtime?: { iteration: number; converged: boolean; failed: boolean };
  onEdit?: () => void;
}

type LoopGroupFlowNode = Node<LoopGroupNodeData, 'loopGroup'>;

function LoopGroupNodeView({ data }: NodeProps<LoopGroupFlowNode>) {
  const { loop, width, height, problems, runtime, onEdit } = data;
  const colors = useThemeColors();
  const hasError = problems.length > 0;
  const borderColor = hasError ? colors.status.failed : colors.alpha('brand.500', 0.45);

  return (
    <div
      data-testid="loop-group"
      data-loop-name={loop.name}
      className="pointer-events-none rounded-[18px] border-2 border-dashed"
      style={{
        width,
        height,
        borderColor,
        background: colors.alpha(hasError ? 'status.failed' : 'brand.500', hasError ? 0.05 : 0.035),
      }}
    >
      <button
        type="button"
        onClick={onEdit}
        disabled={!onEdit}
        className="pointer-events-auto absolute -top-3 left-3 inline-flex max-w-[calc(100%-2rem)] items-center gap-1 truncate rounded-full border px-2 py-0.5 text-meta font-medium shadow-card"
        style={{
          borderColor,
          background: colors.panel,
          color: hasError ? colors.status.failed : colors.get('brand.500'),
        }}
        title={hasError ? problems.join('\n') : `${loop.name}: until ${loop.until || '(no condition)'}`}
      >
        <span aria-hidden>⟳</span>
        <span className="truncate">{loop.name}</span>
        <span className="text-ink-mute">· until {truncateUntil(loop.until)} · ≤{loop.maxIterations}</span>
        {hasError && <span aria-label="loop has problems">⚠</span>}
      </button>

      {runtime && (
        <div
          className="pointer-events-none absolute -top-3 right-3 inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-meta font-medium shadow-card"
          style={{
            borderColor: runtime.failed
              ? colors.status.failed
              : runtime.converged
                ? colors.status.done
                : colors.status.running,
            background: colors.panel,
            color: runtime.failed
              ? colors.status.failed
              : runtime.converged
                ? colors.status.done
                : colors.status.running,
          }}
        >
          iteration {runtime.iteration}/{loop.maxIterations}
          {runtime.converged ? ' · converged' : runtime.failed ? ' · failed' : ''}
        </div>
      )}
    </div>
  );
}

export default memo(LoopGroupNodeView);
