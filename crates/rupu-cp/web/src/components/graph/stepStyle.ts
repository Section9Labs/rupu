// Shared per-state visual vocabulary for the run-graph node components.
//
// Colors come from the unified status descriptor map (`lib/status.ts`) so the
// graph now matches the pills/timeline exactly (running blue-500, done
// green-500, failed red-500, awaiting amber-500). Only the glyph + lowercase
// label are graph-local. The `done`↔`completed` alias is handled by
// `stepStateStyle`.
//
// Colors are JS values consumed via inline `style={{ color, background }}`
// (NOT Tailwind class interpolation), so dynamic state coloring stays static
// at the Tailwind level. The `StepState` union here is the runGraphModel one
// (`done`, not `completed`) — distinct from StatusPill's StepState.

import type { StepState } from '../../lib/runGraphModel';
import { stepStateStyle } from '../../lib/status';

export interface StateStyle {
  /** Foreground / glyph-fill color. */
  color: string;
  /** Soft background tint for chips / cards. */
  bg: string;
  /** Single-char status glyph (shape + color → readable without color). */
  glyph: string;
  /** Short human label. */
  label: string;
}

export const STATE_STYLE: Record<StepState, StateStyle> = {
  running: { color: stepStateStyle('running').hex, bg: stepStateStyle('running').tint, glyph: '⟳', label: 'running' },
  done: { color: stepStateStyle('done').hex, bg: stepStateStyle('done').tint, glyph: '✓', label: 'done' },
  failed: { color: stepStateStyle('failed').hex, bg: stepStateStyle('failed').tint, glyph: '✕', label: 'failed' },
  awaiting_approval: { color: stepStateStyle('awaiting_approval').hex, bg: stepStateStyle('awaiting_approval').tint, glyph: '⏸', label: 'awaiting' },
  // pending keeps the slate-400 glyph color (text reads on the light card).
  pending: { color: stepStateStyle('pending').hex, bg: stepStateStyle('pending').tint, glyph: '•', label: 'pending' },
  skipped: { color: stepStateStyle('pending').hex, bg: stepStateStyle('skipped').tint, glyph: '⤼', label: 'skipped' },
};

/** Fill color used for the colored unit squares in fan-out grids. */
export function glyphBg(state: StepState): string {
  // Pending squares read as a muted fill (slate-300 = `status.skipped` token)
  // rather than the dim slate-400 glyph color, matching the mockup's
  // `.glyph-pend`.
  return state === 'pending' ? stepStateStyle('skipped').hex : STATE_STYLE[state].color;
}
