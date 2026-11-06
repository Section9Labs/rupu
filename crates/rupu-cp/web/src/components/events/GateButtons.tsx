// Approve / Reject on an event that waits on the operator — the gate row and
// its Needs-you card. Same contract as the graph's GateActions: the page owns
// the requests; this shows their state. A decision is pending-state, not
// optimistic: after Approve it says "approved · resuming…" until the run's
// own events move the gate on. Reject asks once more before it acts.

import { useState, type MouseEvent, type ReactNode } from 'react';
import { Check } from 'lucide-react';
import type { StreamCard } from '../../lib/situationRoom/cards';
import type { GateDecision } from '../graph/GateActions';
import { EvGlyph } from './parts';

export interface EventGate {
  state(card: StreamCard): GateDecision;
  approve(card: StreamCard): void;
  reject(card: StreamCard): void;
}

export const IDLE_GATE: GateDecision = { pending: false, decision: null, error: null };

function own(fn: () => void) {
  return (e: MouseEvent) => {
    e.stopPropagation();
    fn();
  };
}

export default function GateButtons({ card, gate, compact }: { card: StreamCard; gate: EventGate; compact?: boolean }) {
  const { pending, decision, error } = gate.state(card);
  const [confirming, setConfirming] = useState(false);
  const label = card.stepId ?? 'this gate';
  const wrap = (children: ReactNode) =>
    compact ? <span className="evx-acts">{children}</span> : <div className="evx-gate">{children}</div>;

  if (decision === 'approved') {
    return wrap(
      <>
        <span className="evx-resolved" data-testid="gate-resolved">
          <EvGlyph mark="running" />
          approved · resuming…
        </span>
      </>,
    );
  }
  if (decision === 'rejected') {
    return wrap(
      <>
        <span className="evx-resolved" data-testid="gate-resolved">
          <EvGlyph mark="failed" />
          rejected
        </span>
      </>,
    );
  }
  if (confirming) {
    return wrap(
      <>
        {!compact && <span className="evx-resolved">Reject {label}?</span>}
        <button
          type="button"
          className="gx-btn gx-btn-danger"
          disabled={pending}
          onClick={own(() => {
            gate.reject(card);
            setConfirming(false);
          })}
          aria-label={`Confirm rejection for ${label}`}
        >
          {pending ? 'Rejecting…' : 'Reject'}
        </button>
        <button type="button" className="gx-btn gx-btn-quiet" onClick={own(() => setConfirming(false))}>
          Keep
        </button>
      </>,
    );
  }
  return wrap(
    <>
      <button type="button" className="gx-approve" disabled={pending} onClick={own(() => gate.approve(card))} aria-label="Approve">
        <Check size={compact ? 12 : 13} strokeWidth={2.25} aria-hidden />
        {pending ? 'Approving…' : 'Approve'}
      </button>
      <button type="button" className="gx-btn gx-btn-quiet" disabled={pending} onClick={own(() => setConfirming(true))} aria-label="Reject">
        Reject
      </button>
      {error && (
        <span className="evx-gate-error" role="alert" title={error}>
          {error}
        </span>
      )}
    </>,
  );
}
