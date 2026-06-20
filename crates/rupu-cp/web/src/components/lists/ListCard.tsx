import { ReactNode } from 'react';
import { cn } from '../../lib/cn';

// Common card wrapper for bucketed list sections. Ported from Okesu so the
// visual rhythm matches the rest of the design system: rounded panel, subtle
// border + shadow, divide-y rows.
export function ListCard({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'bg-panel border border-border rounded-xl shadow-card divide-y divide-border overflow-hidden',
        className,
      )}
    >
      {children}
    </div>
  );
}
