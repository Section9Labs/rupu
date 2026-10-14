// usePagedQueryList — a single-source paged list the server filters by the
// page's query (a project's runs and sessions). On top of `usePagedList`:
// every request carries `q` (a new query or scope is a new list), the first
// page of a scope asks for `?facets=1` once, and a query that doesn't parse
// is the bar's error, never a page error: nothing is asked while the browser
// can't parse it, and a 400 from the server reads as no rows.

import { useCallback, useMemo, useRef, useState } from 'react';
import { queryErrorOf, type ListQueryOpts, type ServerFacets } from '../api';
import { parseQuery } from '../findingQuery/grammar';
import { usePagedList, type UsePagedListResult } from '../usePagedList';
import type { Facets, ServerField } from './rows';
import { mergeFacets } from './serverList';

export interface UsePagedQueryListOptions<T> {
  q: string;
  fields: readonly ServerField[];
  /** What the list is before the query: a new scope re-reads the facets. */
  scope: unknown[];
  fetch: (p: { offset: number; limit: number } & ListQueryOpts) => Promise<T[]>;
}

export interface UsePagedQueryListResult<T> extends UsePagedListResult<T> {
  facets: Facets;
  queryError: string | null;
}

export function usePagedQueryList<T>({ q, fields, scope, fetch }: UsePagedQueryListOptions<T>): UsePagedQueryListResult<T> {
  const scopeKey = JSON.stringify(scope);
  const listKey = JSON.stringify([q, scope]);
  const parsed = useMemo(() => parseQuery(q, fields), [q, fields]);
  const parses = parsed.ok;
  const [facets, setFacets] = useState<{ key: string; counts: ServerFacets } | null>(null);
  const [serverError, setServerError] = useState<{ key: string; message: string } | null>(null);
  const keysRef = useRef({ scopeKey, listKey, haveFacets: false });
  keysRef.current = { scopeKey, listKey, haveFacets: facets?.key === scopeKey };
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;

  const queried = useCallback(
    (p: { offset: number; limit: number }): Promise<T[]> => {
      if (!parses) return Promise.resolve([]);
      const { scopeKey: sk, listKey: lk, haveFacets } = keysRef.current;
      const onFacets =
        p.offset === 0 && !haveFacets
          ? (f: { facets: ServerFacets }) => {
              if (keysRef.current.scopeKey === sk) setFacets({ key: sk, counts: f.facets });
            }
          : undefined;
      return fetchRef.current({ ...p, q, onFacets }).catch((e: unknown) => {
        const qe = queryErrorOf(e);
        if (!qe) throw e;
        if (keysRef.current.listKey === lk) setServerError({ key: lk, message: qe.error });
        return [];
      });
    },
    [q, parses],
  );

  const list = usePagedList<T>({ fetch: queried, deps: [q, scopeKey] });
  const merged = useMemo(() => mergeFacets(facets?.key === scopeKey ? [facets.counts] : []), [facets, scopeKey]);
  const queryError = !parsed.ok ? parsed.error.message : serverError?.key === listKey ? serverError.message : null;
  return { ...list, facets: merged, queryError };
}
