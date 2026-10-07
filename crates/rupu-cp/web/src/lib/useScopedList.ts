// useScopedList — the customer-scope plumbing the scoped pages share
// (WorkflowRuns, AgentRuns, Sessions, Findings, Projects, Usage, Dashboard).
//
// - `customer`: what the page filters by. A page mounted with a fixed
//   `customer` prop (embedded in a customer's detail tab) uses that; otherwise
//   it follows the global scope (`useCustomerParam()`), so the v1 routes and
//   the v2 Activity tabs, which mount the same components without a prop, both
//   pick it up.
// - `guard(promise)`: a 400 from a request that carried the GLOBAL scope means
//   the backend rejected the scope itself (a malformed slug, e.g. a stale
//   localStorage value) — clear it with a one-line notice (`rejectScope`) and
//   let the page refetch unfiltered. A fixed (embedded) customer is never
//   cleared from the global scope: it is the page's own, not the user's pick.
// - `reportHosts(requestHost, offset)`: a sink for the
//   `X-Rupu-Hosts-Without-Customer` header of one page of one host's listing.
//   A page-0 answer replaces what that host's listing reported; later pages
//   add to it. `hostsWithoutCustomer` is the union over every listing, and is
//   cleared whenever `resetKey` changes (a different filter is a fresh list).

import { useCallback, useMemo, useRef, useState } from 'react';
import { ApiError, apiErrorMessage, type CustomerScope, type HostsWithoutCustomerSink } from './api';
import { useCustomerScope } from './customerScope';

/** A 400 whose message is about the `customer` param: the backend rejected
 *  the scope (`customer: <why>`), not some other part of the request. */
export function isScopeRejection(e: unknown): boolean {
  return e instanceof ApiError && e.status === 400 && /customer/i.test(apiErrorMessage(e));
}

/** The one-line notice shown when the backend rejects the scope. */
export function scopeRejectedNotice(e: unknown): string {
  return `The customer filter was rejected (${apiErrorMessage(e)}) — showing all customers.`;
}

export interface ScopedList {
  customer: CustomerScope;
  /** True when the page is fixed to one customer by its `customer` prop. */
  embedded: boolean;
  /** Clear the global scope when `e` is the backend rejecting it (no-op for
   *  an embedded page, an unscoped one, or any other error). */
  rejectIfScopeError(e: unknown): void;
  /** `p`, passing its failure through `rejectIfScopeError` (then rethrown). */
  guard<T>(p: Promise<T>): Promise<T>;
  reportHosts(requestHost: string, offset: number): HostsWithoutCustomerSink | undefined;
  hostsWithoutCustomer: string[];
}

export function useScopedList(customerProp: string | undefined, resetKey: unknown[]): ScopedList {
  const { scope, rejectScope } = useCustomerScope();
  const embedded = !!customerProp;
  const customer: CustomerScope = embedded ? (customerProp as string) : scope;

  const rejectIfScopeError = useCallback(
    (e: unknown) => {
      // `customer` is the scope this request carried: a 400 that lands after
      // the scope moved on is ignored by `rejectScope`.
      if (!embedded && customer && isScopeRejection(e)) rejectScope(scopeRejectedNotice(e), customer);
    },
    [embedded, customer, rejectScope],
  );
  const guard = useCallback(
    <T,>(p: Promise<T>): Promise<T> =>
      p.catch((e: unknown) => {
        rejectIfScopeError(e);
        throw e;
      }),
    [rejectIfScopeError],
  );

  // Per requesting host: the hosts its listing's pages named.
  const key = JSON.stringify([customer, ...resetKey]);
  const [reported, setReported] = useState<{ key: string; byHost: Record<string, string[]> }>({ key, byHost: {} });
  const keyRef = useRef(key);
  keyRef.current = key;

  const reportHosts = useCallback(
    (requestHost: string, offset: number): HostsWithoutCustomerSink | undefined => {
      if (!customer) return undefined; // unscoped requests stay exactly as they were
      const at = key;
      return (ids: string[]) => {
        if (keyRef.current !== at) return; // an answer for a list that has since changed
        setReported((prev) => {
          const byHost = prev.key === at ? prev.byHost : {};
          const before = offset === 0 ? [] : (byHost[requestHost] ?? []);
          const next = [...new Set([...before, ...ids])];
          return { key: at, byHost: { ...byHost, [requestHost]: next } };
        });
      };
    },
    [customer, key],
  );

  const hostsWithoutCustomer = useMemo(
    () => (reported.key === key ? [...new Set(Object.values(reported.byHost).flat())].sort() : []),
    [reported, key],
  );

  return { customer, embedded, rejectIfScopeError, guard, reportHosts, hostsWithoutCustomer };
}
