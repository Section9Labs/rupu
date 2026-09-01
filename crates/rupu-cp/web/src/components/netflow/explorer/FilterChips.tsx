// Filter chips — one removable chip per active cross-filter plus one for
// an applied time window, and a Clear-all. Renders nothing when no filter
// (and no window) is active.
//
// The window chip keys off the SERVER's `window` echo, not the picker's
// local state — same source-of-truth rule as `NetflowWindowReadout`.

import type { NetflowFilters } from '../../../lib/netflow';

export type FilterDim = keyof NetflowFilters;

export interface FilterChipsProps {
  filters: NetflowFilters;
  /** Org filter keys → display labels (from `sankey.orgs`); a key with no
   *  entry (stale filter after a scope change) falls back to the raw key. */
  orgLabel: (key: string) => string;
  windowApplied: boolean;
  onRemove: (dim: FilterDim, key: string) => void;
  onClearWindow: () => void;
  onClearAll: () => void;
}

interface Chip {
  /** Unique identity: `<dim>:<filter key>`. The display label alone is
   *  NOT unique (two ASNs can share one org name), so keys and
   *  accessible names both carry the id. */
  id: string;
  label: string;
  onRemove: () => void;
}

export function FilterChips({
  filters,
  orgLabel,
  windowApplied,
  onRemove,
  onClearWindow,
  onClearAll,
}: FilterChipsProps) {
  const chips: Chip[] = [
    ...filters.workflows.map((v) => ({
      id: `workflow:${v}`,
      label: `workflow:${v}`,
      onRemove: () => onRemove('workflows', v),
    })),
    ...filters.origins.map((v) => ({
      id: `origin:${v}`,
      label: v,
      onRemove: () => onRemove('origins', v),
    })),
    ...filters.orgs.map((v) => ({
      id: `org:${v}`,
      label: `net:${orgLabel(v)}`,
      onRemove: () => onRemove('orgs', v),
    })),
    ...filters.hosts.map((v) => ({
      id: `host:${v}`,
      label: v,
      onRemove: () => onRemove('hosts', v),
    })),
    ...(windowApplied ? [{ id: 'window', label: 'window', onRemove: onClearWindow }] : []),
  ];
  // Any chip at all implies something to clear, so "Clear all" renders
  // unconditionally past this point.
  if (chips.length === 0) return null;

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="text-meta uppercase tracking-wider text-ink-mute">Filters</span>
      {chips.map((c) => (
        <button
          key={c.id}
          type="button"
          onClick={c.onRemove}
          aria-label={`Remove filter ${c.id}`}
          title={c.id}
          className="inline-flex items-center gap-1 rounded-md border border-brand-500/30 bg-brand-500/10 px-2 py-0.5 font-mono text-meta text-brand-500"
        >
          {c.label}
          <span aria-hidden className="opacity-60">
            ×
          </span>
        </button>
      ))}
      <button type="button" onClick={onClearAll} className="text-note text-ink-mute underline">
        Clear all
      </button>
    </div>
  );
}

export default FilterChips;
