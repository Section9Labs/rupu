// The client-side evaluator behind every list the browser holds in full: a
// list supplies a registry of `RowField`s (the grammar's field spec, the
// QueryBar's presentation, and how the field reads a row and matches it) and
// `filterRows` evaluates a query over it. A term's values OR, terms AND, `-`
// negates, bare words search the fields marked `text`. Everything compares
// ignoring case.

import { parseQuery, type Term } from '../findingQuery/grammar';
import type { QueryField } from '../findingQuery/fields';
import { roleWordOf } from '../messages';
import type { FacetValue } from '../../components/query/suggest';
import { admitsSince, admitsUntil, parseRfc3339, parseTimeValue, type TimeValue } from './time';

export type RowValue = string | number | boolean | null | undefined | readonly (string | number | boolean | null | undefined)[];

/**
 * How a field's values meet a query value:
 * - `exact`, `contains`, `prefix`: string comparisons.
 * - `ordered`: the field's `values` are a rank ladder, low to high; declared
 *   with the grammar's `severity` kind, the field takes `>=` and friends.
 * - `address`: a message-space address; a value with no `#` also names every
 *   address with that role word (`pipit` is `pipit#1` and `pipit#20`).
 * - `since`, `until`: `get` returns an RFC 3339 timestamp; the value is a
 *   `time` (`./time.ts`): a span back from now (`30m`, `12h`, `7d`, `2w`), a
 *   UTC date (its whole day) or an RFC 3339 time, the same rules the server
 *   applies.
 */
export type MatchMode = 'exact' | 'contains' | 'prefix' | 'ordered' | 'address' | 'since' | 'until';

export interface RowField<Row> extends QueryField {
  get: (row: Row) => RowValue;
  match: MatchMode;
  /** Suggest the values in use, with counts. A function names the values to
   *  count when they differ from the ones `get` matches. */
  facet?: boolean | ((row: Row) => RowValue);
  /** Bare words search this field, as `contains`. */
  text?: boolean;
  /** An `ordered` field's rank ladder, low to high, when its `values` (what
   *  the field accepts) also hold values outside it, which never compare. */
  order?: readonly string[];
}

export type Facets = Record<string, FacetValue[]>;

export interface FilterResult<Row> {
  rows: Row[];
  error: string | null;
}

export interface FilterOptions<Row> {
  /** The clock `since` / `until` count back from, in ms since the epoch. */
  now?: number;
  /** What bare words search, when it isn't the `text` fields' values. */
  text?: (row: Row) => RowValue;
}

export function rowStrings(v: RowValue): string[] {
  const xs: readonly unknown[] = Array.isArray(v) ? v : [v];
  return xs.filter((x) => x !== null && x !== undefined && x !== '').map((x) => String(x));
}

function compare(have: number, want: number, op: Term['op']): boolean {
  switch (op) {
    case 'gt':
      return have > want;
    case 'ge':
      return have >= want;
    case 'lt':
      return have < want;
    case 'le':
      return have <= want;
    default:
      return have === want;
  }
}

function valueHit<Row>(field: RowField<Row>, have: string[], raw: string, op: Term['op']): boolean {
  const v = raw.toLowerCase();
  const xs = have.map((x) => x.toLowerCase());
  switch (field.match) {
    case 'exact':
      return xs.includes(v);
    case 'contains':
      return xs.some((x) => x.includes(v));
    case 'prefix':
      return xs.some((x) => x.startsWith(v));
    case 'address': {
      const byRole = !v.includes('#');
      return xs.some((x) => x === v || (byRole && roleWordOf(x) === v));
    }
    case 'ordered': {
      if (op === 'eq') return xs.includes(v);
      const ladder = (field.order ?? field.values).map((x) => x.toLowerCase());
      const want = ladder.indexOf(v);
      return want >= 0 && xs.some((x) => ladder.includes(x) && compare(ladder.indexOf(x), want, op));
    }
    default:
      return false;
  }
}

type Pred<Row> = (r: Row) => boolean;

/** One term as a row predicate, or the reason its value can't match. */
function termPred<Row>(
  t: Term,
  field: RowField<Row> | undefined,
  textOf: (r: Row) => string[],
  now: number,
): Pred<Row> | string {
  if (!field) {
    const vs = t.values.map((v) => v.toLowerCase());
    return (r) => {
      const hay = textOf(r).map((x) => x.toLowerCase());
      return vs.some((v) => hay.some((x) => x.includes(v)));
    };
  }
  if (field.match === 'since' || field.match === 'until') {
    const admits = field.match === 'since' ? admitsSince : admitsUntil;
    const bounds: TimeValue[] = [];
    for (const v of t.values) {
      const b = parseTimeValue(v);
      if (b === null) return `\`${v}\` is not a time (expected 30m, 12h, 7d, 2w, 2026-10-01 or an RFC 3339 time)`;
      bounds.push(b);
    }
    return (r) =>
      rowStrings(field.get(r)).some((x) => {
        const ts = parseRfc3339(x);
        return ts !== null && bounds.some((b) => admits(b, ts, now));
      });
  }
  return (r) => {
    const have = rowStrings(field.get(r));
    return t.values.some((v) => valueHit(field, have, v, t.op));
  };
}

/**
 * Filter `rows` by `q`. A query that doesn't parse (a bad time on a `time`
 * field included) filters nothing and comes back as the error (the QueryBar
 * shows its chip in red). A time value on a field not declared `time` that
 * names no time matches no row and comes back as the error.
 */
export function filterRows<Row>(
  rows: readonly Row[],
  q: string,
  fields: readonly RowField<Row>[],
  opts: FilterOptions<Row> = {},
): FilterResult<Row> {
  if (!q.trim()) return { rows: [...rows], error: null };
  const p = parseQuery(q, fields);
  if (!p.ok) return { rows: [...rows], error: p.error.message };
  const now = opts.now ?? Date.now();
  const byKey = new Map(fields.map((f) => [f.key, f]));
  const textFields = fields.filter((f) => f.text);
  const textOf = opts.text
    ? (r: Row) => rowStrings(opts.text!(r))
    : (r: Row) => textFields.flatMap((f) => rowStrings(f.get(r)));
  const preds: Pred<Row>[] = [];
  for (const t of p.terms) {
    const pred = termPred(t, byKey.get(t.key), textOf, now);
    if (typeof pred === 'string') return { rows: [], error: pred };
    preds.push(t.neg ? (r) => !pred(r) : pred);
  }
  return { rows: rows.filter((r) => preds.every((pr) => pr(r))), error: null };
}

/** The values in use for every `facet` field, most used first; ties keep the
 *  order they were first seen in. */
export function rowFacets<Row>(rows: readonly Row[], fields: readonly RowField<Row>[]): Facets {
  const out: Facets = {};
  for (const f of fields) {
    if (!f.facet) continue;
    const read = typeof f.facet === 'function' ? f.facet : f.get;
    const counts = new Map<string, number>();
    for (const r of rows) for (const v of rowStrings(read(r))) counts.set(v, (counts.get(v) ?? 0) + 1);
    out[f.key] = [...counts].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
  }
  return out;
}

type FieldInit<Row> = Omit<RowField<Row>, 'aliases' | 'kind' | 'values'> & Partial<Pick<RowField<Row>, 'aliases' | 'kind' | 'values'>>;

/** A field with the grammar's defaults: no aliases, `severity` kind for an
 *  `ordered` field, `time` for a `since` / `until` one, `enum` for one with a
 *  closed value set, else `text`. */
export function rowField<Row>(f: FieldInit<Row>): RowField<Row> {
  const values = f.values ?? [];
  const kind =
    f.kind ??
    (f.match === 'ordered'
      ? 'severity'
      : f.match === 'since' || f.match === 'until'
        ? 'time'
        : values.length > 0
          ? 'enum'
          : 'text');
  return { ...f, aliases: f.aliases ?? [], kind, values };
}
