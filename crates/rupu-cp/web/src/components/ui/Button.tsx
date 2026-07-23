// Shared button primitive for the rupu CP UI. All native button props pass
// through. Use `className` for layout extras (gaps, shrink-0, width) —
// twMerge in `cn` resolves any padding/colour conflicts so callers can still
// override per-button.
//
// `ring` / `ring-danger` / `link` are the row-action idioms that were
// copy-pasted per-file (the Archive/Delete pill in WorkflowRuns.tsx et al.):
// a compact ring-bordered pill, its error-toned twin, and a bare text link.
// They own their own compact shape (rounded + padding + text size) rather
// than the shared `rounded-md` + `size`-driven padding the other variants
// use — that shape IS the point of the idiom and doesn't scale with `size`.

import { type ButtonHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export type ButtonVariant =
  | 'primary'
  | 'secondary'
  | 'ghost'
  | 'danger'
  | 'danger-outline'
  | 'ring'
  | 'ring-danger'
  | 'link';
export type ButtonSize = 'sm' | 'md';

const VARIANT_CLS: Record<ButtonVariant, string> = {
  primary: 'bg-brand-600 hover:bg-brand-700 text-white',
  secondary: 'border border-border bg-panel hover:bg-bg text-ink',
  ghost: 'hover:bg-bg text-ink-dim',
  // Destructive: filled for the committed action, outline for the lighter one
  // (Cancel / Reject buttons that sit next to a primary).
  danger: 'bg-err hover:bg-err text-white',
  'danger-outline': 'border border-err/30 bg-panel text-err hover:bg-err-bg',
  // Compact per-row action pill (Archive / Restore / …).
  ring: 'rounded px-2 py-0.5 text-note ring-1 ring-border bg-panel text-ink-dim hover:bg-surface-hover hover:text-ink',
  // Same idiom, error tones (Delete / Reject).
  'ring-danger':
    'rounded px-2 py-0.5 text-note ring-1 ring-err/30 bg-err-bg text-err hover:bg-err-bg/70',
  // Bare text link — no chrome, brand-colored, underlines on hover.
  link: 'p-0 text-ui font-medium text-brand-600 hover:text-brand-700 hover:underline',
};

// Variants that own their complete shape (rounding/padding/text-size) and
// must NOT receive the shared `rounded-md` wrapper class or the `size`-keyed
// padding below — those would twMerge-override the compact idiom's own
// classes since they're applied after VARIANT_CLS.
const COMPACT_VARIANTS: ReadonlySet<ButtonVariant> = new Set(['ring', 'ring-danger', 'link']);

const SIZE_CLS: Record<ButtonSize, string> = {
  sm: 'px-2.5 py-1 text-note',
  md: 'px-3 py-1.5 text-ui',
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

export function Button({
  variant = 'primary',
  size = 'md',
  type,
  className,
  ...rest
}: ButtonProps) {
  const compact = COMPACT_VARIANTS.has(variant);
  return (
    // eslint-disable-next-line react/button-has-type
    <button
      type={type ?? 'button'}
      className={cn(
        'inline-flex items-center justify-center font-medium transition-colors',
        'focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500 focus-visible:ring-offset-1',
        'disabled:cursor-not-allowed disabled:opacity-60',
        !compact && 'rounded-md',
        VARIANT_CLS[variant],
        !compact && SIZE_CLS[size],
        className,
      )}
      {...rest}
    />
  );
}

export default Button;
