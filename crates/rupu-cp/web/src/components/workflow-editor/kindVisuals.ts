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
  // split/join (Phase 1 non-linear orchestration nodes) — the brand ramp's
  // two darkest steps, distinct from for_each's brand.500 and from every
  // other kind's accent.
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
 *  are the right flowchart forms rather than a fixed silhouette. `split`/
 *  `join` get their own fan-out/fan-in silhouettes (Task 6) — deliberate
 *  placeholders (recognizable and geometrically correct, not final art); the
 *  operator may refine them with dedicated shape options later. */
export const KIND_SHAPE: Record<StepKind, ShapeName> = {
  step: 'rect',
  for_each: 'hexagon',
  parallel: 'subroutine',
  panel: 'stacked',
  branch: 'vhex',
  approval_gate: 'trapezoid',
  action: 'parallelogram',
  split: 'fanout',
  join: 'fanin',
};

/** Which palette family a kind belongs to — drives the NodePalette rail's
 *  "Work" / "Orchestration" subheadings (Task 6). `work` kinds carry their own
 *  agent/action work; `orchestration` kinds route/gate/fan the run without
 *  doing any of their own (branch/split/join/approval_gate — see workflow.rs's
 *  `is_orch` check, which `split`/`join` already mirror in workflowGraph.ts). */
export const KIND_FAMILY: Record<StepKind, 'work' | 'orchestration'> = {
  step: 'work',
  for_each: 'work',
  parallel: 'work',
  panel: 'work',
  action: 'work',
  branch: 'orchestration',
  split: 'orchestration',
  join: 'orchestration',
  approval_gate: 'orchestration',
};
