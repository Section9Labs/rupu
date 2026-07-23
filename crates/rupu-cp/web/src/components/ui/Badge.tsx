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
  neutral: 'bg-surface text-ink',
  brand: 'bg-brand-50 text-brand-700',
  violet: 'bg-violet-50 text-violet-700',
  sky: 'bg-sky-50 text-sky-700',
  amber: 'bg-warn-bg text-warn',
  green: 'bg-ok-bg text-ok',
  red: 'bg-err-bg text-err',
};

const SIZE_CLS: Record<BadgeSize, string> = {
  sm: 'px-1.5 py-0.5 text-meta',
  md: 'px-2 py-0.5 text-note',
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
        'inline-flex items-center rounded font-medium whitespace-nowrap',
        TONE_CLS[tone],
        SIZE_CLS[size],
        className,
      )}
      {...rest}
    />
  );
}

export default Badge;
