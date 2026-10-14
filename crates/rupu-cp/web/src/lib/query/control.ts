// A control term: a query key that changes what a list loads (which project's
// layer, which rollup range) rather than which rows it keeps. The page reads
// it, and filters the rows by the query without it.
import { parseQuery, type FieldSpec } from '../findingQuery/grammar';
import { soleValues, withTerm } from '../findingQuery/shortcut';

export interface ControlTerm {
  /** The one value it names; null when the query has none (or can't parse). */
  value: string | null;
  /** The query without the key's terms. */
  rest: string;
  /** Set when the key is there but names other than one value, positively. */
  error: string | null;
}

export function controlTerm(q: string, fields: readonly FieldSpec[], key: string, usage: string): ControlTerm {
  const p = parseQuery(q, fields);
  if (!p.ok) return { value: null, rest: q, error: null };
  const rest = withTerm(q, fields, key, null);
  if (!p.terms.some((t) => t.key === key)) return { value: null, rest, error: null };
  const sole = soleValues(q, fields, key);
  if (sole === null || sole.length !== 1) return { value: null, rest, error: usage };
  return { value: sole[0], rest, error: null };
}
