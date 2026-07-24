// kindBridge — the run graph's SINGLE import boundary onto the workflow
// editor's per-kind visual language (`workflow-editor/kindVisuals`). The run
// model and the editor use different kind unions: the run model emits `gate`
// where the editor's `StepKind` says `approval_gate`, and the run model has no
// `branch`. Everything else is identity. Keeping the mapping here means only
// one run-graph file reaches across into the editor's module.

import type { LucideIcon } from 'lucide-react';
import { KIND_ACCENT, KIND_ICON } from '../workflow-editor/kindVisuals';
import type { StepKind } from '../../lib/workflowGraph';
import type { ColorKey } from '../../lib/useThemeColors';
import type { StepNodeDto } from '../../lib/api';

export type RunKind = StepNodeDto['kind'];

// Total map, not a cast: if `RunKind` ever gains a member, this object
// literal fails to satisfy `Record<RunKind, StepKind>` and the build breaks
// at compile time instead of quietly handing `KIND_ACCENT` an `undefined`
// key (which paints `rgb(undefined)`).
const STEP_KIND: Record<RunKind, StepKind> = {
  step: 'step',
  for_each: 'for_each',
  parallel: 'parallel',
  panel: 'panel',
  gate: 'approval_gate',
  action: 'action',
};

/** Map a run-model step kind onto the editor's `StepKind` vocabulary. */
export function runKindToStepKind(kind: RunKind): StepKind {
  return STEP_KIND[kind];
}

/** The themed accent token for a run step's kind (same palette as the editor). */
export function runKindAccent(kind: RunKind): ColorKey {
  return KIND_ACCENT[runKindToStepKind(kind)];
}

/** The lucide icon for a run step's kind (same icons as the editor). */
export function runKindIcon(kind: RunKind): LucideIcon {
  return KIND_ICON[runKindToStepKind(kind)];
}

const LABELS: Record<RunKind, string> = {
  step: 'step',
  for_each: 'for each',
  parallel: 'parallel',
  panel: 'panel',
  gate: 'gate',
  action: 'action',
};

/** Short human label rendered in a node's kind pill. */
export function runKindLabel(kind: RunKind): string {
  return LABELS[kind];
}
