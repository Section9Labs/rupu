// ActionNode — a connector-action card in the run graph (kind `action`). The
// tool name (`scm.prs.create`) is the headline, set in monospace so it reads as
// a code identifier; the step id and a `connector` caption sit underneath.

import { memo } from 'react';
import type { NodeProps, Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { ACTION_H, STEP_W } from '../../lib/nodeSize';
import { NodeFoot, NodeFrame, NodeHead } from './NodeFrame';
import WarnMark from './WarnMark';

export interface ActionNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Entrance slot (see `useEntranceSlots`). */
  slot?: number;
  /** Messages the step's agents sent or received (the run's message layer). */
  messages?: number;
  /** An end of the selected (or hovered) message. */
  lit?: boolean;
}

export type ActionFlowNode = Node<ActionNodeData, 'action'>;

function ActionNodeView({ data }: NodeProps<ActionFlowNode>) {
  const { node } = data;
  return (
    <NodeFrame
      state={node.state}
      kind={node.kind}
      slot={data.slot}
      lit={data.lit}
      width={STEP_W}
      minHeight={ACTION_H}
    >
      <NodeHead kind={node.kind} name={node.id} state={node.state} messages={data.messages}>
        <WarnMark warnings={node.warnings} />
      </NodeHead>
      <div className="gx-body">
        <div className="gx-line font-mono">{node.action ?? node.id}</div>
        <div className="gx-sub">connector</div>
      </div>
      <NodeFoot kind={node.kind} />
    </NodeFrame>
  );
}

export default memo(ActionNodeView);
