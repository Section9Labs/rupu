// usePerHostQueryList — a per-host list the server filters by the page's
// query (workflow runs, agent runs, sessions). On top of
// `usePerHostPagedList`:
// - every request carries `q`; a new query (or scope) is a new list,
// - a query's `host:` terms pick which hosts are asked at all,
// - each host's first page of a scope asks for `?facets=1` once; the counts
//   are summed across hosts, and `host:` suggests every registered host with
//   its state,
// - a query that doesn't parse is the bar's error, never a page error: the
//   page asks no host while the browser can't parse it, and a 400 from the
//   server (a parser drift) reads as no rows,
// - `notes` says which hosts' rows are missing and why (offline,
//   unavailable, unable to filter, older pages failed).

import { useCallback, useMemo, useRef, useState } from 'react';
import { queryErrorOf, type ListHeaders, type ListQueryOpts, type ServerFacets } from '../api';
import { parseQuery } from '../findingQuery/grammar';
import { usePerHostPagedList, type PerHostFetchParams, type UsePerHostPagedListResult } from '../perHost/usePerHostPagedList';
import type { HostTagged } from '../perHost/types';
import type { Facets, ServerField } from './rows';
import { hostFacet, hostNotes, hostPickOf, mergeFacets, type HostNote } from './serverList';

export interface UsePerHostQueryListOptions<T> {
  /** The query the server gets (the page's implicit defaults applied). */
  q: string;
  fields: readonly ServerField[];
  /** What the list is before the query (customer scope, …): a new scope
   *  re-reads the facets. */
  scope: unknown[];
  fetch: (p: PerHostFetchParams & ListQueryOpts) => Promise<T[]>;
  timeField: keyof T & string;
  idField: keyof T & string;
  poll?: boolean;
}

export interface UsePerHostQueryListResult<T> extends UsePerHostPagedListResult<T> {
  facets: Facets;
  /** Why the query can't be evaluated (the client's parse, else the server's 400). */
  queryError: string | null;
  notes: HostNote[];
}

export function usePerHostQueryList<T extends HostTagged>({
  q,
  fields,
  scope,
  fetch,
  timeField,
  idField,
  poll,
}: UsePerHostQueryListOptions<T>): UsePerHostQueryListResult<T> {
  const scopeKey = JSON.stringify(scope);
  const listKey = JSON.stringify([q, scope]);
  const pick = useMemo(() => hostPickOf(q, fields), [q, fields]);
  const parsed = useMemo(() => parseQuery(q, fields), [q, fields]);
  const parses = parsed.ok;

  // Facets per host, for the current scope. A host's page 0 asks for them
  // until one answer has brought them.
  const [facetsByHost, setFacetsByHost] = useState<{ key: string; byHost: Record<string, ServerFacets> }>({ key: scopeKey, byHost: {} });
  const have = useRef<{ key: string; hosts: Set<string> }>({ key: scopeKey, hosts: new Set() });
  if (have.current.key !== scopeKey) have.current = { key: scopeKey, hosts: new Set() };

  // The headers each host's listing reported (page 0 replaces, later pages add).
  const [headers, setHeaders] = useState<{ key: string; byHost: Record<string, ListHeaders> }>({ key: listKey, byHost: {} });
  const [serverError, setServerError] = useState<{ key: string; message: string } | null>(null);

  const keysRef = useRef({ scopeKey, listKey });
  keysRef.current = { scopeKey, listKey };
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;

  const queried = useCallback(
    (p: PerHostFetchParams): Promise<T[]> => {
      if (!parses) return Promise.resolve([]);
      const { scopeKey: sk, listKey: lk } = keysRef.current;
      const wantFacets = p.offset === 0 && !have.current.hosts.has(p.host);
      const onFacets = wantFacets
        ? (f: { facets: ServerFacets }) => {
            if (keysRef.current.scopeKey !== sk) return;
            have.current.hosts.add(p.host);
            setFacetsByHost((prev) => ({ key: sk, byHost: { ...(prev.key === sk ? prev.byHost : {}), [p.host]: f.facets } }));
          }
        : undefined;
      const onHeaders = (h: ListHeaders) => {
        if (keysRef.current.listKey !== lk) return;
        setHeaders((prev) => {
          const byHost = prev.key === lk ? prev.byHost : {};
          const before = p.offset === 0 ? { withoutCustomer: [], unfilterable: [] } : (byHost[p.host] ?? { withoutCustomer: [], unfilterable: [] });
          return {
            key: lk,
            byHost: {
              ...byHost,
              [p.host]: {
                withoutCustomer: [...before.withoutCustomer, ...h.withoutCustomer],
                unfilterable: [...before.unfilterable, ...h.unfilterable],
              },
            },
          };
        });
      };
      return fetchRef.current({ ...p, q, onFacets, onHeaders }).catch((e: unknown) => {
        const qe = queryErrorOf(e);
        if (!qe) throw e;
        if (keysRef.current.listKey === lk) setServerError({ key: lk, message: qe.error });
        return [];
      });
    },
    [q, parses],
  );

  const list = usePerHostPagedList<T>({
    host: null,
    pick,
    fetch: queried,
    timeField,
    idField,
    deps: [q, scopeKey],
    poll,
  });

  const facets = useMemo(() => {
    const parts = facetsByHost.key === scopeKey ? Object.values(facetsByHost.byHost) : [];
    const merged = mergeFacets(parts);
    return { ...merged, host: hostFacet(list.registered, list.slices, merged.host) };
  }, [facetsByHost, scopeKey, list.registered, list.slices]);

  const notes = useMemo(
    () => hostNotes(list.slices, headers.key === listKey ? Object.values(headers.byHost) : [], list.registered),
    [list.slices, list.registered, headers, listKey],
  );

  const queryError = !parsed.ok ? parsed.error.message : serverError?.key === listKey ? serverError.message : null;

  return { ...list, facets, queryError, notes };
}
