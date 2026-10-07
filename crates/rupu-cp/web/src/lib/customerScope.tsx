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
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useLocation } from 'react-router-dom';
import { api, type CustomerRow, type CustomerScope } from './api';

export const CUSTOMER_SCOPE_KEY = 'rupu.cp.customer';

export interface CustomerScopeValue {
  /** null = all customers. */
  scope: CustomerScope;
  /** The scoped customer's row (null for all / none / not loaded). */
  customer: CustomerRow | null;
  /** Active customers (archived ones are loaded on demand by the picker). */
  customers: CustomerRow[];
  setScope(next: CustomerScope): void;
  /** Refetch the list — after create / rename / archive / delete. */
  reload(): void;
  /** e.g. "Customer “acme” no longer exists — showing all customers." */
  notice: string | null;
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
  const [archivedHit, setArchivedHit] = useState<CustomerRow | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  // The latest scope, read by the load effect without re-triggering it.
  const scopeRef = useRef(scope);
  scopeRef.current = scope;

  const setScope = useCallback((next: CustomerScope) => {
    persist(next);
    setNotice(null);
    setScopeState(next);
  }, []);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      let rows: CustomerRow[];
      try {
        rows = await api.getCustomers();
      } catch {
        // Keep the scope: pages still pass it and the backend validates it.
        if (!cancelled) setCustomers([]);
        return;
      }
      if (cancelled) return;
      setCustomers(rows);

      const current = scopeRef.current;
      if (current === null || current === 'none' || rows.some((c) => c.slug === current)) {
        setArchivedHit(null);
        return;
      }
      // Not active: it may be archived (still a valid scope), else it is gone.
      let hit: CustomerRow | null = null;
      try {
        const all = await api.getCustomers({ archived: true });
        hit = all.find((c) => c.slug === current) ?? null;
      } catch {
        if (!cancelled) setArchivedHit(null);
        return; // can't tell — keep the scope
      }
      if (cancelled || scopeRef.current !== current) return;
      if (hit) {
        setArchivedHit(hit);
      } else {
        setArchivedHit(null);
        persist(null);
        setScopeState(null);
        setNotice(`Customer “${current}” no longer exists — showing all customers.`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [nonce]);

  const customer = useMemo(
    () =>
      scope === null || scope === 'none'
        ? null
        : (customers.find((c) => c.slug === scope) ??
          (archivedHit?.slug === scope ? archivedHit : null)),
    [scope, customers, archivedHit],
  );

  const value = useMemo<CustomerScopeValue>(
    () => ({ scope, customer, customers, setScope, reload, notice }),
    [scope, customer, customers, setScope, reload, notice],
  );
  return <CustomerScopeContext.Provider value={value}>{children}</CustomerScopeContext.Provider>;
}

export function useCustomerScope(): CustomerScopeValue {
  const ctx = useContext(CustomerScopeContext);
  if (!ctx) throw new Error('useCustomerScope must be used within a <CustomerScopeProvider>');
  return ctx;
}

/** The value list fetches pass as their `customer` argument. */
export function useCustomerParam(): CustomerScope {
  return useCustomerScope().scope;
}
