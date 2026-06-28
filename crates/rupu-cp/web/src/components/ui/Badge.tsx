// Generic chip primitive for the rupu CP UI. Tone-coloured pill for small
// metadata labels (trigger kinds, counts, tags). NOT for status — use
// StatusPill — and NOT for severity — use coverage/SeverityChip.
//
// Only static Tailwind class strings (no `bg-${x}` templates) so the JIT can
// see every class. Pass `className` for case-transforms / tracking extras.

import { type HTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export type BadgeTone =
  | 'neutral'
  | 'brand'
  | 'violet'
  | 'sky'
  | 'amber'
  | 'green'
  | 'red';

export type BadgeSize = 'sm' | 'md';

const TONE_CLS: Record<BadgeTone, string> = {
  neutral: 'bg-slate-100 text-slate-600',
  brand: 'bg-brand-50 text-brand-700',
  violet: 'bg-violet-50 text-violet-700',
  sky: 'bg-sky-50 text-sky-700',
  amber: 'bg-amber-50 text-amber-700',
  green: 'bg-emerald-50 text-emerald-700',
  red: 'bg-red-50 text-red-700',
};

const SIZE_CLS: Record<BadgeSize, string> = {
  sm: 'px-1.5 py-0.5 text-[10px]',
  md: 'px-2 py-0.5 text-[11px]',
};

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement> {
  tone?: BadgeTone;
  size?: BadgeSize;
}

export function Badge({
  tone = 'neutral',
  size = 'sm',
  className,
  ...rest
}: BadgeProps) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded font-medium',
        TONE_CLS[tone],
        SIZE_CLS[size],
        className,
      )}
      {...rest}
    />
  );
}

export default Badge;
