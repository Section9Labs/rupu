// Stable model → color mapping shared by the usage timeline (stacked area) and
// the model breakdown table, so a model wears the same color in both. Pure: no
// recharts / React deps, so it stays out of any chart chunk and is trivially
// unit-testable.
//
// `pivotLabel` generalizes `modelLabel` to the other five `/usage` pivots
// (provider/agent/workflow/host/project) for `ModelBreakdownTable`, which is
// the one surface that genuinely receives non-model rows from
// `/api/usage?group_by=`. This palette itself stays model-only — see
// `../usage/pivotColors.ts` for the themed categorical ramp non-model pivots
// use instead.

import type { UsageBreakdownRow } from '../../lib/usage';
import type { Pivot } from '../../lib/api';

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

/**
 * Display label for a `UsageBreakdownRow`, keyed by the active pivot. Only
 * the field matching `pivot` is ever non-empty on a given row (see the doc
 * comment on `UsageBreakdownRow` in `lib/usage.ts`), so this reads exactly
 * one field per pivot rather than falling through several — falling through
 * `model || provider || agent` for a `workflow` pivot would read every row's
 * `model` field, which is `""` for that grouping, and collapse every
 * distinct workflow into the same `'—'` row.
 */
export function pivotLabel(
  row: Pick<UsageBreakdownRow, 'model' | 'provider' | 'agent' | 'workflow' | 'host_id' | 'workspace_id'>,
  pivot: Pivot,
): string {
  switch (pivot) {
    case 'model':
      return modelLabel(row);
    case 'provider':
      return row.provider || '—';
    case 'agent':
      return row.agent || '—';
    case 'workflow':
      return row.workflow || '—';
    case 'host':
      return row.host_id || '—';
    case 'project':
      return row.workspace_id || '—';
  }
}
