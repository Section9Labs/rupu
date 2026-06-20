// Shared per-state visual vocabulary for the run-graph node components.
//
// One source of truth for the colors + glyph + label of each `StepState`,
// matching the approved graph-pro / fanout-loop mockups exactly:
//   running  #1860f2   done #2ac769   failed #fb4e4e
//   awaiting #f59e0b   pending #cbd5e1
//
// Colors are JS values consumed via inline `style={{ color, background }}`
// (NOT Tailwind class interpolation), so dynamic state coloring stays static
// at the Tailwind level. The `StepState` union here is the runGraphModel one
// (`done`, not `completed`) — distinct from StatusPill's StepState, hence a
// dedicated map rather than reusing STEP_STATE_STYLES.

import type { StepState } from '../../lib/runGraphModel';

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
  running: { color: '#1860f2', bg: '#eff6ff', glyph: '⟳', label: 'running' },
  done: { color: '#2ac769', bg: '#ecfdf5', glyph: '✓', label: 'done' },
  failed: { color: '#fb4e4e', bg: '#fef2f2', glyph: '✕', label: 'failed' },
  awaiting_approval: { color: '#f59e0b', bg: '#fffbeb', glyph: '⏸', label: 'awaiting' },
  pending: { color: '#94a3b8', bg: '#f8fafc', glyph: '•', label: 'pending' },
  skipped: { color: '#94a3b8', bg: '#f1f5f9', glyph: '⤼', label: 'skipped' },
};

/** Fill color used for the colored unit squares in fan-out grids. */
export function glyphBg(state: StepState): string {
  // Pending squares read as a muted fill (#cbd5e1) rather than the dim text
  // color, matching the mockup's `.glyph-pend`.
  return state === 'pending' ? '#cbd5e1' : STATE_STYLE[state].color;
}
