// MoreNode — the card that stands for children not drawn: a round's "+N
// more" (the run graph's for_each overflow, in place) or a collapsed round's
// one summary card ("23 children · 21 done, 2 failed"). Either expands its
// round.

import { memo } from 'react';
import type { Node, NodeProps } from '@xyflow/react';
import { ChevronsDown, Layers } from 'lucide-react';
import type { RunChild } from '../../lib/api';
import { CARD_W } from '../../lib/flow/flowLayout';
import { NodeFrame } from '../graph/NodeFrame';
import { childState } from './ChildNode';

export interface MoreNodeData extends Record<string, unknown> {
  /** `more`: "+N more"; `summary`: a collapsed round. */
  variant: 'more' | 'summary';
  round: number;
  /** The children it stands for. */
  children: RunChild[];
  onExpand: (round: number) => void;
  slot: number;
}

type MoreFlowNode = Node<MoreNodeData, 'more' | 'summary'>;

/** `21 done, 2 failed` over `children`, busiest first. */
export function stateCounts(children: RunChild[]): string {
  const counts = new Map<string, number>();
  for (const c of children) {
    const s = childState(c);
    counts.set(s, (counts.get(s) ?? 0) + 1);
  }
  const order = ['running', 'failed', 'done', 'paused', 'pending', 'skipped'];
  return order
    .filter((s) => counts.has(s))
    .map((s) => `${counts.get(s)} ${s === 'skipped' ? 'abandoned' : s === 'paused' ? 'interrupted' : s}`)
    .join(', ');
}

function MoreNodeView({ data }: NodeProps<MoreFlowNode>) {
  const { variant, round, children, onExpand, slot } = data;
  const n = children.length;
  const label =
    variant === 'more' ? `+${n} more` : `${n} ${n === 1 ? 'child' : 'children'}${n ? ` · ${stateCounts(children)}` : ''}`;
  const Icon = variant === 'more' ? ChevronsDown : Layers;
  return (
    <NodeFrame state="pending" kind="for_each" slot={slot} width={CARD_W} minHeight={0} testId={`flow-${variant}`}>
      <button
        type="button"
        className="afx-more nodrag"
        onClick={() => onExpand(round)}
        aria-label={variant === 'more' ? `Show ${n} more children of round ${round}` : `Expand round ${round}`}
      >
        <span className="gx-tile">
          <Icon size={12} strokeWidth={1.9} aria-hidden />
        </span>
        <span className="afx-more-label">{label}</span>
      </button>
      {variant === 'more' && n > 0 && <div className="gx-sub">{stateCounts(children)}</div>}
    </NodeFrame>
  );
}

export default memo(MoreNodeView);
