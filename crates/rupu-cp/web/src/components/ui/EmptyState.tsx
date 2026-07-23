// EmptyState — the ONE dashed-box "nothing here" primitive, replacing the 26
// hand-rolled copies scattered across list/detail views. Dashed border,
// centered, optional dim icon + bold title + dim hint + optional action
// (e.g. a "Clear filters" button).

import type { ReactNode } from 'react';

export interface EmptyStateProps {
  title: string;
  /** `string` or a small `ReactNode` (e.g. a hint with a `font-mono` path
   *  segment) — rendered dim below the title either way. */
  hint?: ReactNode;
  action?: ReactNode;
  /** Optional glyph (typically a lucide icon) rendered dim and centered
   *  above the title — e.g. `<Sparkles size={20} />`. Omit for the plain
   *  title+hint look. */
  icon?: ReactNode;
}

export function EmptyState({ title, hint, action, icon }: EmptyStateProps) {
  return (
    <div className="rounded-lg border border-dashed border-border py-10 text-center">
      {icon && <div className="mb-2 flex justify-center text-ink-mute">{icon}</div>}
      <p className="text-sm font-semibold text-ink">{title}</p>
      {hint && <p className="mx-auto mt-1 max-w-sm text-note text-ink-mute">{hint}</p>}
      {action && <div className="mt-3 flex justify-center">{action}</div>}
    </div>
  );
}

export default EmptyState;
