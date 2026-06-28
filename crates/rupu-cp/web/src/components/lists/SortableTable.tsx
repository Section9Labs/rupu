// Shared sortable columnar table (Okesu-style): a header row with clickable,
// sortable column headers (asc/desc toggle + chevron indicator + aria-sort) and
// a divided body of rows. Generic over the row type. Replaces the stacked
// two-line MetricRow pattern across the list pages.
//
// Sorting is purely client-side over the `rows` prop: a column opts in via
// `sortable` + `sortValue`. Strings compare case-insensitively (localeCompare);
// numbers compare numerically; null/undefined always sort LAST regardless of
// direction. The sort is stable (original order is the tiebreaker).

import { Fragment, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { ChevronDown, ChevronRight, ChevronUp } from 'lucide-react';
import { cn } from '../../lib/cn';

export interface Column<T> {
  key: string;
  header: string;
  align?: 'left' | 'right';
  /** Tailwind width class, e.g. `'w-24'`. */
  width?: string;
  sortable?: boolean;
  /** Raw comparable value for this column. Required for `sortable` columns —
   *  use the underlying number/string, never the formatted display string. */
  sortValue?: (row: T) => string | number | null;
  render: (row: T) => React.ReactNode;
}

export interface SortSpec {
  key: string;
  dir: 'asc' | 'desc';
}

function compare(a: string | number | null, b: string | number | null): number {
  // nulls/undefined always sort LAST (handled by caller before applying dir).
  if (typeof a === 'number' && typeof b === 'number') return a - b;
  return String(a).localeCompare(String(b), undefined, { sensitivity: 'base' });
}

export default function SortableTable<T>({
  columns,
  rows,
  rowKey,
  initialSort,
  rowHref,
  renderDetail,
}: {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  initialSort?: SortSpec;
  rowHref?: (row: T) => string | undefined;
  /** When provided, each row gets a leading expand chevron that toggles a
   *  full-width detail panel below it (for evidence / nested concerns / etc).
   *  Expandable rows are never link-wrapped (`rowHref` is ignored). */
  renderDetail?: (row: T) => React.ReactNode;
}) {
  const [sort, setSort] = useState<SortSpec | null>(initialSort ?? null);
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const expandable = Boolean(renderDetail);
  const totalCols = columns.length + (expandable ? 1 : 0);

  function toggleOpen(key: string) {
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  const sorted = useMemo(() => {
    if (!sort) return rows;
    const col = columns.find((c) => c.key === sort.key);
    if (!col?.sortValue) return rows;
    const sortValue = col.sortValue;
    const dirMul = sort.dir === 'asc' ? 1 : -1;
    return rows
      .map((row, i) => ({ row, i }))
      .sort((x, y) => {
        const va = sortValue(x.row);
        const vb = sortValue(y.row);
        const aNull = va === null || va === undefined;
        const bNull = vb === null || vb === undefined;
        if (aNull && bNull) return x.i - y.i;
        if (aNull) return 1; // nulls last, independent of direction
        if (bNull) return -1;
        const cmp = compare(va, vb);
        return cmp === 0 ? x.i - y.i : cmp * dirMul;
      })
      .map((d) => d.row);
  }, [rows, sort, columns]);

  function toggleSort(key: string) {
    setSort((prev) =>
      !prev || prev.key !== key
        ? { key, dir: 'asc' }
        : { key, dir: prev.dir === 'asc' ? 'desc' : 'asc' },
    );
  }

  return (
    <div className="bg-panel border border-border rounded-xl shadow-card overflow-hidden">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border text-meta uppercase tracking-wide text-ink-mute">
            {expandable && <th scope="col" className="w-8" aria-label="Expand" />}
            {columns.map((col) => {
              const active = sort?.key === col.key;
              const dir = active ? sort?.dir : undefined;
              const ariaSort = !col.sortable
                ? undefined
                : active
                  ? dir === 'asc'
                    ? 'ascending'
                    : 'descending'
                  : 'none';
              return (
                <th
                  key={col.key}
                  aria-sort={ariaSort}
                  scope="col"
                  className={cn(
                    'px-4 py-2 font-medium',
                    col.align === 'right' ? 'text-right' : 'text-left',
                    col.width,
                  )}
                >
                  {col.sortable ? (
                    <button
                      type="button"
                      onClick={() => toggleSort(col.key)}
                      aria-label={`Sort by ${col.header}`}
                      className={cn(
                        'inline-flex items-center gap-1 uppercase tracking-wide transition-colors hover:text-ink-dim',
                        col.align === 'right' && 'flex-row-reverse',
                        active && 'text-ink-dim',
                      )}
                    >
                      <span>{col.header}</span>
                      {active &&
                        (dir === 'asc' ? <ChevronUp size={12} /> : <ChevronDown size={12} />)}
                    </button>
                  ) : (
                    col.header
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {sorted.map((row) => {
            const key = rowKey(row);
            // Expandable tables never link-wrap rows (the chevron is the row's
            // interaction); plain tables honour rowHref.
            const href = expandable ? undefined : rowHref?.(row);
            const isOpen = expandable && open.has(key);
            return (
              <Fragment key={key}>
                <tr className="hover:bg-bg/60 transition-colors">
                  {expandable && (
                    <td className="w-8 pl-3 align-middle">
                      <button
                        type="button"
                        onClick={() => toggleOpen(key)}
                        aria-expanded={isOpen}
                        aria-label={isOpen ? 'Collapse row' : 'Expand row'}
                        className="flex h-5 w-5 items-center justify-center rounded text-ink-mute hover:bg-bg hover:text-ink-dim"
                      >
                        {isOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                      </button>
                    </td>
                  )}
                  {columns.map((col) => {
                    const alignCls = cn(
                      col.align === 'right' ? 'text-right tabular-nums' : 'text-left',
                      col.width,
                    );
                    // When the whole row is a link, each cell wraps its content in
                    // a block <Link> so the entire row is clickable (and every cell
                    // is a navigation target) without nesting anchors. Pages that
                    // use rowHref render plain content (no inner links); pages with
                    // per-column links / interactive cells omit rowHref.
                    return href ? (
                      <td key={col.key} className={alignCls}>
                        <Link to={href} className="block px-4 py-2.5 align-middle">
                          {col.render(row)}
                        </Link>
                      </td>
                    ) : (
                      <td key={col.key} className={cn('px-4 py-2.5 align-middle', alignCls)}>
                        {col.render(row)}
                      </td>
                    );
                  })}
                </tr>
                {isOpen && (
                  <tr className="bg-bg/40">
                    <td colSpan={totalCols} className="px-4 py-3">
                      {renderDetail?.(row)}
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
