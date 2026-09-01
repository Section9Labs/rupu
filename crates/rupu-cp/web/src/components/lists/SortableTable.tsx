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
  /** Table-rules §5.1/§5.3: shrink this column to its content
   *  (`width:1%; white-space:nowrap` on both `<th>` and `<td>`) instead of
   *  letting it stretch. Use on every label/number/time column so the ONE
   *  `subject` column is the only thing that flexes. Combine with
   *  `align:'right'` for numbers/times (adds `tabular-nums`). */
  fit?: boolean;
  sortable?: boolean;
  /** Raw comparable value for this column. Required for `sortable` columns —
   *  use the underlying number/string, never the formatted display string. */
  sortValue?: (row: T) => string | number | null;
  /** Table-rules §5.1: marks the ONE flexible truncating column per table
   *  (the workflow/agent/file/… subject). Its `<td>` gets the
   *  `max-width:0` + inner `truncate` treatment instead of pushing the row
   *  wider than the table. Requires `titleValue` (or a plain-string
   *  `render`) so the full value is still available via the `title` tooltip
   *  attribute when truncated. */
  subject?: boolean;
  /** Plain-text form of this column's value, used for the subject column's
   *  `title` tooltip when `render` returns markup rather than a bare
   *  string. Falls back to `render(row)` itself when it happens to already
   *  be a string. */
  titleValue?: (row: T) => string;
  /** This column renders its OWN interactive controls (buttons, links to a
   *  different destination than the row) — typically a row-actions column.
   *  On a `rowHref` table, SortableTable normally wraps every cell's content
   *  in the row's own link so the whole row is clickable; for a column whose
   *  content is sometimes `null` (e.g. an action button that only appears
   *  conditionally), that wrapping link would render with no content and no
   *  accessible name, and any button the column DOES render would nest
   *  inside it (`<button>` inside `<a>`). Set `interactive: true` to render
   *  this column as a plain, unwrapped cell instead — its own controls stay
   *  independently focusable/announced and there is no nested/empty anchor.
   *  Row click-through for other columns is unaffected. */
  interactive?: boolean;
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

/** Table-rules §5.1: the subject column's cell content, wrapped so a long
 *  value truncates with an ellipsis instead of stretching the row (paired
 *  with the `max-w-0` class on the `<td>` itself — see `cellClass` below).
 *  The `title` attribute carries the untruncated value: `titleValue` when
 *  given, else the rendered content itself if it happens to already be a
 *  plain string. */
function renderCellContent<T>(col: Column<T>, row: T): React.ReactNode {
  const content = col.render(row);
  if (!col.subject) return content;
  const title = col.titleValue ? col.titleValue(row) : typeof content === 'string' ? content : undefined;
  return (
    <span className="block truncate" title={title}>
      {content}
    </span>
  );
}

/** Shared alignment/fit/subject classes for a column's `<td>` (and, minus
 *  the subject truncation, its `<th>`). */
function cellClass<T>(col: Column<T>): string {
  return cn(
    col.align === 'right' ? 'text-right tabular-nums' : 'text-left',
    col.fit && 'w-[1%] whitespace-nowrap',
    col.subject && 'max-w-0',
    col.width,
  );
}

export default function SortableTable<T>({
  columns,
  rows,
  rowKey,
  initialSort,
  rowHref,
  onRowClick,
  renderDetail,
}: {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  initialSort?: SortSpec;
  rowHref?: (row: T) => string | undefined;
  /** Row-level click handler for tables whose rows open an in-page panel
   *  rather than navigate (e.g. the netflow flow table's detail
   *  slide-over). Mutually exclusive with `rowHref` by convention — a row
   *  that navigates should stay a real link. Adds the pointer cursor +
   *  hover treatment so the affordance is visible. */
  onRowClick?: (row: T) => void;
  /** Per-row: return the detail-panel content for a row, or `null` (or
   *  `false`) when that particular row has nothing to expand. A row is
   *  expandable (gets the leading chevron + toggles a full-width detail
   *  panel below it) iff this returns non-null for it; `rowHref` link-wraps
   *  every OTHER row exactly as it would without `renderDetail` at all — the
   *  two are mutually exclusive per row, not table-global. Consumers whose
   *  `renderDetail` always returns content (the common case — evidence /
   *  nested-concern panels) see no behavior change: every row is
   *  expandable, so `rowHref` never applies, same as before. */
  renderDetail?: (row: T) => React.ReactNode;
}) {
  const [sort, setSort] = useState<SortSpec | null>(initialSort ?? null);
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  // Whether the table has the detail-panel FEATURE at all (reserves the
  // leading chevron column in the header and every row, for grid
  // alignment) — distinct from whether any GIVEN row is expandable.
  const hasDetailFeature = Boolean(renderDetail);
  const totalCols = columns.length + (hasDetailFeature ? 1 : 0);
  // rowHref link-wraps every cell so the whole row is clickable, but that
  // used to mean a 13-column row was 13 identical tab stops and screen
  // readers announced the same link 13 times per row. Only the subject
  // column's link (the table's ONE flexible, describing column — §5.1) stays
  // a real, focusable, announced link; every other cell's link is present
  // for mouse click purposes only (`tabIndex={-1}` + `aria-hidden`), so a row
  // is exactly one Tab stop. Falls back to the first column when a table has
  // no `subject` column at all.
  const subjectKey = columns.find((c) => c.subject)?.key ?? columns[0]?.key;

  /** Non-null/non-false detail content means this row is expandable. */
  function isDetailContent(node: React.ReactNode): boolean {
    return node !== null && node !== undefined && node !== false;
  }

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
            {hasDetailFeature && <th scope="col" className="w-8" aria-label="Expand" />}
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
                  className={cn('px-4 py-2 font-medium', cellClass(col))}
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
            // A row is expandable iff renderDetail returns non-null content
            // for IT specifically — not table-global. Rows without detail
            // fall through to rowHref, exactly as if renderDetail were absent.
            const detailContent = renderDetail?.(row);
            const isRowExpandable = hasDetailFeature && isDetailContent(detailContent);
            const href = isRowExpandable ? undefined : rowHref?.(row);
            const isOpen = isRowExpandable && open.has(key);
            // M5 (whole-branch-review): a table that declares `rowHref` can
            // still have individual rows with nothing to link to (e.g.
            // AgentRuns rows with no `transcript_path`) — those must not
            // show the same hover highlight as a genuinely clickable row,
            // or the affordance lies about what a click will do. Tables
            // that never declare `rowHref` at all (plain/action-only lists)
            // are unaffected — this only suppresses hover for a row that
            // fell through where the table's OWN rowHref came up empty.
            const isDeadLinkRow = Boolean(rowHref) && !href && !isRowExpandable;
            return (
              <Fragment key={key}>
                <tr
                  onClick={onRowClick ? () => onRowClick(row) : undefined}
                  className={cn(
                    'transition-colors',
                    !isDeadLinkRow && 'hover:bg-bg/60',
                    onRowClick && 'cursor-pointer',
                  )}
                >
                  {hasDetailFeature && (
                    <td className="w-8 pl-3 align-middle">
                      {isRowExpandable && (
                        <button
                          type="button"
                          onClick={() => toggleOpen(key)}
                          aria-expanded={isOpen}
                          aria-label={isOpen ? 'Collapse row' : 'Expand row'}
                          className="flex h-5 w-5 items-center justify-center rounded text-ink-mute hover:bg-bg hover:text-ink-dim"
                        >
                          {isOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                        </button>
                      )}
                    </td>
                  )}
                  {columns.map((col) => {
                    const alignCls = cellClass(col);
                    // When the whole row is a link, each cell wraps its content in
                    // a block <Link> so the entire row is clickable (and every cell
                    // is a navigation target) without nesting anchors — EXCEPT a
                    // column marked `interactive`, which carries its own real
                    // controls (and sometimes renders nothing at all): wrapping
                    // it in the row link would nest a <button> inside an <a>, or
                    // render an empty anchor with no accessible name when the
                    // column's content is conditionally `null`. Those columns
                    // always get the plain, unwrapped `<td>` branch below. Pages
                    // that use rowHref render plain content (no inner links) for
                    // their non-interactive cells; pages with per-column links
                    // omit rowHref entirely.
                    const isSubjectCell = col.key === subjectKey;
                    // I7: mouse-only for every non-subject cell reached in the
                    // link-wrapped branch (interactive columns never reach it —
                    // see the `href && !col.interactive` gate below).
                    const isMouseOnlyCell = !isSubjectCell;
                    return href && !col.interactive ? (
                      <td key={col.key} className={alignCls}>
                        <Link
                          to={href}
                          className="block px-4 py-2.5 align-middle"
                          {...(isMouseOnlyCell
                            ? { tabIndex: -1, 'aria-hidden': true }
                            : null)}
                        >
                          {renderCellContent(col, row)}
                        </Link>
                      </td>
                    ) : (
                      <td key={col.key} className={cn('px-4 py-2.5 align-middle', alignCls)}>
                        {renderCellContent(col, row)}
                      </td>
                    );
                  })}
                </tr>
                {isOpen && (
                  <tr className="bg-bg/40">
                    <td colSpan={totalCols} className="px-4 py-3">
                      {detailContent}
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
