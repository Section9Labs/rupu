// RoundNode — one of the lead's rounds, on the spine: the run graph's card
// with what the lead decided that round, the goals met after it, the run's
// spend as it ended, how many children it launched and the round's lead
// transcript. A run that predates the decision record shows the round
// number, counts and transcript link only. A round whose lead call failed is
// drawn failed, with the reason its event recorded.

import { memo } from 'react';
import { Link } from 'react-router-dom';
import type { Node, NodeProps } from '@xyflow/react';
import { Flag } from 'lucide-react';
import type { FlowRound } from '../../lib/flow/flowModel';
import type { StepState } from '../../lib/runGraphModel';
import { formatSpendShort } from '../../lib/agentiflow';
import { CARD_H, CARD_W } from '../../lib/flow/flowLayout';
import { NodeFrame, StatusCapsule } from '../graph/NodeFrame';
import { MessageCount } from '../graph/MessageCount';

export interface RoundNodeData extends Record<string, unknown> {
  round: FlowRound;
  /** The flow is running and this is its open round. */
  live: boolean;
  /** Children it launched, any depth. */
  launched: number;
  /** The lead's messages this round. */
  messages: number;
  slot: number;
}

type RoundFlowNode = Node<RoundNodeData, 'round'>;

/** A round's state: the open one runs, one whose lead call failed failed,
 *  any other ended one is done. */
export function roundCardState(r: Pick<FlowRound, 'failed'>, live: boolean): StepState {
  if (live) return 'running';
  return r.failed ? 'failed' : 'done';
}

function RoundNodeView({ data }: NodeProps<RoundFlowNode>) {
  const { round: r, live, launched, messages, slot } = data;
  const state: StepState = roundCardState(r, live);
  const met = r.goalsAfter?.filter((g) => g.met).length;
  return (
    <NodeFrame state={state} kind="step" slot={slot} width={CARD_W} minHeight={CARD_H} testId="flow-round">
      <div className="gx-head">
        <span className="gx-tile" title="lead round">
          <Flag size={12} strokeWidth={1.9} aria-hidden />
        </span>
        <span className="gx-name">round {r.round}</span>
        <MessageCount n={messages} />
        <StatusCapsule state={state} />
      </div>
      <div className="gx-body">
        {r.failed ? (
          <div className="gx-error afx-result" title={r.error ?? undefined} data-testid="flow-round-error">
            {r.error ?? 'the lead\'s call failed this round'}
          </div>
        ) : (
          r.decision && (
            <div className="afx-decision" title={r.decision}>
              {r.decision}
            </div>
          )
        )}
        <div className="gx-sub afx-result">
          {r.spentUsd != null && <span>{formatSpendShort(r.spentUsd)} spent · </span>}
          {r.transcriptPath ? (
            <Link
              className="afx-link nodrag"
              to={`/transcript?path=${encodeURIComponent(r.transcriptPath)}&live=${live ? 1 : 0}`}
            >
              lead transcript
            </Link>
          ) : (
            <span>no lead transcript</span>
          )}
        </div>
      </div>
      <div className="gx-foot">
        <span className="gx-kind">lead</span>
        <span className="afx-foot-r">
          {r.goalsAfter && (
            <span>
              goals {met}/{r.goalsAfter.length} ·
            </span>
          )}
          {r.turnBudgetHit && <span title="the round stopped at its turn budget">turn budget ·</span>}
          <span>{launched} launched</span>
        </span>
      </div>
    </NodeFrame>
  );
}

export default memo(RoundNodeView);
