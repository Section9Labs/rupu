// A control term: a query key that changes what a list loads (which project's
// layer, which rollup range) rather than which rows it keeps. The page reads
// it, and filters the rows by the query without it.
import { parseToken, tokenize, type FieldSpec } from '../findingQuery/grammar';
import { soleValues } from '../findingQuery/shortcut';

export interface ControlTerm {
  /** The one value it names; null when the query has none (or can't parse). */
  value: string | null;
  /** The query without the key's terms. */
  rest: string;
  /** Set when the key is there but names other than one value, positively. */
  error: string | null;
}

/** Read token by token, so a broken token elsewhere in the query (a link
 *  with a typo) doesn't reset what the page loads. */
export function controlTerm(q: string, fields: readonly FieldSpec[], key: string, usage: string): ControlTerm {
  const t = tokenize(q);
  if (!t.ok) return { value: null, rest: q, error: null };
  const isKey = (raw: string) => {
    const p = parseToken(raw, fields);
    return p.ok && p.terms[0]?.key === key;
  };
  const raws = t.tokens.map((x) => x.text);
  const mine = raws.filter(isKey);
  const rest = raws.filter((raw) => !isKey(raw)).join(' ');
  if (mine.length === 0) return { value: null, rest, error: null };
  const sole = soleValues(mine.join(' '), fields, key);
  if (sole === null || sole.length !== 1) return { value: null, rest, error: usage };
  return { value: sole[0], rest, error: null };
}
