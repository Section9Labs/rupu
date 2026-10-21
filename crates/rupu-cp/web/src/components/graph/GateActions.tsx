// GateActions — Approve / Reject on a gate node that waits on the operator.
// The run page owns the requests (RunDetail's gate handlers, the same ones its
// gate banner uses); this row shows their state. A decision is pending-state,
// not optimistic: after Approve the row says "approved · resuming…" until the
// run's own status moves the gate on. Reject asks once more before it acts.

import { useState, type MouseEvent } from 'react';
import { Check } from 'lucide-react';
import { StatusGlyph } from './NodeFrame';

export interface GateDecision {
  pending: boolean;
  decision: 'approved' | 'rejected' | null;
  error: string | null;
}

export interface GateControl {
  state(stepId: string): GateDecision;
  approve(stepId: string): void;
  reject(stepId: string): void;
}

/** Keep a click on the row from also selecting (or dragging) the node. */
function own(fn: () => void) {
  return (e: MouseEvent) => {
    e.stopPropagation();
    fn();
  };
}

export default function GateActions({ stepId, control }: { stepId: string; control: GateControl }) {
  const { pending, decision, error } = control.state(stepId);
  const [confirming, setConfirming] = useState(false);

  if (decision === 'approved') {
    return (
      <div className="gx-body gx-row gx-gate-done" data-testid="gate-resolved">
        <StatusGlyph state="running" />
        <span className="gx-sub">approved · resuming…</span>
      </div>
    );
  }
  if (decision === 'rejected') {
    return (
      <div className="gx-body gx-row" data-testid="gate-resolved">
        <span className="gx-sub">rejected</span>
      </div>
    );
  }
  if (confirming) {
    return (
      <div className="gx-body gx-row nodrag nopan">
        <span className="gx-sub gx-grow">Reject?</span>
        <button
          type="button"
          className="gx-btn gx-btn-danger"
          disabled={pending}
          onClick={own(() => {
            control.reject(stepId);
            setConfirming(false);
          })}
          aria-label={`Confirm rejection for ${stepId}`}
        >
          {pending ? 'Rejecting…' : 'Reject'}
        </button>
        <button type="button" className="gx-btn gx-btn-quiet" onClick={own(() => setConfirming(false))}>
          Keep
        </button>
      </div>
    );
  }
  return (
    <div className="gx-body gx-row nodrag nopan">
      <button
        type="button"
        className="gx-approve"
        disabled={pending}
        onClick={own(() => control.approve(stepId))}
        aria-label={`Approve ${stepId}`}
      >
        <Check size={13} strokeWidth={2.25} aria-hidden />
        {pending ? 'Approving…' : 'Approve'}
      </button>
      <button
        type="button"
        className="gx-btn gx-btn-quiet"
        disabled={pending}
        onClick={own(() => setConfirming(true))}
        aria-label={`Reject ${stepId}`}
      >
        Reject
      </button>
      {error && (
        <span className="gx-error" role="alert" title={error}>
          {error}
        </span>
      )}
    </div>
  );
}
