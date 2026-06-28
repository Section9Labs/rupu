// useThemeColors — the unifying bridge between the tokenized CSS palette and the
// INLINE / runtime consumers that can't go through Tailwind classes: the xyflow
// graph canvas, the recharts charts, and the imperatively-mounted CodeMirror
// editors. Those paint with raw color strings (`style={{…}}`, SVG attrs, canvas),
// so they need ready-to-use `rgb(…)` strings for the CURRENT theme.
//
// The palette lives once in `src/styles.css` (`:root` = light,
// `[data-theme="dark"]` = dark) as space-separated RGB *channels* (e.g.
// `--c-ink: 15 23 42`). This hook reads those channels off
// `getComputedStyle(document.documentElement)` and wraps them as `rgb(…)`. It
// recomputes whenever the resolved theme `mode` flips (ThemeProvider applies the
// `data-theme` attribute synchronously during render, so the computed values are
// already fresh when this memo runs).

import { useContext, useMemo } from 'react';
import { ThemeContext } from '../components/theme/ThemeProvider';

// ── token registry ────────────────────────────────────────────────────────────
// Dotted keys map to the CSS variable that carries their RGB channels. The keys
// double as the `get(key)` / `alpha(key, a)` argument type, so inline consumers
// can reach any token (e.g. `colors.alpha('status.running', 0.06)`).
const VARS = {
  bg: '--c-bg',
  panel: '--c-panel',
  surface: '--c-surface',
  surfaceHover: '--c-surface-hover',
  border: '--c-border',
  ink: '--c-ink',
  inkDim: '--c-ink-dim',
  inkMute: '--c-ink-mute',
  'brand.500': '--c-brand-500',
  'brand.600': '--c-brand-600',
  'brand.700': '--c-brand-700',
  'status.running': '--c-status-running',
  'status.done': '--c-status-done',
  'status.completed': '--c-status-completed',
  'status.failed': '--c-status-failed',
  'status.awaiting': '--c-status-awaiting',
  'status.pending': '--c-status-pending',
  'status.skipped': '--c-status-skipped',
  'status.cancelled': '--c-status-cancelled',
  'status.rejected': '--c-status-rejected',
  'sev.critical': '--c-sev-critical',
  'sev.high': '--c-sev-high',
  'sev.medium': '--c-sev-medium',
  'sev.low': '--c-sev-low',
  'sev.info': '--c-sev-info',
} as const;

export type ColorKey = keyof typeof VARS;

export interface ThemeColors {
  bg: string;
  panel: string;
  surface: string;
  surfaceHover: string;
  border: string;
  ink: string;
  inkDim: string;
  inkMute: string;
  brand: { 500: string; 600: string; 700: string };
  status: {
    running: string;
    done: string;
    completed: string;
    failed: string;
    awaiting: string;
    pending: string;
    skipped: string;
    cancelled: string;
    rejected: string;
  };
  sev: { critical: string; high: string; medium: string; low: string; info: string };
  /** Resolved `rgb(…)` for any token key. */
  get: (key: ColorKey) => string;
  /** `rgb(… / a)` for any token key — for soft tints / translucent strokes. */
  alpha: (key: ColorKey, a: number) => string;
}

/** Read the raw space-separated RGB channels for a CSS variable (e.g. `"15 23 42"`). */
function readChannels(name: string): string {
  try {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || '0 0 0';
  } catch {
    return '0 0 0';
  }
}

/**
 * Snapshot every token for the CURRENTLY-applied theme. Pure (no React) so it is
 * trivially unit-testable: set the CSS vars + `data-theme` on the document and
 * call it directly.
 */
export function readThemeColors(): ThemeColors {
  const channels = {} as Record<ColorKey, string>;
  for (const key of Object.keys(VARS) as ColorKey[]) {
    channels[key] = readChannels(VARS[key]);
  }

  const get = (key: ColorKey): string => `rgb(${channels[key]})`;
  const alpha = (key: ColorKey, a: number): string => `rgb(${channels[key]} / ${a})`;

  return {
    bg: get('bg'),
    panel: get('panel'),
    surface: get('surface'),
    surfaceHover: get('surfaceHover'),
    border: get('border'),
    ink: get('ink'),
    inkDim: get('inkDim'),
    inkMute: get('inkMute'),
    brand: { 500: get('brand.500'), 600: get('brand.600'), 700: get('brand.700') },
    status: {
      running: get('status.running'),
      done: get('status.done'),
      completed: get('status.completed'),
      failed: get('status.failed'),
      awaiting: get('status.awaiting'),
      pending: get('status.pending'),
      skipped: get('status.skipped'),
      cancelled: get('status.cancelled'),
      rejected: get('status.rejected'),
    },
    sev: {
      critical: get('sev.critical'),
      high: get('sev.high'),
      medium: get('sev.medium'),
      low: get('sev.low'),
      info: get('sev.info'),
    },
    get,
    alpha,
  };
}

/**
 * Hook form: returns the themed color strings for the current theme, recomputing
 * when `mode` flips. Memoized per mode so inline consumers get a stable object
 * between toggles.
 */
export function useThemeColors(): ThemeColors {
  // Read the theme from context when a provider is present (the app path), so the
  // memo recomputes on toggle. When rendered WITHOUT a provider (isolated tests /
  // detached previews) fall back to the live `data-theme` attribute instead of
  // throwing — leaf graph/chart/editor components must stay provider-optional.
  const ctx = useContext(ThemeContext);
  const mode =
    ctx?.mode ??
    (typeof document !== 'undefined' && document.documentElement.dataset.theme === 'dark'
      ? 'dark'
      : 'light');
  // `mode` is the dependency: ThemeProvider applies `data-theme` synchronously
  // during render before children render, so the computed styles read here are
  // already the new theme's values.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  return useMemo(() => readThemeColors(), [mode]);
}

export default useThemeColors;
