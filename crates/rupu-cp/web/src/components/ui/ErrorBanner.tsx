// ErrorBanner — the `border-err/30 bg-err-bg` banner as a component,
// replacing the ~20 copy-pasted `<div>`s across list/detail pages.

import type { ReactNode } from 'react';
import { cn } from '../../lib/cn';

export interface ErrorBannerProps {
  children: ReactNode;
  className?: string;
}

export function ErrorBanner({ children, className }: ErrorBannerProps) {
  return (
    <div
      role="alert"
      className={cn(
        'rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err',
        className,
      )}
    >
      {children}
    </div>
  );
}

export default ErrorBanner;
