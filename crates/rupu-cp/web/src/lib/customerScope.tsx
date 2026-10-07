// Global customer scope: which customer the CP is filtered to.
//
// `null` = all customers, `'none'` = work with no customer, otherwise a slug.
// Initial value: `?customer=` from the URL (adopted, then persisted), else
// localStorage, else null. List fetches pass `useCustomerParam()` as their
// `customer` argument; the backend validates it (a 400 on an unknown slug is
// handled by the pages, which clear the scope through `setScope`).

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { useLocation } from 'react-router-dom';
import { api, type CustomerRow, type CustomerScope } from './api';

export const CUSTOMER_SCOPE_KEY = 'rupu.cp.customer';

export interface CustomerScopeValue {
  /** null = all customers. */
  scope: CustomerScope;
  /** The scoped customer's row (null for all / none / not yet resolved). */
  customer: CustomerRow | null;
  /** Active customers (archived ones are loaded on demand by the picker). */
  customers: CustomerRow[];
  /** Change the scope. Pass `row` when the caller already has the customer's
   *  row (the picker); otherwise it is resolved from the loaded list, or — for
   *  a slug not in the active list — looked up among archived customers. */
  setScope(next: CustomerScope, row?: CustomerRow): void;
  /** Clear a scope the backend rejected and show `message` as a one-line
   *  notice. Pages call this on a 400 from a request carrying the scope. */
  rejectScope(message: string): void;
  /** True once the active list has loaded (successfully) at least once. */
  loaded?: boolean;
  /** Refetch the list — after create / rename / archive / delete. A failed
   *  refetch keeps the rows already loaded. */
  reload(): void;
  /** e.g. "Customer “acme” no longer exists — showing all customers." */
  notice: string | null;
  /** Bumps on every `rejectScope`, so a repeated identical notice is a new
   *  occurrence (a dismissed one shows again). */
  noticeSeq: number;
}

function readStored(): CustomerScope {
  try {
    const raw = window.localStorage.getItem(CUSTOMER_SCOPE_KEY);
    return raw ? raw : null;
  } catch {
    return null;
  }
}

function persist(scope: CustomerScope): void {
  try {
    if (scope === null) window.localStorage.removeItem(CUSTOMER_SCOPE_KEY);
    else window.localStorage.setItem(CUSTOMER_SCOPE_KEY, scope);
  } catch {
    // private mode / quota — in-memory state still works
  }
}

const CustomerScopeContext = createContext<CustomerScopeValue | null>(null);

export function CustomerScopeProvider({ children }: { children: ReactNode }): JSX.Element {
  const location = useLocation();
  const [scope, setScopeState] = useState<CustomerScope>(() => {
    const fromUrl = new URLSearchParams(location.search).get('customer');
    const initial = fromUrl || readStored();
    if (fromUrl) persist(initial);
    return initial;
  });
  const [customers, setCustomers] = useState<CustomerRow[]>([]);
  const [loaded, setLoaded] = useState(false);
  // A scoped customer that is not in the active list: picked by the caller, or
  // found among the archived customers.
  const [extra, setExtra] = useState<CustomerRow | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [noticeSeq, setNoticeSeq] = useState(0);
  const [nonce, setNonce] = useState(0);

  const setScope = useCallback((next: CustomerScope, row?: CustomerRow) => {
    persist(next);
    setNotice(null);
    setExtra(row ?? null);
    setScopeState(next);
  }, []);
  const rejectScope = useCallback((message: string) => {
    persist(null);
    setExtra(null);
    setScopeState(null);
    setNotice(message);
    setNoticeSeq((n) => n + 1);
  }, []);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  // Load the active list (mount + reload()).
  useEffect(() => {
    let cancelled = false;
    api.getCustomers().then(
      (rows) => {
        if (cancelled) return;
        setCustomers(rows);
        setLoaded(true);
      },
      () => {
        // Keep the scope and any rows already loaded: pages still pass the
        // scope and the backend validates it.
      },
    );
    return () => {
      cancelled = true;
    };
  }, [nonce]);

  // Resolve a slug scope that the active list does not hold: it may be an
  // archived customer (still a valid scope), else it no longer exists.
  useEffect(() => {
    if (scope === null || scope === 'none') {
      setExtra(null);
      return;
    }
    if (!loaded) return;
    if (customers.some((c) => c.slug === scope) || extra?.slug === scope) return;
    let cancelled = false;
    api.getCustomers({ archived: true }).then(
      (all) => {
        if (cancelled) return;
        const hit = all.find((c) => c.slug === scope) ?? null;
        if (hit) setExtra(hit);
        else rejectScope(`Customer “${scope}” no longer exists — showing all customers.`);
      },
      () => {
        // Can't tell — keep the scope.
      },
    );
    return () => {
      cancelled = true;
    };
  }, [scope, customers, loaded, extra, rejectScope]);

  const customer = useMemo(
    () =>
      scope === null || scope === 'none'
        ? null
        : (customers.find((c) => c.slug === scope) ?? (extra?.slug === scope ? extra : null)),
    [scope, customers, extra],
  );

  const value = useMemo<CustomerScopeValue>(
    () => ({ scope, customer, customers, loaded, setScope, rejectScope, reload, notice, noticeSeq }),
    [scope, customer, customers, loaded, setScope, rejectScope, reload, notice, noticeSeq],
  );
  return <CustomerScopeContext.Provider value={value}>{children}</CustomerScopeContext.Provider>;
}

export function useCustomerScope(): CustomerScopeValue {
  const ctx = useContext(CustomerScopeContext);
  if (!ctx) throw new Error('useCustomerScope must be used within a <CustomerScopeProvider>');
  return ctx;
}

/** The active customers, for resolving a row's slug to a name and tint. Unlike
 *  `useCustomerScope` it tolerates a missing provider (an empty list): it only
 *  decorates a row, so a surface mounted without the scope still renders. */
export function useCustomerDirectory(): CustomerRow[] {
  return useContext(CustomerScopeContext)?.customers ?? [];
}

/** [`useCustomerDirectory`] plus whether the active list has loaded yet. */
export function useCustomerDirectoryState(): { customers: CustomerRow[]; loaded: boolean } {
  const ctx = useContext(CustomerScopeContext);
  return { customers: ctx?.customers ?? [], loaded: ctx?.loaded ?? true };
}

/** The value list fetches pass as their `customer` argument. */
export function useCustomerParam(): CustomerScope {
  return useCustomerScope().scope;
}
