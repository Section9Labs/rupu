// ListSkeleton — the loading view for a run/session list, in the real table
// layout (spec §"Loading & empty"): a bordered panel of pulsing rows shaped
// like the rows about to render, so the page never jumps from a centred
// spinner to a full table.
//
// It keeps the accessible announcement the bare `Spinner` used to carry: the
// panel is a `role="status"` live region labelled with the loading copy, and
// the same copy is repeated in an `sr-only` span, so screen readers (and the
// list tests that assert on the loading label) still see it while the visible
// rows are purely decorative.

import { Skeleton } from '../ui/Skeleton';

export interface ListSkeletonProps {
  /** Loading copy, e.g. "Loading agent runs…" — announced, not drawn. */
  label: string;
  /** How many placeholder rows to draw. */
  rows?: number;
}

export function ListSkeleton({ label, rows = 8 }: ListSkeletonProps) {
  return (
    <div
      role="status"
      aria-label={label}
      className="overflow-hidden rounded-xl border border-border bg-panel"
    >
      <span className="sr-only">{label}</span>
      {Array.from({ length: rows }).map((_, i) => (
        <div
          key={i}
          aria-hidden
          className="flex items-center gap-4 border-t border-border px-4 py-3.5 first:border-t-0"
        >
          <Skeleton className="h-5 w-20 rounded-full" />
          <Skeleton className="h-4 w-44" />
          <div className="flex-1" />
          <Skeleton className="hidden h-4 w-16 sm:block" />
          <Skeleton className="h-4 w-12" />
        </div>
      ))}
    </div>
  );
}

export default ListSkeleton;
