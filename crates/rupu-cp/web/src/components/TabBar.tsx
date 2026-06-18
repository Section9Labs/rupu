// Shared horizontal tab bar — pill-style buttons on a subtle background
// strip. Ported verbatim (imports adapted) from Okesu so the Run detail
// view's Graph/Events tabs read the same as the rest of the design system.
//
// The pattern: a strip below the page header containing one TabButton
// per top-level view. Active tab gets a panel-colored background, a
// soft shadow, and a 1px ring — creates a clean lift without making
// the inactive tabs look hidden.

import type { LucideIcon } from 'lucide-react';
import { cn } from '../lib/cn';

export function TabBar({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-6 py-2.5 border-b border-border bg-panel/40 flex items-center gap-1">
      {children}
    </div>
  );
}

export function TabButton({
  active,
  onClick,
  icon: Icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: LucideIcon;
  label: string;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        'inline-flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-md',
        active
          ? 'bg-panel text-ink shadow-sm ring-1 ring-border'
          : 'text-ink-dim hover:text-ink hover:bg-slate-100',
      )}
    >
      <Icon size={12} />
      {label}
    </button>
  );
}
