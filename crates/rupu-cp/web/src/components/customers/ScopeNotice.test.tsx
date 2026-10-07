// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import { CustomerScopeProvider, useCustomerScope } from '../../lib/customerScope';
import { ScopeNotice } from './ScopeNotice';

let ctx: ReturnType<typeof useCustomerScope>;
function Probe() {
  ctx = useCustomerScope();
  return null;
}

beforeEach(() => {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => store.set(k, String(v)),
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
  });
  vi.spyOn(api, 'getCustomers').mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('ScopeNotice', () => {
  it('shows a repeated identical notice again after it was dismissed', async () => {
    render(
      <MemoryRouter>
        <CustomerScopeProvider>
          <Probe />
          <ScopeNotice />
        </CustomerScopeProvider>
      </MemoryRouter>,
    );
    expect(screen.queryByRole('status')).toBeNull();

    act(() => ctx.rejectScope('Customer “acme” no longer exists.'));
    const first = screen.getByRole('status');
    expect(first).toHaveTextContent('acme');
    fireEvent.click(within(first).getByRole('button', { name: /dismiss/i }));
    expect(screen.queryByRole('status')).toBeNull();

    // the very same text, a new occurrence
    act(() => ctx.rejectScope('Customer “acme” no longer exists.'));
    expect(screen.getByRole('status')).toHaveTextContent('acme');
  });
});
