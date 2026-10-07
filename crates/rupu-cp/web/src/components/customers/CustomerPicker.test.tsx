// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type CustomerRow } from '../../lib/api';
import { CustomerScopeProvider } from '../../lib/customerScope';
import { ThemeProvider } from '../theme/ThemeProvider';
import { CustomerPicker } from './CustomerPicker';

function row(slug: string, name: string, projects = 1, archived = false): CustomerRow {
  return {
    slug,
    name,
    notes: null,
    contact: null,
    color: null,
    tint: { light: '#111111', dark: '#eeeeee' },
    archived,
    created_at: '2026-10-01T00:00:00Z',
    rollup: {
      projects,
      run_count: 0,
      usage: {} as CustomerRow['rollup']['usage'],
      findings_open: 0,
      last_active: null,
    },
    default_account: null,
  };
}

const ACME = row('acme', 'Acme Corp', 3);
const GLOBEX = row('globex', 'Globex', 2);
const OLD = row('old', 'Old Co', 1, true);

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

let getCustomers: ReturnType<typeof vi.spyOn>;

function mount(variant?: 'sidebar' | 'compact') {
  return render(
    <ThemeProvider>
      <MemoryRouter>
        <CustomerScopeProvider>
          <CustomerPicker variant={variant} />
        </CustomerScopeProvider>
      </MemoryRouter>
    </ThemeProvider>,
  );
}

async function open() {
  const trigger = await screen.findByRole('button', { name: /customer scope/i });
  // wait for the list to load so rows exist
  await waitFor(() => expect(getCustomers).toHaveBeenCalled());
  fireEvent.click(trigger);
  return trigger;
}

beforeEach(() => {
  installLocalStorage();
  installMatchMedia();
  getCustomers = vi.spyOn(api, 'getCustomers').mockImplementation(async (opts) =>
    opts?.archived ? [ACME, GLOBEX, OLD] : [ACME, GLOBEX],
  );
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('CustomerPicker', () => {
  it('shows "All customers" and lists the customers with project counts when opened', async () => {
    mount();
    const trigger = await open();
    expect(trigger).toHaveTextContent('All customers');
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    expect(within(menu).getByText('Globex')).toBeInTheDocument();
    expect(within(menu).getByText('Unassigned')).toBeInTheDocument();
    expect(within(menu).getByRole('menuitem', { name: /Acme Corp/ })).toHaveTextContent('3');
    // All customers = sum of the rows' project counts
    expect(within(menu).getByRole('menuitem', { name: /All customers/ })).toHaveTextContent('5');
  });

  it('typing filters the rows', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    fireEvent.change(screen.getByPlaceholderText('Find a customer…'), { target: { value: 'glob' } });
    expect(within(menu).queryByText('Acme Corp')).toBeNull();
    expect(within(menu).getByText('Globex')).toBeInTheDocument();
  });

  it('picking Acme sets the scope, closes the menu and the trigger shows the name', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    fireEvent.click(within(menu).getByRole('menuitem', { name: /Acme Corp/ }));
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByRole('button', { name: /customer scope/i })).toHaveTextContent('Acme Corp');
    expect(localStorage.getItem('rupu.cp.customer')).toBe('acme');
  });

  it('"Unassigned" sets the scope to none', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    fireEvent.click(within(menu).getByRole('menuitem', { name: /Unassigned/ }));
    expect(localStorage.getItem('rupu.cp.customer')).toBe('none');
    expect(screen.getByRole('button', { name: /customer scope/i })).toHaveTextContent('Unassigned');
  });

  it('"All customers" clears the scope', async () => {
    localStorage.setItem('rupu.cp.customer', 'acme');
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    fireEvent.click(within(menu).getByRole('menuitem', { name: /All customers/ }));
    expect(localStorage.getItem('rupu.cp.customer')).toBeNull();
    expect(screen.getByRole('button', { name: /customer scope/i })).toHaveTextContent('All customers');
  });

  it('marks the current customer with a check', async () => {
    localStorage.setItem('rupu.cp.customer', 'acme');
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    await waitFor(() =>
      expect(within(menu).getByRole('menuitem', { name: /Acme Corp/ })).toHaveAttribute('aria-current', 'true'),
    );
    expect(within(menu).getByRole('menuitem', { name: /Globex/ })).not.toHaveAttribute('aria-current');
  });

  it('Escape closes the menu and returns focus to the trigger', async () => {
    mount();
    const trigger = await open();
    await screen.findByRole('menu');
    fireEvent.keyDown(screen.getByPlaceholderText('Find a customer…'), { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('arrow keys move through the rows and Enter picks', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    const search = screen.getByPlaceholderText('Find a customer…');
    // rows: All customers (0), Acme (1), Globex (2), Unassigned (3)
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    fireEvent.keyDown(search, { key: 'ArrowUp' });
    fireEvent.keyDown(search, { key: 'Enter' });
    expect(screen.getByRole('button', { name: /customer scope/i })).toHaveTextContent('Acme Corp');
  });

  it('"Show archived" fetches archived customers and lists them muted', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    expect(within(menu).queryByText('Old Co')).toBeNull();
    fireEvent.click(screen.getByLabelText('Show archived'));
    await waitFor(() => expect(getCustomers).toHaveBeenCalledWith({ archived: true }));
    await waitFor(() => expect(within(menu).getByText('Old Co')).toBeInTheDocument());
    expect(within(menu).getByRole('menuitem', { name: /Old Co/ }).className).toMatch(/text-ink-mute/);
  });

  it('"Manage →" links to /customers', async () => {
    mount();
    await open();
    await screen.findByRole('menu');
    expect(screen.getByRole('link', { name: /Manage/ })).toHaveAttribute('href', '/customers');
  });

  it('the compact variant has no CUSTOMER label', async () => {
    mount('compact');
    await screen.findByRole('button', { name: /customer scope/i });
    expect(screen.queryByText('Customer')).toBeNull();
  });

  it('Escape works from a row, the checkbox and the Manage link, and returns focus to the trigger', async () => {
    mount();
    const trigger = await open();
    let menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    for (const get of [
      () => within(menu).getByRole('menuitem', { name: /Acme Corp/ }),
      () => screen.getByLabelText('Show archived'),
      () => screen.getByRole('link', { name: /Manage/ }),
    ]) {
      const el = get();
      el.focus();
      fireEvent.keyDown(el, { key: 'Escape' });
      expect(screen.queryByRole('menu')).toBeNull();
      expect(trigger).toHaveFocus();
      fireEvent.click(trigger);
      menu = await screen.findByRole('menu');
    }
  });

  it('closes when focus moves to an element outside the picker', async () => {
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    mount();
    await open();
    await screen.findByRole('menu');
    const search = screen.getByPlaceholderText('Find a customer…');
    act(() => search.focus());
    act(() => outside.focus());
    expect(screen.queryByRole('menu')).toBeNull();
    outside.remove();
  });

  it('does not close when focus moves between elements inside the picker', async () => {
    mount();
    await open();
    await screen.findByRole('menu');
    act(() => screen.getByPlaceholderText('Find a customer…').focus());
    act(() => screen.getByLabelText('Show archived').focus());
    expect(screen.getByRole('menu')).toBeInTheDocument();
  });

  it('announces the active row with aria-activedescendant', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    const search = screen.getByPlaceholderText('Find a customer…');
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    const id = search.getAttribute('aria-activedescendant');
    expect(id).toBeTruthy();
    expect(document.getElementById(id!)).toHaveTextContent('Acme Corp');
  });

  it('scrolls the active row into view', async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    try {
      mount();
      await open();
      await screen.findByRole('menu');
      scroll.mockClear();
      fireEvent.keyDown(screen.getByPlaceholderText('Find a customer…'), { key: 'ArrowDown' });
      expect(scroll).toHaveBeenCalledWith({ block: 'nearest' });
    } finally {
      delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
    }
  });

  it('an empty filtered list never sets a negative active index', async () => {
    mount();
    await open();
    const menu = await screen.findByRole('menu');
    await waitFor(() => expect(within(menu).getByText('Acme Corp')).toBeInTheDocument());
    const search = screen.getByPlaceholderText('Find a customer…');
    fireEvent.change(search, { target: { value: 'zzz' } });
    expect(within(menu).getByText('No matching customer.')).toBeInTheDocument();
    expect(search).not.toHaveAttribute('aria-activedescendant');
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    fireEvent.keyDown(search, { key: 'ArrowDown' });
    fireEvent.keyDown(search, { key: 'ArrowUp' });
    fireEvent.keyDown(search, { key: 'Enter' }); // nothing to pick
    expect(screen.getByRole('menu')).toBeInTheDocument();
    // clearing the filter lands on the first row, not on index -1
    fireEvent.change(search, { target: { value: '' } });
    expect(document.getElementById(search.getAttribute('aria-activedescendant')!)).toHaveTextContent(
      'All customers',
    );
    fireEvent.keyDown(search, { key: 'Enter' });
    expect(screen.getByRole('button', { name: /customer scope/i })).toHaveTextContent('All customers');
  });

  it('refetches archived customers every time "Show archived" is ticked', async () => {
    mount();
    await open();
    await screen.findByRole('menu');
    const box = screen.getByLabelText('Show archived');
    const archivedCalls = () => getCustomers.mock.calls.filter((c: unknown[]) => (c[0] as { archived?: boolean } | undefined)?.archived).length;
    fireEvent.click(box);
    await waitFor(() => expect(archivedCalls()).toBe(1));
    fireEvent.click(box);
    fireEvent.click(box);
    await waitFor(() => expect(archivedCalls()).toBe(2));
  });

  it('clears the archived error when a refetch starts and succeeds', async () => {
    let fail = true;
    getCustomers.mockImplementation(async (opts?: { archived?: boolean }) => {
      if (opts?.archived) {
        if (fail) throw new Error('boom');
        return [ACME, GLOBEX, OLD];
      }
      return [ACME, GLOBEX];
    });
    mount();
    await open();
    await screen.findByRole('menu');
    const box = screen.getByLabelText('Show archived');
    fireEvent.click(box);
    expect(await screen.findByText(/Couldn’t load archived customers/)).toBeInTheDocument();
    fail = false;
    fireEvent.click(box);
    fireEvent.click(box);
    await waitFor(() => expect(screen.queryByText(/Couldn’t load archived customers/)).toBeNull());
    expect(await screen.findByText('Old Co')).toBeInTheDocument();
  });
});
