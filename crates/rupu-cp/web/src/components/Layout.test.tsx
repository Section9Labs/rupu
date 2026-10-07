// @vitest-environment jsdom
// v1 shell: the customer picker sits under the brand, the Customers leaf is in
// the nav, and a rejected-scope notice shows above the page.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api } from '../lib/api';
import { CustomerScopeProvider } from '../lib/customerScope';
import { ThemeProvider } from './theme/ThemeProvider';
import Layout from './Layout';

function installLocalStorage() {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => store.set(k, String(v)),
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
  });
}

function installMatchMedia() {
  vi.stubGlobal(
    'matchMedia',
    vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => true,
    })),
  );
}

function renderLayout() {
  return render(
    <ThemeProvider>
      <MemoryRouter initialEntries={['/dashboard']}>
        <CustomerScopeProvider>
          <Routes>
            <Route element={<Layout />}>
              <Route path="*" element={<div>body</div>} />
            </Route>
          </Routes>
        </CustomerScopeProvider>
      </MemoryRouter>
    </ThemeProvider>,
  );
}

beforeEach(() => {
  installLocalStorage();
  installMatchMedia();
  vi.spyOn(api, 'getCustomers').mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Layout (v1 shell)', () => {
  it('shows the CUSTOMER picker under the brand and a Customers nav link', async () => {
    renderLayout();
    expect(await screen.findByRole('button', { name: /customer scope/i })).toHaveTextContent('All customers');
    expect(screen.getByText('Customer')).toBeInTheDocument();
    const nav = screen.getByRole('navigation');
    expect(within(nav).getByRole('link', { name: 'Customers' })).toHaveAttribute('href', '/customers');
  });

  it('shows a dismissible notice for a rejected stored scope', async () => {
    localStorage.setItem('rupu.cp.customer', 'ghost');
    renderLayout();
    const notice = await screen.findByRole('status');
    expect(notice).toHaveTextContent('ghost');
    fireEvent.click(within(notice).getByRole('button', { name: /dismiss/i }));
    expect(screen.queryByRole('status')).toBeNull();
  });
});
