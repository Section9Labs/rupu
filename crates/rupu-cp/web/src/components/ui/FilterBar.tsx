// FilterBar — the ONE control-row layout for every list page. Fixed slot
// order, never caller-configurable, so every page's filter row reads the
// same way left-to-right:
//
//   [Segmented view] [FilterPills group(s)…] --- spacer --- [SearchInput?] [HostSelect]
//
// A page uses only the slots it needs; empty slots are omitted entirely (no
// placeholder gap). One row, wraps on narrow viewports.

import type { ReactNode } from 'react';

export interface FilterBarProps {
  /** The Segmented "which view" control, e.g. Runs/Cycles/Claims. */
  view?: ReactNode;
  /** One or more FilterPills groups. */
  filters?: ReactNode;
  /** The SearchInput, when the page supports free-text filtering. */
  search?: ReactNode;
  /** The scope select (HostSelect) — "data from where?". */
  scope?: ReactNode;
}

export function FilterBar({ view, filters, search, scope }: FilterBarProps) {
  const hasLeft = Boolean(view || filters);
  const hasRight = Boolean(search || scope);
  return (
    <div className="flex flex-wrap items-center gap-2.5">
      {view}
      {filters}
      {/* Spacer only matters when there's something to push right; an empty
          flex-1 div is otherwise inert. */}
      {hasLeft && hasRight && <div className="flex-1" aria-hidden="true" />}
      {search}
      {scope}
    </div>
  );
}

export default FilterBar;
