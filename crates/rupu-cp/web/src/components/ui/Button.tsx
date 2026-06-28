// Shared button primitive for the rupu CP UI. Three variants + two sizes; all
// native button props pass through. Use `className` for layout extras (gaps,
// shrink-0, width) — twMerge in `cn` resolves any padding/colour conflicts so
// callers can still override per-button.

import { type ButtonHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'danger-outline';
export type ButtonSize = 'sm' | 'md';

const VARIANT_CLS: Record<ButtonVariant, string> = {
  primary: 'bg-brand-600 hover:bg-brand-700 text-white',
  secondary: 'border border-border bg-white hover:bg-bg text-ink',
  ghost: 'hover:bg-bg text-ink-dim',
  // Destructive: filled for the committed action, outline for the lighter one
  // (Cancel / Reject buttons that sit next to a primary).
  danger: 'bg-red-600 hover:bg-red-700 text-white',
  'danger-outline': 'border border-red-300 bg-white text-red-700 hover:bg-red-50',
};

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
  return (
    // eslint-disable-next-line react/button-has-type
    <button
      type={type ?? 'button'}
      className={cn(
        'inline-flex items-center justify-center rounded-md font-medium transition-colors',
        'focus:outline-none focus-visible:ring-2 focus-visible:ring-brand-500 focus-visible:ring-offset-1',
        'disabled:cursor-not-allowed disabled:opacity-60',
        VARIANT_CLS[variant],
        SIZE_CLS[size],
        className,
      )}
      {...rest}
    />
  );
}

export default Button;
