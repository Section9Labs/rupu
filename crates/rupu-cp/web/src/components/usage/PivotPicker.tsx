// PivotPicker — the attribution dimension.
//
// 'This autoflow costs $40/night' was unanswerable when group_by was
// model-only. That is the actionable question: attribution is what lets you
// change something.

import type { Pivot } from '../../lib/api';

export type { Pivot };

/** Canonical pivot order — mirrors the six `GroupBy` variants in
 *  `rupu-cp/src/usage.rs`, in the order `GroupBy::parse` accepts them. */
export const PIVOTS: Pivot[] = ['model', 'provider', 'agent', 'workflow', 'host', 'project'];

/** Human display label for a pivot dimension, for section/column titles. */
export const PIVOT_LABEL: Record<Pivot, string> = {
  model: 'Model',
  provider: 'Provider',
  agent: 'Agent',
  workflow: 'Workflow',
  host: 'Host',
  project: 'Project',
};

export function PivotPicker({ value, onChange }: { value: Pivot; onChange: (p: Pivot) => void }) {
  return (
    <div className="flex rounded-md border border-[rgb(var(--c-border))]">
      {PIVOTS.map((p) => (
        <button
          key={p}
          type="button"
          onClick={() => onChange(p)}
          className={`px-2 py-1 text-xs capitalize ${
            value === p
              ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
              : 'text-[rgb(var(--c-ink-mute))]'
          }`}
        >
          {p}
        </button>
      ))}
    </div>
  );
}
