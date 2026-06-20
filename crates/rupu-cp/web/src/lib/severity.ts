/**
 * Shared severity type + style map for the rupu CP UI.
 *
 * Used by:
 *   - CoverageDetail (finding rows / FindingRecord badges)
 *   - FindingCard (transcript finding cards)
 *
 * Only STATIC Tailwind class strings; no `bg-${x}` template expressions.
 * Colour ramp comes from the `sev.*` tokens in tailwind.config.ts:
 *   sev-critical #9333ea  → purple
 *   sev-high     #dc2626  → red
 *   sev-medium   #ea580c  → orange
 *   sev-low      #ca8a04  → yellow
 *   sev-info     #64748b  → slate
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
  /** Display label (uppercase in practice; kept lower here so callers can case-transform) */
  label: string;
  /** Combined pill class string for inline badges (bg + text + ring) */
  pill: string;
}

export const SEVERITY_STYLE: Record<Severity, SeverityStyle> = {
  critical: {
    text: 'text-sev-critical',
    bg: 'bg-purple-50',
    ring: 'ring-purple-200',
    bar: 'bg-[#9333ea]',
    label: 'critical',
    pill: 'bg-purple-50 text-sev-critical ring-purple-200',
  },
  high: {
    text: 'text-sev-high',
    bg: 'bg-red-50',
    ring: 'ring-red-200',
    bar: 'bg-[#dc2626]',
    label: 'high',
    pill: 'bg-red-50 text-sev-high ring-red-200',
  },
  medium: {
    text: 'text-sev-medium',
    bg: 'bg-orange-50',
    ring: 'ring-orange-200',
    bar: 'bg-[#ea580c]',
    label: 'medium',
    pill: 'bg-orange-50 text-sev-medium ring-orange-200',
  },
  low: {
    text: 'text-sev-low',
    bg: 'bg-yellow-50',
    ring: 'ring-yellow-200',
    bar: 'bg-[#ca8a04]',
    label: 'low',
    pill: 'bg-yellow-50 text-sev-low ring-yellow-200',
  },
  info: {
    text: 'text-sev-info',
    bg: 'bg-slate-100',
    ring: 'ring-slate-200',
    bar: 'bg-[#64748b]',
    label: 'info',
    pill: 'bg-slate-100 text-sev-info ring-slate-200',
  },
};
