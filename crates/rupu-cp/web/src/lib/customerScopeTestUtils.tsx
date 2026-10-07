// Test-only helpers for the customer scope. Imported by *.test.tsx files only.
//
// Every page that follows the global scope reads it from
// `<CustomerScopeProvider>`, so page tests mount one. `withCustomerScope`
// does the setup: it clears any stored scope and stubs `api.getCustomers` —
// unless the test already spied it — so the provider never reaches the
// network and a scoped slug resolves instead of being rejected as "no longer
// exists".

import type { ReactNode } from 'react';
import { vi } from 'vitest';
import { api, type CustomerRow } from './api';
import { CUSTOMER_SCOPE_KEY, CustomerScopeProvider } from './customerScope';

export function customerRow(slug: string, over: Partial<CustomerRow> = {}): CustomerRow {
  return {
    slug,
    name: slug.charAt(0).toUpperCase() + slug.slice(1),
    notes: null,
    contact: null,
    color: null,
    tint: { light: '#2255aa', dark: '#88aaee' },
    archived: false,
    created_at: '2026-10-01T00:00:00Z',
    rollup: {
      projects: 1,
      run_count: 0,
      usage: {
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        total_tokens: 0,
        cost_usd: 0,
        priced: true,
        runs: 0,
      },
      findings_open: 0,
      last_active: null,
    },
    default_account: null,
    ...over,
  };
}

export const ACME = customerRow('acme');

/** Wrap `ui` (already inside a router) in the scope provider. The provider
 *  starts from the router's `?customer=` (jsdom's localStorage is unreliable
 *  under this Node version, so tests scope through the URL:
 *  `<MemoryRouter initialEntries={['/?customer=acme']}>`); any stored scope is
 *  cleared first so an earlier test's never leaks in. */
export function withCustomerScope(
  ui: ReactNode,
  { customers = [ACME] }: { customers?: CustomerRow[] } = {},
): JSX.Element {
  try {
    window.localStorage.removeItem(CUSTOMER_SCOPE_KEY);
  } catch {
    // no storage — nothing to leak
  }
  if (!vi.isMockFunction(api.getCustomers)) vi.spyOn(api, 'getCustomers').mockResolvedValue(customers);
  return <CustomerScopeProvider>{ui}</CustomerScopeProvider>;
}

/** A router entry that starts the scope at `scope`. */
export function scopedEntry(scope: string, path = '/'): string {
  return `${path}?customer=${encodeURIComponent(scope)}`;
}
