// PivotPicker — the attribution dimension.
//
// 'This autoflow costs $40/night' was unanswerable when group_by was
// model-only. That is the actionable question: attribution is what lets you
// change something.
//
// Reimplemented internally on `ui/Segmented` (One Control Language kit) —
// props and visual result unchanged; it was previously its own bespoke
// boxed-tab dialect (and one of the `[rgb(var(--c-*))]` literal-class spots
// slated for a token-utility cleanup in Phase 3).

import type { Pivot } from '../../lib/api';
import { Segmented } from '../ui/Segmented';

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
    <Segmented
      ariaLabel="Pivot"
      size="sm"
      // Raw lowercase value as the label — Segmented's `capitalize` class
      // renders it Title-Case visually (parity with the pre-kit markup:
      // `{p}` + a `capitalize` Tailwind class) while keeping the accessible
      // name/DOM text lowercase, which existing consumer tests assert on.
      options={PIVOTS.map((p) => ({ value: p, label: p }))}
      value={value}
      onChange={(v) => onChange(v as Pivot)}
    />
  );
}
