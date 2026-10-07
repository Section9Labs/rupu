// The one-line info note a surface shows while a customer scope is set but
// the surface itself can't be filtered by customer (autoflow listings,
// coverage, the concern catalog, the dashboard's fleet counts) — so an
// unfiltered list or number is never passed off as the scoped customer's.
// Renders nothing when unscoped.

import type { ReactNode } from 'react';
import { Info } from 'lucide-react';
import { cn } from '../../lib/cn';
import { useCustomerScope } from '../../lib/customerScope';

export function UnscopedNote({ children, className }: { children: ReactNode; className?: string }) {
  const { scope } = useCustomerScope();
  if (scope === null) return null;
  return (
    <p
      role="note"
      className={cn(
        'flex items-center gap-2 rounded-lg border border-brand-100 bg-brand-50 px-3 py-2 text-note text-brand-700',
        className,
      )}
    >
      <Info size={13} aria-hidden className="shrink-0" />
      <span>{children}</span>
    </p>
  );
}
