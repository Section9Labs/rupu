// kindVisuals — the SINGLE shared source for "what does a StepKind look
// like": an accent color token and a lucide icon. Both classic and the
// `next` (instrument) look consume `KIND_ACCENT` (previously duplicated as
// EditableStepNode.KIND_KEY + NodePalette.KIND_ACCENT — a byte-identical
// refactor, values unchanged). `KIND_ICON` is `next`-only (classic markup
// never renders an icon).
//
// Per-kind accent → a THEMED palette token: step/blue (running), for_each/
// violet (brand), parallel/purple (sev-critical), panel/amber (awaiting),
// branch/green (done — a routing decision, distinct from every other kind).

import { Bot, Columns3, GitBranch, Merge, Repeat, ShieldCheck, Split, UserCheck, Zap, type LucideIcon } from 'lucide-react';
import type { StepKind } from '../../lib/workflowGraph';
import type { ColorKey } from '../../lib/useThemeColors';
import type { ShapeName } from './nodeShapes';

export const KIND_ACCENT: Record<StepKind, ColorKey> = {
  step: 'status.running',
  for_each: 'brand.500',
  parallel: 'sev.critical',
  panel: 'status.awaiting',
  branch: 'status.done',
  // approval_gate/paused (a human hold) + action/sev.info (a connector call).
  approval_gate: 'status.paused',
  action: 'sev.info',
  // split/join (Phase 1 non-linear orchestration nodes) — placeholder brand-
  // ramp tokens distinct from every other kind. Task 5-7 (the graph-mode
  // renderer arc) may retune these; nothing downstream depends on the exact
  // value yet.
  split: 'brand.600',
  join: 'brand.700',
};

export const KIND_ICON: Record<StepKind, LucideIcon> = {
  step: Bot,
  for_each: Repeat,
  parallel: Columns3,
  panel: ShieldCheck,
  branch: GitBranch,
  approval_gate: UserCheck,
  action: Zap,
  split: Split,
  join: Merge,
};

/** Which flowchart symbol each kind paints (see nodeShapes.ts). `parallel` and
 *  `panel` keep a rectangular body deliberately — they are the only kinds whose
 *  height grows with content, so the subroutine/stacked idioms (which grow)
 *  are the right flowchart forms rather than a fixed silhouette. `split`/`join`
 *  are placeholder `rect` (no dedicated fan-out/barrier silhouette exists yet
 *  in nodeShapes.ts) — a real symbol is Task 5-7's call, out of scope here. */
export const KIND_SHAPE: Record<StepKind, ShapeName> = {
  step: 'rect',
  for_each: 'hexagon',
  parallel: 'subroutine',
  panel: 'stacked',
  branch: 'vhex',
  approval_gate: 'trapezoid',
  action: 'parallelogram',
  split: 'rect',
  join: 'rect',
};
