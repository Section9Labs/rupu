// Shared sortable columnar table (Okesu-style): a header row with clickable,
// sortable column headers (asc/desc toggle + chevron indicator + aria-sort) and
// a divided body of rows. Generic over the row type. Replaces the stacked
// two-line MetricRow pattern across the list pages.
//
// Sorting is purely client-side over the `rows` prop: a column opts in via
// `sortable` + `sortValue`. Strings compare case-insensitively (localeCompare);
// numbers compare numerically; null/undefined always sort LAST regardless of
// direction. The sort is stable (original order is the tiebreaker).
//
// `virtualize` opts a long table into window virtualization: above its
// threshold only the rows near the viewport are mounted, with spacer rows
// standing in for the rest (see `useWindowVirtualRows`). Column widths still
// match the full table: a hidden, zero-height "width probe" body renders the
// rows that are widest in each column (see `widthProbeRows`).

import { Fragment, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { ChevronDown, ChevronRight, ChevronUp } from 'lucide-react';
import { cn } from '../../lib/cn';
import { useWindowVirtualRows } from './useWindowVirtualRows';

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
  /** Virtualized tables only: the exact text this column displays for a
   *  row (the same formatting its `render` uses), used to find the rows that
   *  are widest in this column so the hidden width probe can render them.
   *  Must be a pure function of the row. Falls back to `titleValue`, then
   *  a plain-string `render` result, then the stringified `sortValue`. */
  widthText?: (row: T) => string;
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

/** Rows within this many pixels of a column's widest are probed too, to
 *  absorb measurement error (font approximations, inner markup). */
const PROBE_SLACK_PX = 32;
/** The same slack when only character counts are available. */
const PROBE_SLACK_CHARS = 4;
/** At most this many probe rows per column. */
const PROBE_MAX_PER_COLUMN = 16;

/** The text a column displays for a row, for width-candidate selection:
 *  `widthText`, else `titleValue`, else a plain-string `render`, else the
 *  stringified `sortValue`; `null` when none of those yields a string. */
function widthTextOf<T>(col: Column<T>, row: T): string | null {
  if (col.widthText) return col.widthText(row);
  if (col.titleValue) return col.titleValue(row);
  const rendered = col.render(row);
  if (typeof rendered === 'string' || typeof rendered === 'number') return String(rendered);
  const v = col.sortValue?.(row);
  return v === null || v === undefined ? null : String(v);
}

type TextWidth = (font: string | undefined, text: string) => number;

/** A text measurer: canvas `measureText` in the given font when a 2D
 *  context exists, else the character count (jsdom). The second value says
 *  which unit it returns. */
function textMeasurer(): [TextWidth, 'px' | 'chars'] {
  try {
    if (typeof OffscreenCanvas !== 'undefined') {
      const ctx = new OffscreenCanvas(1, 1).getContext('2d');
      if (ctx) {
        return [
          (font, text) => {
            if (font) ctx.font = font;
            return ctx.measureText(text).width;
          },
          'px',
        ];
      }
    }
  } catch {
    // fall through to character counts
  }
  return [(_font, text) => text.length, 'chars'];
}

/** Indices (into `rows`, ascending) of the rows a virtualized table renders
 *  in its width probe: for each non-subject column, the row whose text is
 *  widest plus every row within the slack of it (one row per distinct
 *  string, at most `PROBE_MAX_PER_COLUMN`). Each distinct string is
 *  measured once. */
export function widthProbeRows<T>(
  rows: T[],
  columns: Column<T>[],
  fonts: ReadonlyMap<string, string>,
): number[] {
  const [measure, unit] = textMeasurer();
  const slack = unit === 'px' ? PROBE_SLACK_PX : PROBE_SLACK_CHARS;
  const picked = new Set<number>();
  for (const col of columns) {
    if (col.subject) continue;
    const font = fonts.get(col.key);
    // Distinct text → the first row showing it.
    const firstRow = new Map<string, number>();
    for (let i = 0; i < rows.length; i++) {
      const text = widthTextOf(col, rows[i]);
      if (text !== null && !firstRow.has(text)) firstRow.set(text, i);
    }
    const measured = Array.from(firstRow, ([text, i]) => ({ i, w: measure(font, text) }));
    if (measured.length === 0) continue;
    measured.sort((a, b) => b.w - a.w || a.i - b.i);
    const widest = measured[0].w;
    for (let k = 0; k < measured.length && k < PROBE_MAX_PER_COLUMN; k++) {
      if (k > 0 && measured[k].w < widest - slack) break;
      picked.add(measured[k].i);
    }
  }
  return Array.from(picked).sort((a, b) => a - b);
}

/** The font a column's text is drawn in, read from a mounted cell: its
 *  innermost first element (e.g. a `font-mono` span), else the cell. */
function cellFont(td: Element): string | undefined {
  let el: Element = td;
  while (el.firstElementChild) el = el.firstElementChild;
  const cs = getComputedStyle(el);
  // Built from the longhands: the `font` shorthand serializes to '' in
  // some engines.
  if (!cs.fontSize || !cs.fontFamily) return undefined;
  return `${cs.fontStyle || 'normal'} ${cs.fontWeight || '400'} ${cs.fontSize} ${cs.fontFamily}`;
}

export default function SortableTable<T>({
  columns,
  rows,
  rowKey,
  initialSort,
  rowHref,
  onRowClick,
  renderDetail,
  virtualize,
}: {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  initialSort?: SortSpec;
  rowHref?: (row: T) => string | undefined;
  /** Row-level activation handler for tables whose rows open an in-page
   *  panel rather than navigate (e.g. the netflow flow table's detail
   *  slide-over). Mutually exclusive with `rowHref` by convention — a row
   *  that navigates should stay a real link. Rows become focusable
   *  (`tabIndex=0`) and activate on Enter/Space as well as click, so
   *  whatever the panel discloses is never mouse-only. */
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
  /** Mount only the rows near the viewport once the table has more than
   *  `threshold` rows (window virtualization; sorting still covers every
   *  row). At or below the threshold rendering is unchanged. For tables
   *  without `renderDetail` (detail-row heights are not measured).
   *
   *  Column widths: an auto-layout table sizes a `fit` column to its widest
   *  cell over ALL rows, so a virtualized table also renders a hidden,
   *  zero-height width-probe body holding each non-subject column's widest
   *  rows (picked by `widthText`). That reproduces the full table's column
   *  widths exactly when every non-subject column is `fit` (the subject
   *  column takes the rest, as it does in the full table). */
  virtualize?: { threshold: number };
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

  const isVirtual = virtualize !== undefined && sorted.length > virtualize.threshold;
  const virtualKeys = isVirtual ? sorted.map(rowKey) : [];
  const win = useWindowVirtualRows(virtualKeys, isVirtual);

  // Width probe (virtualized only). The candidate rows depend on the row
  // set and the columns, never on scroll position or sort order. Columns
  // are keyed by their keys: callers rebuild the column array every render.
  const columnsRef = useRef(columns);
  columnsRef.current = columns;
  const columnSig = columns.map((c) => `${c.key}${c.subject ? '*' : ''}`).join('|');
  const [probeFonts, setProbeFonts] = useState<ReadonlyMap<string, string>>(new Map());
  const probeRows = useMemo(
    () => (isVirtual ? widthProbeRows(rows, columnsRef.current, probeFonts).map((i) => rows[i]) : []),
    [isVirtual, rows, columnSig, probeFonts],
  );
  // Once rows are mounted, read each column's font so the candidates are
  // measured in it (before paint; at most one re-pick).
  useLayoutEffect(() => {
    if (!isVirtual) return;
    const tr = win.bodyRef.current?.querySelector('tr:not([aria-hidden])');
    if (!tr) return;
    const cells = Array.from(tr.children).slice(hasDetailFeature ? 1 : 0);
    const next = new Map<string, string>();
    columnsRef.current.forEach((col, i) => {
      const font = cells[i] ? cellFont(cells[i]) : undefined;
      if (font) next.set(col.key, font);
    });
    setProbeFonts((prev) =>
      prev.size === next.size && Array.from(next).every(([k, v]) => prev.get(k) === v) ? prev : next,
    );
  }, [isVirtual, columnSig, hasDetailFeature]);

  function toggleSort(key: string) {
    setSort((prev) =>
      !prev || prev.key !== key
        ? { key, dir: 'asc' }
        : { key, dir: prev.dir === 'asc' ? 'desc' : 'asc' },
    );
  }

  /** One body row (plus its open detail row); `index` is its position in
   *  `sorted`. */
  function renderRow(row: T, index: number) {
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
          ref={isVirtual ? win.measure(key) : undefined}
          aria-rowindex={isVirtual ? index + 2 : undefined}
          onClick={onRowClick ? () => onRowClick(row) : undefined}
          tabIndex={onRowClick ? 0 : undefined}
          onKeyDown={
            onRowClick
              ? (e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    onRowClick(row);
                  }
                }
              : undefined
          }
          className={cn(
            'transition-colors',
            !isDeadLinkRow && 'hover:bg-bg/60',
            onRowClick && 'cursor-pointer focus-visible:bg-bg/60 focus-visible:outline-none',
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
  }

  return (
    <div className="bg-panel border border-border rounded-xl shadow-card overflow-hidden">
      <table className="w-full text-sm" aria-rowcount={isVirtual ? sorted.length + 1 : undefined}>
        <thead>
          <tr
            className="border-b border-border text-meta uppercase tracking-wide text-ink-mute"
            aria-rowindex={isVirtual ? 1 : undefined}
          >
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
        <tbody ref={win.bodyRef} className="divide-y divide-border">
          {isVirtual
            ? win.slots.flatMap((slot) =>
                slot.kind === 'spacer'
                  ? [
                      <tr key={`\u0000${slot.key}`} aria-hidden="true" style={{ height: slot.px }}>
                        <td colSpan={totalCols} />
                      </tr>,
                    ]
                  : sorted
                      .slice(slot.start, slot.end)
                      .map((row, j) => renderRow(row, slot.start + j)),
              )
            : sorted.map((row, i) => renderRow(row, i))}
        </tbody>
        {isVirtual && (
          // Width probe: the widest rows of each non-subject column, laid
          // out with the real cells' classes and horizontal padding so the
          // columns size exactly as in the full table — but zero-height
          // (no vertical padding, zero-height content, a collapsed row) and
          // invisible, inert and hidden from assistive tech.
          <tbody aria-hidden="true" {...{ inert: '' }}>
            {probeRows.map((row) => (
              <tr key={rowKey(row)} style={{ visibility: 'collapse' }}>
                {hasDetailFeature && <td className="w-8 pl-3 py-0" style={{ visibility: 'hidden' }} />}
                {columns.map((col) => (
                  <td
                    key={col.key}
                    className={cn('px-4 py-0', cellClass(col))}
                    style={{ visibility: 'hidden' }}
                  >
                    {!col.subject && (
                      <div style={{ height: 0, overflow: 'visible' }}>{col.render(row)}</div>
                    )}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        )}
      </table>
    </div>
  );
}
