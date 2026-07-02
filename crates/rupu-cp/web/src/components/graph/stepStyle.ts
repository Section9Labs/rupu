// Shared per-state visual vocabulary for the run-graph node components.
//
// The graph paints with INLINE `style={{ color, background }}` (dynamic state
// coloring can't go through Tailwind class interpolation), so the colors must be
// resolved as raw strings at render. They come from the THEMED palette via
// `useThemeColors()` — so the graph matches the pills/timeline AND flips
// light↔dark (no white cards / illegible glyphs on near-black). Only the glyph +
// lowercase label are graph-local and theme-independent.
//
// The `StepState` union here is the runGraphModel one (`done`, not `completed`).

import type { StepState } from '../../lib/runGraphModel';
import type { ColorKey, ThemeColors } from '../../lib/useThemeColors';

export interface StateStyle {
  /** Foreground / glyph-fill color (themed). */
  color: string;
  /** Soft background tint for chips / cards (themed, translucent). */
  bg: string;
  /** Single-char status glyph (shape + color → readable without color). */
  glyph: string;
  /** Short human label. */
  label: string;
}

/** Graph-local glyph + label per state (theme-independent). */
const GLYPH_LABEL: Record<StepState, { glyph: string; label: string }> = {
  running: { glyph: '⟳', label: 'running' },
  done: { glyph: '✓', label: 'done' },
  failed: { glyph: '✕', label: 'failed' },
  awaiting_approval: { glyph: '⏸', label: 'awaiting' },
  // Distinct glyph from `awaiting_approval` (⏸ vs ❚❚) — a paused step is a
  // deliberate operator pause mid-run, not a gate waiting on a decision.
  paused: { glyph: '❚❚', label: 'paused' },
  pending: { glyph: '•', label: 'pending' },
  skipped: { glyph: '⤼', label: 'skipped' },
};

/** Map a graph step-state to its themed status color-token key. */
const STATE_KEY: Record<StepState, ColorKey> = {
  running: 'status.running',
  done: 'status.done',
  failed: 'status.failed',
  awaiting_approval: 'status.awaiting',
  paused: 'status.paused',
  pending: 'status.pending',
  skipped: 'status.skipped',
};

/** Resolve the full themed style for a state from the current palette. */
export function stateStyle(c: ThemeColors, state: StepState): StateStyle {
  const key = STATE_KEY[state];
  return {
    color: c.get(key),
    bg: c.alpha(key, 0.12),
    glyph: GLYPH_LABEL[state].glyph,
    label: GLYPH_LABEL[state].label,
  };
}

/** Fill color used for the colored unit squares in fan-out grids. Pending
 *  squares read as a muted fill (the `skipped` slate token) rather than the dim
 *  pending glyph color, matching the mockup's `.glyph-pend`. */
export function glyphBg(c: ThemeColors, state: StepState): string {
  return state === 'pending' ? c.get('status.skipped') : c.get(STATE_KEY[state]);
}
