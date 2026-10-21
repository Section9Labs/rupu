// GateNode — a standalone approval gate in the run graph (kind `gate`). While
// it waits it carries Approve / Reject (GateActions, wired to the same
// endpoints as the run page's gate banner); otherwise its row says how it is
// set up: auto-approve, and whether a rejection has an `on_reject` chain.
//
// NOTE: `data.node.approval_gate` is the standalone gate's own config — NOT
// `data.node.gate`, which is the *panel* step's iteration-loop gate (see
// `PanelLoopNode`). Do not conflate the two.

import { memo } from 'react';
import type { NodeProps, Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { GATE_H, STEP_W } from '../../lib/nodeSize';
import { NodeFoot, NodeFrame, NodeHead } from './NodeFrame';
import GateActions, { type GateControl } from './GateActions';
import WarnMark from './WarnMark';

export interface GateNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Entrance slot (see `useEntranceSlots`). */
  slot?: number;
  /** Approve / reject while the gate waits; absent on a read-only graph. */
  gate?: GateControl;
}

export type GateFlowNode = Node<GateNodeData, 'gate'>;

function GateNodeView({ data }: NodeProps<GateFlowNode>) {
  const { node, gate } = data;
  const config = node.approval_gate;
  const waiting = node.state === 'awaiting_approval' && gate !== undefined;
  return (
    <NodeFrame state={node.state} kind={node.kind} slot={data.slot} width={STEP_W} minHeight={GATE_H}>
      <NodeHead kind={node.kind} name={node.id} state={node.state}>
        <WarnMark warnings={node.warnings} />
      </NodeHead>
      {waiting ? (
        <GateActions stepId={node.id} control={gate} />
      ) : (
        <div className="gx-body gx-row">
          {config?.auto_approve && <span className="gx-tag">auto</span>}
          {config?.has_on_reject && <span className="gx-sub">↳ on reject</span>}
          {!config?.auto_approve && !config?.has_on_reject && (
            <span className="gx-sub">{node.state === 'awaiting_approval' ? 'waiting for approval' : 'approval gate'}</span>
          )}
        </div>
      )}
      <NodeFoot kind={node.kind} />
    </NodeFrame>
  );
}

export default memo(GateNodeView);
