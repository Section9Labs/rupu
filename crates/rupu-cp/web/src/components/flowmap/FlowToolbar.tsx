// FlowToolbar — which message kinds the map shows, and whose messages; the
// round stepper (`children`) sits at its end.

import type { ReactNode } from 'react';
import { KIND_FILTERS, type KindFilter } from '../../lib/messages';

export default function FlowToolbar({
  kinds,
  participant,
  participants,
  onKinds,
  onParticipant,
  children,
}: {
  kinds: ReadonlySet<KindFilter>;
  /** `null`: everyone. */
  participant: string | null;
  participants: string[];
  onKinds: (next: Set<KindFilter>) => void;
  onParticipant: (next: string | null) => void;
  children?: ReactNode;
}) {
  return (
    <div className="afx-toolbar">
      <div className="afx-toolbar-kinds" role="group" aria-label="Message kinds">
        {KIND_FILTERS.map((k) => {
          const on = kinds.has(k.key);
          return (
            <button
              key={k.key}
              type="button"
              className="afx-pill"
              aria-pressed={on}
              onClick={() => {
                const next = new Set(kinds);
                if (on) next.delete(k.key);
                else next.add(k.key);
                onKinds(next);
              }}
            >
              {k.label}
            </button>
          );
        })}
      </div>
      <select
        className="afx-select"
        aria-label="Participant"
        value={participant ?? ''}
        onChange={(e) => onParticipant(e.target.value || null)}
      >
        <option value="">Everyone</option>
        {participants.map((p) => (
          <option key={p} value={p}>
            {p}
          </option>
        ))}
      </select>
      {children}
    </div>
  );
}
