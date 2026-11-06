// ParallelNode — a `parallel` step: one card whose rows are its sub-steps,
// each with its own status capsule (a running row is tinted and carries the
// progress line) and, when it runs an agent, that agent's identity.

import { memo } from 'react';
import type { NodeProps, Node } from '@xyflow/react';
import type { GraphNode } from '../../lib/runGraphModel';
import { nodeSize } from '../../lib/nodeSize';
import { useStateCue } from '../../lib/graph/runMotion';
import { AgentIdentity } from '../codename/AgentIdentity';
import { NodeFoot, NodeFrame, NodeHead, RunLine, StatusCapsule } from './NodeFrame';
import WarnMark from './WarnMark';

export interface ParallelNodeData extends Record<string, unknown> {
  node: GraphNode;
  /** Entrance slot (see `useEntranceSlots`). */
  slot?: number;
  /** Messages the step's agents sent or received (the run's message layer). */
  messages?: number;
  /** An end of the selected (or hovered) message. */
  lit?: boolean;
}

type ParallelFlowNode = Node<ParallelNodeData, 'parallel'>;
type Sub = NonNullable<GraphNode['parallel']>[number];

function SubRow({ sub }: { sub: Sub }) {
  const { cue } = useStateCue(sub.state);
  return (
    <div data-state={sub.state} data-cue={cue ?? undefined} className="gx-subrow gx-s">
      <div className="gx-subrow-head">
        <span className="gx-subname">{sub.id}</span>
        <StatusCapsule state={sub.state} small />
      </div>
      <AgentIdentity
        className="gx-ident gx-ident-sm"
        codename={sub.codename}
        derived={sub.codenameDerived}
        agent={sub.agent}
        provider={sub.provider}
        model={sub.model}
      />
      {sub.state === 'running' && <RunLine />}
    </div>
  );
}

function ParallelNodeView({ data }: NodeProps<ParallelFlowNode>) {
  const { node } = data;
  const subs = node.parallel ?? [];
  const done = subs.filter((s) => s.state === 'done').length;
  const box = nodeSize(node);
  return (
    <NodeFrame
      state={node.state}
      kind={node.kind}
      slot={data.slot}
      lit={data.lit}
      width={box.width}
      minHeight={box.height}
      testId="rg-container"
      live={false}
    >
      <NodeHead kind={node.kind} name={node.id} state={node.state} messages={data.messages}>
        <WarnMark warnings={node.warnings} />
        <span className="gx-count">
          {done}/{subs.length}
        </span>
      </NodeHead>
      <div className="gx-subrows">
        {subs.map((sub) => (
          <SubRow key={sub.id} sub={sub} />
        ))}
        {subs.length === 0 && <div className="gx-sub">no sub-steps</div>}
      </div>
      <NodeFoot kind={node.kind} />
    </NodeFrame>
  );
}

export default memo(ParallelNodeView);
