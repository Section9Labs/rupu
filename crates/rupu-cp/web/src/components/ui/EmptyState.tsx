// EmptyState — the ONE dashed-box "nothing here" primitive, replacing the 26
// hand-rolled copies scattered across list/detail views. Dashed border,
// centered, bold title + dim hint + optional action (e.g. a "Clear filters"
// button).

import type { ReactNode } from 'react';

export interface EmptyStateProps {
  title: string;
  hint?: string;
  action?: ReactNode;
}

export function EmptyState({ title, hint, action }: EmptyStateProps) {
  return (
    <div className="rounded-lg border border-dashed border-border py-10 text-center">
      <p className="text-sm font-semibold text-ink">{title}</p>
      {hint && <p className="mx-auto mt-1 max-w-sm text-note text-ink-mute">{hint}</p>}
      {action && <div className="mt-3 flex justify-center">{action}</div>}
    </div>
  );
}

export default EmptyState;
