// WorkflowFrameNode — a workflow child drawn as the workflow it is: a framed
// region whose inner nodes are its real run graph (the map adds them as
// their own nodes over the frame, with the run graph's components, see
// RunGraph's NODE_TYPES), the child space's arcs between them, and a compact
// board strip along its foot. A workflow the lead generated is labelled
// "written by the lead in round N" with a button that opens the definition it
// wrote. Its first render asks for its graph (`onLoad`), so a frame loads
// when it first comes into view.

import { memo, useEffect } from 'react';
import { Link } from 'react-router-dom';
import { Handle, Position, type Node, type NodeProps } from '@xyflow/react';
import { FileCode2, PenLine, Workflow } from 'lucide-react';
import type { RunChild } from '../../lib/api';
import type { FlowBoardSpace } from '../../lib/flow/flowModel';
import { STRIP_LIMIT, type RailSection } from '../../lib/flow/flowEdges';
import { FRAME_HEAD } from '../../lib/flow/flowLayout';
import { StatusCapsule } from '../graph/NodeFrame';
import BoardRail from './BoardRail';
import { InferredMark, MessageCount, childSpend, childState } from './ChildNode';

export interface WorkflowFrameData extends Record<string, unknown> {
  child: RunChild;
  /** The round that launched it (the label of a generated one). */
  round: number;
  inferred: boolean;
  width: number;
  /** The inner graph's area under the header. */
  graphHeight: number;
  /** Its graph is loaded (inner nodes are drawn over it). */
  loaded: boolean;
  error: string | null;
  board: FlowBoardSpace | null;
  /** The strip's sections opened past their latest items. */
  boardOpen: ReadonlySet<RailSection>;
  onBoardOpen: (node: string, section: RailSection, open: boolean) => void;
  messages: number;
  onLoad: (childRunId: string) => void;
  onDefinition: (child: RunChild) => void;
  onSelectMessage: (id: string) => void;
  anchor: (key: string) => React.ReactNode;
  slot: number;
}

type FrameFlowNode = Node<WorkflowFrameData, 'frame'>;

function WorkflowFrameView({ data }: NodeProps<FrameFlowNode>) {
  const { child: c, onLoad } = data;
  const state = childState(c);
  useEffect(() => {
    onLoad(c.run_id);
  }, [onLoad, c.run_id]);
  const spend = childSpend(c);
  const strip = data.board && (data.board.view || data.board.posts.length > 0);
  return (
    <div
      className="afx-frame gx-s"
      data-state={state}
      data-testid="flow-frame"
      style={{ width: data.width, ['--i' as string]: data.slot }}
    >
      {/* The run graph's under-glow: a running frame breathes like a running card. */}
      <span className="gx-glow" aria-hidden />
      {/* First, so an edge with no handle id lands here, not on a strip item. */}
      <Handle type="target" position={Position.Left} className="gx-port" />
      <Handle type="source" position={Position.Right} className="gx-port" />
      <div className="afx-frame-head" style={{ height: FRAME_HEAD }}>
        {c.generated ? (
          <span className="afx-frame-pen">
            <PenLine size={12} aria-hidden />
            written by the lead in round {data.round}
          </span>
        ) : (
          <Workflow size={12} aria-hidden className="afx-frame-icon" />
        )}
        <b className="afx-frame-name">{c.name}</b>
        {c.generated && c.handle && (
          <button type="button" className="afx-frame-btn nodrag" onClick={() => data.onDefinition(c)}>
            <FileCode2 size={12} aria-hidden />
            definition
          </button>
        )}
        <MessageCount n={data.messages} />
      </div>
      <div className="afx-frame-graph" style={{ height: data.graphHeight }}>
        {!data.loaded && (
          <span className="afx-frame-note">{data.error ? `couldn't load this run's graph: ${data.error}` : 'loading run graph…'}</span>
        )}
      </div>
      {strip && (
        <div className="afx-frame-strip nodrag">
          <BoardRail
            compact
            board={data.board!.view}
            posts={data.board!.posts}
            onSelect={data.onSelectMessage}
            anchor={data.anchor}
            limit={STRIP_LIMIT}
            open={data.boardOpen}
            onOpen={(section, open) => data.onBoardOpen(c.run_id, section, open)}
          />
        </div>
      )}
      <div className="afx-frame-foot">
        {data.inferred && <InferredMark />}
        <span className="afx-frame-who">{c.address ?? c.handle ?? c.name}</span>
        {spend && <span>{spend}</span>}
        <Link className="afx-link nodrag" to={`/runs/${encodeURIComponent(c.run_id)}`}>
          run
        </Link>
        <StatusCapsule state={state} small />
      </div>
    </div>
  );
}

export default memo(WorkflowFrameView);
