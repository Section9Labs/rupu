/**
 * Shared severity type + style map for the rupu CP UI.
 *
 * Used by:
 *   - CoverageDetail (finding rows / FindingRecord badges)
 *   - FindingCard (transcript finding cards)
 *
 * Only STATIC Tailwind class strings; no `bg-${x}` template expressions.
 * Colour ramp + soft backgrounds come from the THEMED `sev.*` / `sev.*-bg`
 * tokens in tailwind.config.ts (which flip light↔dark via `data-theme`):
 *   sev-critical → purple   · sev-critical-bg → soft purple tint
 *   sev-high     → red      · sev-high-bg     → soft red tint
 *   sev-medium   → orange   · sev-medium-bg   → soft orange tint
 *   sev-low      → yellow   · sev-low-bg      → soft yellow tint
 *   sev-info     → slate    · sev-info-bg     → soft slate tint
 *
 * Backgrounds, text, ring, and the hairline bar all use sev tokens, so the
 * whole ramp themes automatically (no white-on-dark / light tints on dark).
 */

export type Severity = 'info' | 'low' | 'medium' | 'high' | 'critical';

export interface SeverityStyle {
  /** Tailwind text colour class from the sev.* ramp */
  text: string;
  /** Light tinted background for card / badge backgrounds */
  bg: string;
  /** Ring class for badge borders */
  ring: string;
  /** Solid background for the 1-px hairline bar at the card top */
  bar: string;
  /** Border-colour counterpart of `bar` — for the CodeViewer's left-edge
   *  severity band (`border-l-2` + this class) and other border accents. */
  barBorder: string;
  /** Display label (uppercase in practice; kept lower here so callers can case-transform) */
  label: string;
  /** Combined pill class string for inline badges (bg + text + ring) */
  pill: string;
}

export const SEVERITY_STYLE: Record<Severity, SeverityStyle> = {
  critical: {
    text: 'text-sev-critical',
    bg: 'bg-sev-critical-bg',
    ring: 'ring-sev-critical/30',
    bar: 'bg-sev-critical',
    barBorder: 'border-sev-critical',
    label: 'critical',
    pill: 'bg-sev-critical-bg text-sev-critical ring-sev-critical/30',
  },
  high: {
    text: 'text-sev-high',
    bg: 'bg-sev-high-bg',
    ring: 'ring-sev-high/30',
    bar: 'bg-sev-high',
    barBorder: 'border-sev-high',
    label: 'high',
    pill: 'bg-sev-high-bg text-sev-high ring-sev-high/30',
  },
  medium: {
    text: 'text-sev-medium',
    bg: 'bg-sev-medium-bg',
    ring: 'ring-sev-medium/30',
    bar: 'bg-sev-medium',
    barBorder: 'border-sev-medium',
    label: 'medium',
    pill: 'bg-sev-medium-bg text-sev-medium ring-sev-medium/30',
  },
  low: {
    text: 'text-sev-low',
    bg: 'bg-sev-low-bg',
    ring: 'ring-sev-low/30',
    bar: 'bg-sev-low',
    barBorder: 'border-sev-low',
    label: 'low',
    pill: 'bg-sev-low-bg text-sev-low ring-sev-low/30',
  },
  info: {
    text: 'text-sev-info',
    bg: 'bg-sev-info-bg',
    ring: 'ring-sev-info/30',
    bar: 'bg-sev-info',
    barBorder: 'border-sev-info',
    label: 'info',
    pill: 'bg-sev-info-bg text-sev-info ring-sev-info/30',
  },
};

/** Severity ordering for "worst first" sorts (e.g. picking the dominant
 *  severity when multiple findings stack on one source line). Higher = more
 *  severe. */
export function severityRank(sev: Severity): number {
  return { critical: 4, high: 3, medium: 2, low: 1, info: 0 }[sev] ?? 0;
}
