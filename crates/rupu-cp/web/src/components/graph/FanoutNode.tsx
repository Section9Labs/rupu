// FanoutNode — a `for_each` step. Three presentations:
//
//   no units yet  the card with a line saying where the fan-out stands;
//   N ≤ 12        a grid of unit cells, each a button selecting that unit's
//                 transcript, its glyph morphing as the unit runs and lands
//                 (a landing cell pops once);
//   N > 12        a summary card: done / total, the percentage, a progress
//                 bar, the counts by state, a density preview of the first 60
//                 units, and "expand all" opening the unit browser.

import { memo } from 'react';
import type { NodeProps, Node } from '@xyflow/react';
import type { GraphNode, UnitView } from '../../lib/runGraphModel';
import { stateStyle } from './stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import { fanoutInlineCols, nodeSize, FANOUT_INLINE_THRESHOLD } from '../../lib/nodeSize';
import { useStateCue } from '../../lib/graph/runMotion';
import { memberLabel, parseCodename } from '../../lib/codename';
import { cn } from '../../lib/cn';
import { derivedTitle } from '../codename/CrewChip';
import { RoleBadge } from '../codename/RoleBadge';
import { NodeFoot, NodeFrame, NodeHead, StatusGlyph } from './NodeFrame';
import WarnMark, { unitWarningSuffix } from './WarnMark';

export interface FanoutNodeData extends Record<string, unknown> {
  node: GraphNode;
  onOpenUnit?: (stepId: string, index: number) => void;
  onExpandFanout?: (stepId: string) => void;
  /** Entrance slot (see `useEntranceSlots`). */
  slot?: number;
}

type FanoutFlowNode = Node<FanoutNodeData, 'fanout'>;

const PREVIEW_CELLS = 60;

/** Compact crew-role tag for a fan-out: role badge + role word + `×N`. Fan-out
 *  steps carry no step-level codename (instances live on units), so the role
 *  comes from the node's codename when set, else the first unit's. Per-unit
 *  names stay out of the node — they live in unit titles and the unit browser,
 *  which keeps the card O(1) at hundreds–thousands of units. */
function FanoutRole({ node, total }: { node: GraphNode; total: number }) {
  const named: { codename?: string; codenameDerived?: boolean } | undefined = node.codename
    ? node
    : node.fanout?.units.find((u) => u.codename);
  const codename = named?.codename;
  if (!codename) return null;
  const derived = named?.codenameDerived === true;
  const { role } = parseCodename(codename);
  if (!role) return null;
  return (
    <span
      data-testid="rg-fanout-role"
      title={derivedTitle(codename, derived)}
      data-derived={derived || undefined}
      className={cn('gx-role', derived && 'opacity-60')}
    >
      <RoleBadge role={role} size={10} />
      <span className="font-mono">
        {role} ×{total}
      </span>
    </span>
  );
}

/** Hover title for one unit: `leaf · agent · provider/model · state`. */
function unitTitle(node: GraphNode, u: UnitView, stateLabel: string): string {
  const who = u.codename ? memberLabel(u.codename, node.agent, u.provider, u.model) : u.key;
  return `${who} · ${stateLabel}${unitWarningSuffix(u.warnings)}`;
}

function UnitCell({ node, unit, onOpen }: { node: GraphNode; unit: UnitView; onOpen?: () => void }) {
  const colors = useThemeColors();
  const { cue } = useStateCue(unit.state);
  return (
    <button
      type="button"
      data-state={unit.state}
      data-cue={cue ?? undefined}
      title={unitTitle(node, unit, stateStyle(colors, unit.state).label)}
      onClick={onOpen}
      className={cn('gx-cell gx-s nodrag', unit.warnings?.length && 'ring-warn')}
    >
      <StatusGlyph state={unit.state} />
    </button>
  );
}

function FanoutNodeView({ data }: NodeProps<FanoutFlowNode>) {
  const { node, onOpenUnit, onExpandFanout } = data;
  const fo = node.fanout;
  const box = nodeSize(node);
  const frame = {
    state: node.state,
    kind: node.kind,
    slot: data.slot,
    width: box.width,
    minHeight: box.height,
  } as const;

  // No units to show. This is NOT always "awaiting" — a for_each over an empty
  // list completes with zero units, and a failed/skipped step never fans out.
  if (!fo || fo.total === 0) {
    const message =
      node.state === 'running'
        ? 'starting units…'
        : node.state === 'done'
          ? 'no units — nothing to fan out'
          : node.state === 'failed'
            ? 'failed before fan-out'
            : node.state === 'skipped'
              ? 'skipped'
              : 'awaiting units…';
    return (
      <NodeFrame {...frame} testId="rg-container">
        <NodeHead kind={node.kind} name={node.id} state={node.state}>
          <WarnMark warnings={node.warnings} />
        </NodeHead>
        <div key={message} className="gx-body gx-sub gx-swap">
          {message}
        </div>
        <NodeFoot kind={node.kind} />
      </NodeFrame>
    );
  }

  const total = fo.total;
  const done = fo.byState.done;
  const failed = fo.byState.failed;
  const runningUnits = fo.byState.running;
  const pending = fo.byState.pending + fo.byState.awaiting_approval + fo.byState.skipped;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  // The summary card says done / total in its body; the grid says it here.
  const head = (count: boolean) => (
    <NodeHead kind={node.kind} name={node.id} state={node.state}>
      <WarnMark warnings={node.warnings} />
      {count && (
        <span className="gx-count" aria-label={`${done} of ${total} done`}>
          {done}/{total}
        </span>
      )}
    </NodeHead>
  );

  // ---- Small fan-out: a grid of unit cells --------------------------------
  if (total <= FANOUT_INLINE_THRESHOLD) {
    return (
      <NodeFrame {...frame} testId="rg-container" live={false}>
        {head(true)}
        <div className="gx-body gx-cells" style={{ gridTemplateColumns: `repeat(${fanoutInlineCols(total)}, 1fr)` }}>
          {fo.units.map((u) => (
            <UnitCell key={u.index} node={node} unit={u} onOpen={() => onOpenUnit?.(node.id, u.index)} />
          ))}
        </div>
        <NodeFoot kind={node.kind}>
          {failed > 0 && <span className="gx-failcount">{failed} failed</span>}
          <FanoutRole node={node} total={total} />
        </NodeFoot>
      </NodeFrame>
    );
  }

  // ---- Large fan-out: the summary card ------------------------------------
  const preview = fo.units.slice(0, PREVIEW_CELLS);
  return (
    <NodeFrame {...frame} testId="rg-container" live={false}>
      {head(false)}
      <div className="gx-body">
        <div className="gx-big">
          <span className="gx-big-n">{done}</span>
          <span className="gx-sub">/ {total} units</span>
          <span className="gx-pct">{pct}%</span>
        </div>
        <div className="gx-bar" data-live={node.state === 'running' || undefined}>
          <span className="gx-bar-fill" style={{ scale: `${pct / 100} 1` }} />
        </div>
        <div className="gx-stats">
          <span>
            <b>{done}</b> done
          </span>
          <span>
            <b>{runningUnits}</b> running
          </span>
          <span>
            <b>{pending}</b> pending
          </span>
          {failed > 0 && <span className="gx-failcount">{failed} failed</span>}
        </div>
        <div className="gx-mini">
          {preview.map((u) => (
            <span key={u.index} data-state={u.state} className="gx-mini-cell gx-s" />
          ))}
        </div>
        <button type="button" className="gx-link nodrag" onClick={() => onExpandFanout?.(node.id)}>
          ▸ expand all {total}
          {failed > 0 && ` · failed (${failed})`}
          {runningUnits > 0 && ` · running (${runningUnits})`}
        </button>
      </div>
      <NodeFoot kind={node.kind}>
        <FanoutRole node={node} total={total} />
      </NodeFoot>
    </NodeFrame>
  );
}

export default memo(FanoutNodeView);
