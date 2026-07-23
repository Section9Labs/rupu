// Select — the shared `<select>` chrome, promoted out of `HostSelect` (which
// duplicated this exact style) so any dropdown control looks the same.
// Forwards every native `<select>` prop; callers own `<option>` children.

import { type SelectHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export type SelectProps = SelectHTMLAttributes<HTMLSelectElement>;

export const selectCls =
  'rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink ' +
  'focus:border-brand-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60';

export function Select({ className, children, ...rest }: SelectProps) {
  return (
    <select className={cn(selectCls, className)} {...rest}>
      {children}
    </select>
  );
}

export default Select;
