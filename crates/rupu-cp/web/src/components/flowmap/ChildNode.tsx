// ChildNode — a child an agentiflow round launched (an agent, a sub-agent at
// any depth), drawn as the run graph's card (NodeFrame, its head / status
// capsule / foot): handle · message count · status, the agent's identity,
// its one-line result (or error), and tokens · duration · transcript in the
// foot. A child placed by its start time, for want of a recorded round, is
// marked `inferred`.

import { memo } from 'react';
import { Link } from 'react-router-dom';
import type { Node, NodeProps } from '@xyflow/react';
import { Bot, CircleDashed, CornerDownRight, ScrollText, Workflow, type LucideIcon } from 'lucide-react';
import type { ChildKind, RunChild } from '../../lib/api';
import type { StepState } from '../../lib/runGraphModel';
import { formatDuration } from '../../lib/duration';
import { formatTokens } from '../../lib/usage';
import { CARD_H, CARD_W } from '../../lib/flow/flowLayout';
import { AgentIdentity } from '../codename/AgentIdentity';
import { NodeFrame, StatusCapsule } from '../graph/NodeFrame';
import { MessageCount } from '../graph/MessageCount';
import type { RunKind } from '../graph/kindBridge';

/** A child kind as the run graph paints it: the accent of the run kind it
 *  is closest to, its own icon and the word its foot shows. */
export const CHILD_KIND: Record<ChildKind, { run: RunKind; Icon: LucideIcon; label: string }> = {
  agent: { run: 'step', Icon: Bot, label: 'agent' },
  subagent: { run: 'run', Icon: CornerDownRight, label: 'sub-agent' },
  workflow: { run: 'for_each', Icon: Workflow, label: 'workflow' },
};

/** A child's status as a run-graph state (a `done` child that did not
 *  succeed failed; an abandoned one was skipped; an interrupted one waits to
 *  be continued). */
export function childState(c: Pick<RunChild, 'status' | 'success'>): StepState {
  switch (c.status) {
    case 'running':
      return 'running';
    case 'failed':
      return 'failed';
    case 'done':
      return c.success === false ? 'failed' : 'done';
    case 'abandoned':
      return 'skipped';
    case 'interrupted':
      return 'paused';
    default:
      return 'pending';
  }
}

const clock = (iso: string | null) => {
  const t = iso ? Date.parse(iso) : NaN;
  return Number.isFinite(t) ? new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : null;
};

/** `410k → 10k · 17m`, as much of it as is known. */
export function childSpend(c: Pick<RunChild, 'tokens' | 'duration_ms'>): string {
  const parts: string[] = [];
  if (c.tokens) parts.push(`${formatTokens(c.tokens.input)} → ${formatTokens(c.tokens.output)}`);
  if (c.duration_ms != null) parts.push(formatDuration(c.duration_ms));
  return parts.join(' · ');
}

export { MessageCount } from '../graph/MessageCount';

export function InferredMark() {
  return (
    <span
      className="afx-inferred"
      data-testid="flow-inferred"
      title="inferred: placed by its start time, as this run did not record the round that launched it"
      aria-label="inferred placement"
    >
      <CircleDashed size={12} aria-hidden />
    </span>
  );
}

/** A transcript link as an icon (a card's foot has little room). */
export function TranscriptLink({ path, live, label = 'transcript' }: { path: string; live: boolean; label?: string }) {
  return (
    <Link
      className="afx-link afx-icon-link nodrag"
      to={`/transcript?path=${encodeURIComponent(path)}&live=${live ? 1 : 0}`}
      title={label}
      aria-label={label}
    >
      <ScrollText size={12} aria-hidden />
    </Link>
  );
}

export interface ChildNodeData extends Record<string, unknown> {
  child: RunChild;
  inferred: boolean;
  /** Messages it sent or received. */
  messages: number;
  slot: number;
}

type ChildFlowNode = Node<ChildNodeData, 'child'>;

function ChildNodeView({ data }: NodeProps<ChildFlowNode>) {
  const { child: c, inferred, messages, slot } = data;
  const state = childState(c);
  const kind = CHILD_KIND[c.kind];
  const started = clock(c.started_at);
  const sub = c.error ? (
    <div className="gx-error afx-result" title={c.error}>
      {c.error}
    </div>
  ) : c.output_excerpt ? (
    <div className="gx-sub afx-result" title={c.output_excerpt}>
      {c.output_excerpt}
    </div>
  ) : state === 'running' && started ? (
    <div className="gx-sub afx-result">started {started}</div>
  ) : null;
  const spend = childSpend(c);
  return (
    <NodeFrame state={state} kind={kind.run} slot={slot} width={CARD_W} minHeight={CARD_H} testId="flow-child">
      <div className="gx-head">
        <span className="gx-tile" title={kind.label}>
          <kind.Icon size={12} strokeWidth={1.9} aria-hidden />
        </span>
        <span className="gx-name" title={c.codename ?? c.name}>
          {c.address ?? c.handle ?? c.name}
        </span>
        {inferred && <InferredMark />}
        <MessageCount n={messages} />
        <StatusCapsule state={state} />
      </div>
      <div className="gx-body">
        <AgentIdentity className="gx-ident" codename={c.codename} agent={c.name} />
        {sub}
      </div>
      <div className="gx-foot">
        <span className="gx-kind">{kind.label}</span>
        <span className="afx-foot-r">
          {spend && <span>{spend}</span>}
          {c.transcript_path && <TranscriptLink path={c.transcript_path} live={state === 'running'} />}
        </span>
      </div>
    </NodeFrame>
  );
}

export default memo(ChildNodeView);
