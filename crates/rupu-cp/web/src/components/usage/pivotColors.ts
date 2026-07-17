// pivotColors — categorical color ramp for NON-model pivots (provider / agent
// / workflow / host / project).
//
// `modelColors.ts`'s MODEL_PALETTE is model IDENTITY, not an arbitrary
// category assignment — deliberately kept separate and untouched for the
// model pivot. Every other pivot has no identity of its own, so its colors
// come from the THEMED token set via `useThemeColors()` (never a hardcoded
// hex literal), reading correctly in both light and dark.

import type { ThemeColors } from '../../lib/useThemeColors';

/** Ten visually-distinct themed tones, reusing existing semantic tokens
 *  rather than inventing new ones. Order is arbitrary but fixed, so the
 *  mapping in `assignCategoricalColors` is deterministic. */
export function categoricalRamp(theme: ThemeColors): string[] {
  return [
    theme.brand[500],
    theme.status.running,
    theme.sev.high,
    theme.info,
    theme.status.awaiting,
    theme.brand[700],
    theme.sev.medium,
    theme.status.paused,
    theme.sev.low,
    theme.status.completed,
  ];
}

/**
 * Assign a stable color to each key. Keys are sorted first so the mapping is
 * deterministic regardless of input order — mirrors `assignModelColors`'s
 * contract in `modelColors.ts`.
 */
export function assignCategoricalColors(
  keys: readonly string[],
  theme: ThemeColors,
): Map<string, string> {
  const sorted = [...new Set(keys)].sort();
  const ramp = categoricalRamp(theme);
  const map = new Map<string, string>();
  sorted.forEach((k, i) => {
    map.set(k, ramp[i % ramp.length]);
  });
  return map;
}
