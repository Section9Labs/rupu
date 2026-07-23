// Input — the shared text-input chrome, promoted out of
// `settings/ConfigField.tsx`'s `fieldCls` so any plain text/number input in
// the kit looks the same. Forwards every native `<input>` prop.

import { type InputHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export type InputProps = InputHTMLAttributes<HTMLInputElement>;

export const inputCls =
  'w-full max-w-sm rounded-md border border-border bg-panel px-3 py-1.5 text-sm text-ink shadow-sm ' +
  'placeholder:text-ink-mute transition-colors focus:border-brand-500 focus:outline-none ' +
  'focus:ring-2 focus:ring-brand-500/20 disabled:cursor-not-allowed disabled:opacity-60';

export function Input({ className, ...rest }: InputProps) {
  return <input className={cn(inputCls, className)} {...rest} />;
}

export default Input;
