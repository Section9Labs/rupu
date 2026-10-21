// StepNode — the run graph's agent-step card (kind `step`, and the kinds with
// no card of their own: run / split / join / branch). Head: kind tile · name ·
// status capsule; body: the agent's identity (`leaf · agent`, then
// provider/model on its own line, so neither clips the other); foot: the kind.
// The frame (NodeFrame) carries the motion.

import { memo } from 'react';
import type { NodeProps, Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { nodeSize } from '../../lib/nodeSize';
import { AgentIdentity } from '../codename/AgentIdentity';
import { NodeFoot, NodeFrame, NodeHead } from './NodeFrame';
import WarnMark from './WarnMark';

export interface StepNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Entrance slot (see `useEntranceSlots`). */
  slot?: number;
}

type StepFlowNode = Node<StepNodeData, 'step'>;

function StepNodeView({ data }: NodeProps<StepFlowNode>) {
  const { node } = data;
  const box = nodeSize(node);
  return (
    <NodeFrame state={node.state} kind={node.kind} slot={data.slot} width={box.width} minHeight={box.height}>
      <NodeHead kind={node.kind} name={node.id} state={node.state}>
        <WarnMark warnings={node.warnings} />
      </NodeHead>
      <AgentIdentity
        className="gx-body gx-ident"
        codename={node.codename}
        derived={node.codenameDerived}
        agent={node.agent}
        provider={node.provider}
        model={node.model}
      />
      <NodeFoot kind={node.kind} />
    </NodeFrame>
  );
}

export default memo(StepNodeView);
