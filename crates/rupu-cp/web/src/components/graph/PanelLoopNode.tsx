// PanelLoopNode — a `panel` step with a gate fix-loop: the round counter
// (`round 2/5`, turning ↻ while it loops), the gate's condition, and one row
// per panelist / fixer run (each selects its transcript). The model carries no
// panelist names before they run, so the rows are the runs, not a roster.

import { memo } from 'react';
import type { NodeProps, Node } from '@xyflow/react';
import type { GraphNode, UnitView } from '../../lib/runGraphModel';
import { stateStyle } from './stepStyle';
import { useThemeColors } from '../../lib/useThemeColors';
import { nodeSize } from '../../lib/nodeSize';
import { memberLabel } from '../../lib/codename';
import { useStateCue } from '../../lib/graph/runMotion';
import { AgentIdentity } from '../codename/AgentIdentity';
import { NodeFoot, NodeFrame, NodeHead, StatusGlyph } from './NodeFrame';
import WarnMark, { unitWarningSuffix } from './WarnMark';

export interface PanelLoopNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Select a panelist/fixer unit's transcript — same callback FanoutNode uses. */
  onOpenUnit?: (stepId: string, index: number) => void;
  /** Entrance slot (see `useEntranceSlots`). */
  slot?: number;
}

type PanelFlowNode = Node<PanelLoopNodeData, 'panel'>;

function PanelRow({ unit, onOpen }: { unit: UnitView; onOpen?: () => void }) {
  const colors = useThemeColors();
  const { cue } = useStateCue(unit.state);
  const who = unit.codename ? memberLabel(unit.codename, unit.agent, unit.provider, unit.model) : unit.key;
  return (
    <button
      type="button"
      data-state={unit.state}
      data-cue={cue ?? undefined}
      title={`${who} · ${stateStyle(colors, unit.state).label}${unitWarningSuffix(unit.warnings)}`}
      onClick={onOpen}
      className="gx-prow gx-s nodrag"
    >
      <StatusGlyph state={unit.state} />
      {unit.codename || unit.agent ? (
        <AgentIdentity
          className="gx-ident gx-ident-sm min-w-0 flex-1"
          codename={unit.codename}
          derived={unit.codenameDerived}
          agent={unit.agent}
          provider={unit.provider}
          model={unit.model}
        />
      ) : (
        <span className="gx-subname min-w-0 flex-1 truncate">{unit.key}</span>
      )}
      <WarnMark warnings={unit.warnings?.map((message) => ({ index: unit.index, message }))} />
    </button>
  );
}

function PanelLoopNodeView({ data }: NodeProps<PanelFlowNode>) {
  const { node, onOpenUnit } = data;
  const gate = node.gate;
  const round = node.round;
  const units = node.fanout?.units ?? [];
  const box = nodeSize(node);
  return (
    <NodeFrame
      state={node.state}
      kind={node.kind}
      slot={data.slot}
      width={box.width}
      minHeight={box.height}
      testId="rg-container"
    >
      <NodeHead kind={node.kind} name={node.id} state={node.state}>
        <WarnMark warnings={node.warnings} />
        {round && (
          <span className="gx-count">
            {node.state === 'running' && (
              <span className="gx-loop" aria-label="looping">
                ↻
              </span>
            )}
            {round.current}/{round.max}
          </span>
        )}
      </NodeHead>
      <div className="gx-body gx-chip">
        {gate ? (
          <>
            gate ≥ {gate.until_severity} · max {gate.max_iterations}
          </>
        ) : (
          'gate'
        )}
      </div>
      {units.length > 0 && (
        <div className="gx-prows">
          {units.map((u) => (
            <PanelRow key={u.index} unit={u} onOpen={() => onOpenUnit?.(node.id, u.index)} />
          ))}
        </div>
      )}
      <NodeFoot kind={node.kind}>{node.agent && <span className="gx-sub truncate">{node.agent}</span>}</NodeFoot>
    </NodeFrame>
  );
}

export default memo(PanelLoopNodeView);
