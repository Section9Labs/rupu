// Shortcuts into a query: a tile, pill or chip that stands for one key's
// value set (the findings severity tiles, the Messages kind pills and
// participant chips). It reads as active when the query's only term for its
// key selects exactly that set, and clicking it swaps that key's tokens.
import { parseQuery, quoteValue, tokenize, type FieldSpec } from './grammar';

/** The values of the query's only term for `key`, when that term is positive
 *  and `:`; else null. */
export function soleValues(q: string, fields: readonly FieldSpec[], key: string): string[] | null {
  const p = parseQuery(q, fields);
  if (!p.ok) return null;
  const ts = p.terms.filter((t) => t.key === key);
  return ts.length === 1 && !ts[0].neg && ts[0].op === 'eq' ? ts[0].values : null;
}

/** `q` with every token for `key` removed, then `key:<v1,v2>` appended when
 *  `values` holds any. */
export function withTerm(q: string, fields: readonly FieldSpec[], key: string, values: string[] | null): string {
  const t = tokenize(q);
  const kept = (t.ok ? t.tokens.map((x) => x.text) : []).filter((raw) => {
    const p = parseQuery(raw, fields);
    return !(p.ok && p.terms[0]?.key === key);
  });
  const added = values && values.length > 0 ? [`${key}:${values.map(quoteValue).join(',')}`] : [];
  return [...kept, ...added].join(' ');
}
