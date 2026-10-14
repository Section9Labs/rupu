// The browser's side of a list the server filters (`?q=`): which hosts a
// query's `host:` terms leave to ask, the facets the hosts' answers add up
// to, and what the list can't show (a host offline, unable to filter, or
// whose older rows failed to load), for the one quiet line under the bar.

import { parseQuery, type FieldSpec } from '../findingQuery/grammar';
import type { ListHeaders, ServerFacets } from '../api';
import type { HostPick } from '../perHost/usePerHostPagedList';
import type { HostSeed, HostSlice } from '../perHost/types';
import type { FacetValue } from '../../components/query/suggest';
import type { Facets } from './rows';

/** The hosts a query's `host:` terms allow: a positive term's values (every
 *  positive term, so their intersection), less every negated term's. Null
 *  when it names no host or doesn't parse. */
export function hostPickOf(q: string, fields: readonly FieldSpec[]): HostPick | null {
  const p = parseQuery(q, fields);
  if (!p.ok) return null;
  const terms = p.terms.filter((t) => t.key === 'host');
  if (terms.length === 0) return null;
  let include: string[] | null = null;
  const exclude: string[] = [];
  for (const t of terms) {
    const vs = t.values.map((v) => v.toLowerCase());
    if (t.neg) exclude.push(...vs);
    else include = include === null ? vs : include.filter((v) => vs.includes(v));
  }
  return { include, exclude };
}

/** Sum the hosts' facet counts, most used first. */
export function mergeFacets(parts: readonly ServerFacets[]): Facets {
  const sums: Record<string, Map<string, number>> = {};
  for (const part of parts) {
    for (const [key, values] of Object.entries(part)) {
      const m = (sums[key] ??= new Map());
      for (const [value, n] of Object.entries(values)) m.set(value, (m.get(value) ?? 0) + n);
    }
  }
  const out: Facets = {};
  for (const [key, m] of Object.entries(sums)) {
    out[key] = [...m].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count || a.value.localeCompare(b.value));
  }
  return out;
}

export type HostState = 'live' | 'loading' | 'offline' | 'unavailable' | 'not queried';

/** Every registered host's state on this list: a slice's, else the query
 *  left it out. */
export function hostStates<T>(registered: readonly HostSeed[], slices: readonly HostSlice<T>[]): { host: HostSeed; state: HostState }[] {
  const byId = new Map(slices.map((s) => [s.hostId, s]));
  return registered.map((host) => {
    const s = byId.get(host.id);
    const state: HostState = !s
      ? 'not queried'
      : s.state === 'loading' || s.catchingUp
        ? 'loading'
        : s.state === 'ok'
          ? 'live'
          : s.state;
    return { host, state };
  });
}

/** `host:` suggestions: every registered host, its name and state beside
 *  the rows it holds (when the facets counted them). */
export function hostFacet<T>(
  registered: readonly HostSeed[],
  slices: readonly HostSlice<T>[],
  counts: readonly FacetValue[] | undefined,
): FacetValue[] {
  const n = new Map((counts ?? []).map((c) => [c.value, c.count]));
  return hostStates(registered, slices).map(({ host, state }) => ({
    value: host.id,
    count: n.get(host.id),
    detail: host.name !== host.id ? `${host.name} · ${state}` : state,
  }));
}

/** Why a host's rows are missing from the list. */
export type HostNote =
  | { kind: 'offline' | 'unavailable' | 'customer' | 'archive' | 'paging'; hostId: string; name: string }
  | { kind: 'field'; hostId: string; name: string; field: string };

const CUSTOMER_REASON = /can[’']t (report (a customer|customers)|be filtered by customer)/i;
const ARCHIVE_REASON = /can[’']t list archived/i;
const FIELD_REASON = /can[’']t report `([^`]+)`/i;

function fieldNote(hostId: string, name: string, field: string): HostNote {
  if (field === 'customer') return { kind: 'customer', hostId, name };
  if (field === 'is') return { kind: 'archive', hostId, name };
  return { kind: 'field', hostId, name, field };
}

/** What every host the list couldn't fully show is missing, once each. */
export function hostNotes<T>(
  slices: readonly HostSlice<T>[],
  headers: readonly ListHeaders[],
  registered: readonly HostSeed[],
): HostNote[] {
  const names = new Map([...registered.map((h) => [h.id, h.name] as const), ...slices.map((s) => [s.hostId, s.name] as const)]);
  const nameOf = (id: string) => names.get(id) ?? id;
  const out: HostNote[] = [];
  const seen = new Set<string>();
  const add = (n: HostNote) => {
    const key = `${n.kind}\u0000${n.hostId}\u0000${n.kind === 'field' ? n.field : ''}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push(n);
  };
  for (const s of slices) {
    if (s.state === 'offline') add({ kind: 'offline', hostId: s.hostId, name: s.name });
    else if (s.state === 'unavailable') {
      const reason = s.reason ?? '';
      const field = FIELD_REASON.exec(reason)?.[1];
      if (CUSTOMER_REASON.test(reason)) add({ kind: 'customer', hostId: s.hostId, name: s.name });
      else if (ARCHIVE_REASON.test(reason)) add({ kind: 'archive', hostId: s.hostId, name: s.name });
      else if (field) add(fieldNote(s.hostId, s.name, field));
      else add({ kind: 'unavailable', hostId: s.hostId, name: s.name });
    }
    if (s.pagingFailed) add({ kind: 'paging', hostId: s.hostId, name: s.name });
  }
  for (const h of headers) {
    for (const id of h.withoutCustomer) add({ kind: 'customer', hostId: id, name: nameOf(id) });
    for (const u of h.unfilterable) add(fieldNote(u.host, nameOf(u.host), u.field));
  }
  return out;
}

/** One note as the line says it; `noun` is the list's rows ("runs"). */
export function noteText(n: HostNote, noun: string): string {
  switch (n.kind) {
    case 'offline':
      return `${n.name} offline: its ${noun} are missing`;
    case 'unavailable':
      return `${n.name} can't serve this list: its ${noun} are missing`;
    case 'customer':
      return `${n.name} can't filter by customer: its ${noun} are left out`;
    case 'archive':
      return `${n.name} can't list archived ${noun}: they are left out`;
    case 'field':
      return `${n.name} can't filter by ${n.field}: its ${noun} are left out`;
    case 'paging':
      return `${n.name}: older ${noun} couldn't load`;
  }
}
