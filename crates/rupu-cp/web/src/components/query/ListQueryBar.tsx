// The standard list filter row: one full-width query line above a list's
// table, with nothing beside it. Under it, quietly: why the query can't be
// evaluated, one line per alert (an incomplete list), and "N of M" while a
// query narrows a list the browser holds in full. Every list page mounts it
// the same way.

import { useMemo, type ReactNode } from 'react';
import { useSearchParams } from 'react-router-dom';
import type { QueryField } from '../../lib/findingQuery/fields';
import { filterRows, rowFacets, type Facets, type FilterOptions, type RowField } from '../../lib/query/rows';
import { cn } from '../../lib/cn';
import { QueryBar } from './QueryBar';
import type { FacetValue } from './suggest';

export interface ListQueryBarProps {
  value: string;
  onChange: (q: string) => void;
  fields: readonly QueryField[];
  facets?: Record<string, FacetValue[]>;
  /** Rows before the query; with `shown`, "N of M" while a query narrows
   *  the list. A list the server filters knows neither and passes none. */
  total?: number;
  /** Rows the query keeps. */
  shown?: number;
  label: string;
  placeholder?: string;
  valueTone?: (key: string, value: string) => string | null;
  /** Why the query matches nothing it could (`filterRows`' error). */
  error?: string | null;
  /** One quiet line each, under the bar. */
  alerts?: ReactNode[];
  className?: string;
}

export function ListQueryBar({
  value,
  onChange,
  fields,
  facets,
  total,
  shown,
  label,
  placeholder,
  valueTone,
  error,
  alerts = [],
  className,
}: ListQueryBarProps) {
  const active = value.trim() !== '' && total !== undefined && shown !== undefined;
  return (
    <div className={cn('mb-4', className)}>
      <QueryBar
        value={value}
        onChange={onChange}
        fields={fields}
        facets={facets}
        label={label}
        placeholder={placeholder}
        valueTone={valueTone}
      />
      {(error || alerts.length > 0 || active) && (
        <div className="mt-1 flex items-start gap-3 px-1 text-note">
          <div className="min-w-0 flex-1 space-y-0.5">
            {error && (
              <p role="alert" className="text-err">
                {error}
              </p>
            )}
            {alerts.map((a, i) => (
              <p key={i} className="text-ink-mute">
                {a}
              </p>
            ))}
          </div>
          {active && (
            <span data-testid="query-count" className="shrink-0 tabular-nums text-ink-mute">
              {shown} of {total}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/** The list's query, kept in the URL's `param` (other params ride along). */
export function useListQuery(param = 'q'): [string, (q: string) => void] {
  const [params, setParams] = useSearchParams();
  const q = params.get(param) ?? '';
  const setQ = (next: string) =>
    setParams(
      (prev) => {
        const p = new URLSearchParams(prev);
        if (next.trim()) p.set(param, next);
        else p.delete(param);
        return p;
      },
      { replace: true },
    );
  return [q, setQ];
}

/** `rows` filtered by `q` over `fields`, with the facets of every row. */
export function useFilteredRows<Row>(
  rows: readonly Row[],
  q: string,
  fields: readonly RowField<Row>[],
  opts?: FilterOptions<Row>,
): { rows: Row[]; error: string | null; facets: Facets } {
  const facets = useMemo(() => rowFacets(rows, fields), [rows, fields]);
  const result = useMemo(() => filterRows(rows, q, fields, opts), [rows, q, fields, opts]);
  return { ...result, facets };
}

/** What a list shows when its query keeps none of its rows. */
export function NoMatches({ noun }: { noun: string }) {
  return <p className="rounded-xl border border-border bg-panel px-4 py-8 text-center text-sm text-ink-dim">No {noun} match the filter.</p>;
}

export default ListQueryBar;
