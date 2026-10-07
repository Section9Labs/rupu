// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type CustomerRow } from './api';
import {
  CUSTOMER_SCOPE_KEY,
  CustomerScopeProvider,
  useCustomerParam,
  useCustomerScope,
} from './customerScope';

function row(slug: string, archived = false): CustomerRow {
  return {
    slug,
    name: slug.toUpperCase(),
    notes: null,
    contact: null,
    color: null,
    tint: { light: '#111111', dark: '#eeeeee' },
    archived,
    created_at: '2026-10-01T00:00:00Z',
    rollup: {
      projects: 0,
      run_count: 0,
      usage: {} as CustomerRow['rollup']['usage'],
      findings_open: 0,
      last_active: null,
    },
    default_account: null,
  };
}

let ctx: ReturnType<typeof useCustomerScope>;
let param: ReturnType<typeof useCustomerParam>;
function Probe() {
  ctx = useCustomerScope();
  param = useCustomerParam();
  return (
    <div>
      <span data-testid="scope">{String(ctx.scope)}</span>
      <span data-testid="notice">{ctx.notice ?? ''}</span>
      <span data-testid="customers">{ctx.customers.map((c) => c.slug).join(',')}</span>
      <span data-testid="customer">{ctx.customer?.slug ?? ''}</span>
    </div>
  );
}

function mount(url = '/') {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <CustomerScopeProvider>
        <Probe />
      </CustomerScopeProvider>
    </MemoryRouter>,
  );
}

// jsdom's localStorage is unreliable under this Node version — install an
// in-memory one (same approach as WorkflowEditor.test.tsx).
function installLocalStorage(throwOnSet = false) {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => {
      if (throwOnSet) throw new Error('quota');
      store.set(k, String(v));
    },
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
  });
}

beforeEach(() => {
  installLocalStorage();
  vi.spyOn(api, 'getCustomers').mockResolvedValue([row('acme'), row('globex')]);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('CustomerScopeProvider', () => {
  it('defaults to all customers and loads the list once', async () => {
    mount();
    await waitFor(() => expect(screen.getByTestId('customers').textContent).toBe('acme,globex'));
    expect(screen.getByTestId('scope').textContent).toBe('null');
    expect(param).toBeNull();
    expect(api.getCustomers).toHaveBeenCalledTimes(1);
  });

  it('adopts ?customer= from the URL and persists it', async () => {
    mount('/activity?customer=acme');
    await waitFor(() => expect(screen.getByTestId('customer').textContent).toBe('acme'));
    expect(screen.getByTestId('scope').textContent).toBe('acme');
    expect(window.localStorage.getItem(CUSTOMER_SCOPE_KEY)).toBe('acme');
    expect(param).toBe('acme');
  });

  it('reads the stored scope when the URL has none', async () => {
    window.localStorage.setItem(CUSTOMER_SCOPE_KEY, 'globex');
    mount();
    await waitFor(() => expect(screen.getByTestId('customer').textContent).toBe('globex'));
  });

  it('setScope persists, and null clears the stored value', async () => {
    mount();
    await waitFor(() => expect(screen.getByTestId('customers').textContent).toBe('acme,globex'));
    act(() => ctx.setScope('globex'));
    expect(screen.getByTestId('scope').textContent).toBe('globex');
    expect(window.localStorage.getItem(CUSTOMER_SCOPE_KEY)).toBe('globex');
    act(() => ctx.setScope(null));
    expect(window.localStorage.getItem(CUSTOMER_SCOPE_KEY)).toBeNull();
  });

  it("keeps 'none' without a customer row", async () => {
    window.localStorage.setItem(CUSTOMER_SCOPE_KEY, 'none');
    mount();
    await waitFor(() => expect(screen.getByTestId('customers').textContent).toBe('acme,globex'));
    expect(screen.getByTestId('scope').textContent).toBe('none');
    expect(screen.getByTestId('customer').textContent).toBe('');
    expect(screen.getByTestId('notice').textContent).toBe('');
  });

  it('clears a stored slug that no longer exists and sets a notice', async () => {
    window.localStorage.setItem(CUSTOMER_SCOPE_KEY, 'gone');
    mount();
    await waitFor(() => expect(screen.getByTestId('scope').textContent).toBe('null'));
    expect(screen.getByTestId('notice').textContent).toBe(
      'Customer “gone” no longer exists — showing all customers.',
    );
    expect(window.localStorage.getItem(CUSTOMER_SCOPE_KEY)).toBeNull();
  });

  it('keeps an archived customer that the active list omits', async () => {
    window.localStorage.setItem(CUSTOMER_SCOPE_KEY, 'old');
    (api.getCustomers as unknown as ReturnType<typeof vi.fn>).mockImplementation(
      async (opts?: { archived?: boolean }) =>
        opts?.archived ? [row('acme'), row('old', true)] : [row('acme')],
    );
    mount();
    await waitFor(() => expect(screen.getByTestId('customer').textContent).toBe('old'));
    expect(screen.getByTestId('scope').textContent).toBe('old');
    expect(screen.getByTestId('notice').textContent).toBe('');
  });

  it('keeps the scope when the customer list fails to load', async () => {
    window.localStorage.setItem(CUSTOMER_SCOPE_KEY, 'acme');
    (api.getCustomers as unknown as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('boom'));
    mount();
    await waitFor(() => expect(api.getCustomers).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.getByTestId('scope').textContent).toBe('acme');
    expect(screen.getByTestId('customers').textContent).toBe('');
    expect(screen.getByTestId('notice').textContent).toBe('');
  });

  it('reload() refetches the list', async () => {
    mount();
    await waitFor(() => expect(screen.getByTestId('customers').textContent).toBe('acme,globex'));
    (api.getCustomers as unknown as ReturnType<typeof vi.fn>).mockResolvedValue([row('acme')]);
    act(() => ctx.reload());
    await waitFor(() => expect(screen.getByTestId('customers').textContent).toBe('acme'));
  });

  it('survives storage that throws', async () => {
    installLocalStorage(true);
    mount();
    await waitFor(() => expect(screen.getByTestId('customers').textContent).toBe('acme,globex'));
    act(() => ctx.setScope('acme'));
    expect(screen.getByTestId('scope').textContent).toBe('acme');
  });
});

describe('useCustomerScope outside a provider', () => {
  it('throws a clear error', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => render(<Probe />)).toThrow(/CustomerScopeProvider/);
    spy.mockRestore();
  });
});
