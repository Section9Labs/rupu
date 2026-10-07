// What the spotlight dropdown offers for the token being typed (Ghost's
// `graph3d/query.ts` `suggest`, adapted to a registry + server facets):
//   ''            → every key (`key:`)
//   'ta'          → fuzzy-matched keys, then "search for 'ta'" as text
//   'tag:ne'      → values for `tag` in use (facets) or the key's fixed
//                   values, fuzzy-matched on the part after the last comma
import { fuzzyScore } from '../../lib/fuzzy';
import { findField, quoteValue } from '../../lib/findingQuery/grammar';
import type { QueryField } from '../../lib/findingQuery/fields';

export interface FacetValue {
  value: string;
  count: number;
}

export interface Suggestion {
  id: string;
  kind: 'key' | 'value' | 'text';
  label: string;
  detail?: string;
  /** The draft text after accepting this suggestion. */
  insert: string;
  /** Accepting commits `insert` as a chip (else it only replaces the draft). */
  commit: boolean;
  field?: QueryField;
  value?: string;
  /** Matched char indices in `label`, for highlighting. */
  matched: number[];
}

const KEY_OP = /^([A-Za-z_]+)(>=|<=|:|>|<)(.*)$/s;

/**
 * Split a keyed value at its last item-separating comma, walking it the way
 * the grammar's `scan` does: a quote opens only at an item start (the value
 * start or right after a comma), and `\` escapes the next char. `prefix` is
 * everything up to and including that comma; `partial` is the item being
 * typed, with the quote of a still-open leading quote stripped.
 */
function splitItem(rest: string): { prefix: string; partial: string } {
  const cs = Array.from(rest);
  let cut = -1;
  let itemStart = true;
  let quote: string | null = null;
  for (let i = 0; i < cs.length; i++) {
    const c = cs[i];
    if (quote !== null) {
      if (c === '\\') i++;
      else if (c === quote) {
        quote = null;
        itemStart = false;
      }
      continue;
    }
    if (c === '\\') {
      i++;
      itemStart = false;
    } else if (itemStart && (c === '"' || c === "'")) {
      quote = c;
      itemStart = false;
    } else if (c === ',') {
      cut = i;
      itemStart = true;
    } else {
      itemStart = false;
    }
  }
  const prefix = cs.slice(0, cut + 1).join('');
  const item = cs.slice(cut + 1).join('');
  return { prefix, partial: quote !== null ? item.slice(1) : item };
}

export function suggest(
  draft: string,
  fields: readonly QueryField[],
  facets: Record<string, FacetValue[]> | undefined,
  max = 8,
): Suggestion[] {
  const neg = draft.length > 1 && draft.startsWith('-');
  const body = neg ? draft.slice(1) : draft;
  const sign = neg ? '-' : '';
  const m = KEY_OP.exec(body);
  if (m) {
    const [, name, op, rest] = m;
    const field = findField(name, fields) as QueryField | undefined;
    if (!field) return [];
    // A comparison takes one value and only `severity` takes one.
    if (op !== ':' && field.kind !== 'severity') return [];
    const { prefix, partial } = splitItem(rest);
    if (op !== ':' && prefix !== '') return [];
    const counts = new Map((facets?.[field.key] ?? []).map((f) => [f.value, f.count]));
    const pool = field.values.length > 0 ? field.values : [...counts.keys()];
    return pool
      .map((value) => ({ value, hit: fuzzyScore(partial, value), count: counts.get(value) ?? 0 }))
      .filter((x) => x.hit !== null)
      .sort((a, b) =>
        field.values.length > 0 && partial === ''
          ? 0
          : b.hit!.score - a.hit!.score || b.count - a.count || a.value.localeCompare(b.value),
      )
      .slice(0, max)
      .map((x) => ({
        id: `v:${x.value}`,
        kind: 'value' as const,
        label: x.value,
        detail: counts.has(x.value) ? String(x.count) : undefined,
        insert: `${sign}${field.key}${op}${prefix}${quoteValue(x.value)}`,
        commit: true,
        field,
        value: x.value,
        matched: x.hit!.matched,
      }));
  }
  const keys = fields
    .map((field) => {
      const hits = [field.key, ...field.aliases].map((k) => fuzzyScore(body, k)).filter((h) => h !== null);
      const best = hits.sort((a, b) => b!.score - a!.score)[0];
      return best ? { field, hit: best } : null;
    })
    .filter((x): x is NonNullable<typeof x> => x !== null)
    .sort((a, b) => (body === '' ? 0 : b.hit!.score - a.hit!.score))
    .slice(0, body === '' ? max : max - 1)
    .map(({ field, hit }) => ({
      id: `k:${field.key}`,
      kind: 'key' as const,
      label: field.key,
      detail: field.description,
      insert: `${sign}${field.key}:`,
      commit: false,
      field,
      matched: body === '' ? [] : hit!.matched,
    }));
  if (body === '') return keys;
  return [
    ...keys,
    { id: 'text', kind: 'text', label: `Search “${body}”`, insert: draft, commit: true, matched: [] },
  ];
}
