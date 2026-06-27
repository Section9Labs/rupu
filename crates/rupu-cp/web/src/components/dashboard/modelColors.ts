// Stable model → color mapping shared by the usage timeline (stacked area) and
// the model breakdown table, so a model wears the same color in both. Pure: no
// recharts / React deps, so it stays out of any chart chunk and is trivially
// unit-testable.

/** Brand-ramp + distinct-hue palette. Cycled if there are more models than colors. */
export const MODEL_PALETTE: readonly string[] = [
  '#1860f2', // brand blue
  '#22c55e', // green
  '#f59e0b', // amber
  '#a855f7', // purple
  '#ec4899', // pink
  '#06b6d4', // cyan
  '#ef4444', // red
  '#84cc16', // lime
  '#6366f1', // indigo
  '#14b8a6', // teal
];

/** Slate used for the `others (N)` rollup row / series. */
export const OTHER_COLOR = '#94a3b8';

/**
 * Assign a stable color to each model. Models are sorted first so the mapping is
 * deterministic regardless of the input order — the timeline and the table pass
 * the same model set and therefore agree on colors.
 */
export function assignModelColors(models: readonly string[]): Map<string, string> {
  const sorted = [...new Set(models)].sort();
  const map = new Map<string, string>();
  sorted.forEach((m, i) => {
    map.set(m, MODEL_PALETTE[i % MODEL_PALETTE.length]);
  });
  return map;
}

/** Display label for a model row (model, else provider, else agent, else `—`). */
export function modelLabel(row: { model: string; provider: string; agent: string }): string {
  return row.model || row.provider || row.agent || '—';
}
