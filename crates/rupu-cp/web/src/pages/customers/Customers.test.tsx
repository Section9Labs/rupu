// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { api, ApiError, type CustomerRow, type CustomerScope, type ProjectRow } from '../../lib/api';
import * as scopeLib from '../../lib/customerScope';
import Customers from './Customers';

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  priced: true,
  runs: 0,
};

function row(over: Partial<CustomerRow> & { slug: string; name: string }): CustomerRow {
  return {
    notes: null,
    contact: null,
    color: null,
    tint: { light: '#111111', dark: '#eeeeee' },
    archived: false,
    created_at: '2026-10-01T00:00:00Z',
    rollup: {
      projects: 1,
      run_count: 10,
      usage: { ...USAGE, cost_usd: 10 },
      findings_open: 0,
      last_active: '2026-10-05T00:00:00Z',
    },
    default_account: { account: 'work', locked_by: null, inherited: false },
    ...over,
  };
}

const ACME = row({
  slug: 'acme',
  name: 'Acme Corp',
  default_account: { account: 'acme-prod', locked_by: 'customer', inherited: false },
  rollup: {
    projects: 3,
    run_count: 40,
    usage: { ...USAGE, cost_usd: 30 },
    findings_open: 5,
    last_active: '2026-10-05T00:00:00Z',
  },
});
const GLOBEX = row({
  slug: 'globex',
  name: 'Globex',
  default_account: { account: 'default', locked_by: null, inherited: true },
  rollup: {
    projects: 2,
    run_count: 12,
    usage: { ...USAGE, cost_usd: 10, pricing_error: 'priced at global rates' },
    findings_open: 0,
    last_active: '2026-10-01T00:00:00Z',
    hosts_without_customer: ['mini'],
  },
});
const INITECH = row({
  slug: 'initech',
  name: 'Initech',
  default_account: null,
  layer_error: 'bad toml at line 3',
  rollup: {
    projects: 1,
    run_count: 2,
    usage: { ...USAGE, cost_usd: 0 },
    findings_open: 0,
    last_active: null,
  },
});

function proj(ws: string, customer: ProjectRow['customer'] | undefined): ProjectRow {
  const p = {
    ws_id: ws,
    name: ws,
    path: `/p/${ws}`,
    created_at: '2026-01-01T00:00:00Z',
    usage: USAGE,
    run_count: 0,
    customer,
  } as ProjectRow;
  if (customer === undefined) delete (p as Partial<ProjectRow>).customer;
  return p;
}
const ref = { slug: 'acme', name: 'Acme Corp', tint: ACME.tint, archived: false };
const PROJECTS = [proj('a', ref), proj('b', ref), proj('c', ref), proj('d', ref), proj('e', ref), proj('u1', null), proj('u2', null), proj('u3', undefined)];

let setScope: ReturnType<typeof vi.fn<(n: CustomerScope) => void>>;
let reload: ReturnType<typeof vi.fn<() => void>>;
let getCustomers: ReturnType<typeof vi.spyOn>;

function Where() {
  const l = useLocation();
  return <div data-testid="where">{l.pathname + l.search}</div>;
}

function mount() {
  return render(
    <MemoryRouter initialEntries={['/customers']}>
      <Customers />
      <Where />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  setScope = vi.fn<(n: CustomerScope) => void>();
  reload = vi.fn<() => void>();
  vi.spyOn(scopeLib, 'useCustomerScope').mockReturnValue({
    scope: null,
    customer: null,
    customers: [],
    setScope,
    reload,
    rejectScope: vi.fn(),
    notice: null,
    noticeSeq: 0,
  });
  getCustomers = vi.spyOn(api, 'getCustomers').mockResolvedValue([GLOBEX, ACME, INITECH]);
  vi.spyOn(api, 'getProjects').mockResolvedValue(PROJECTS);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function bodyRows() {
  return screen.getAllByRole('row').slice(1);
}

describe('Customers page', () => {
  it('renders rows, sorted by last active desc', async () => {
    mount();
    await screen.findByText('Acme Corp');
    const rows = bodyRows();
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByText('Acme Corp')).toBeInTheDocument();
    expect(within(rows[1]).getByText('Globex')).toBeInTheDocument();
    expect(within(rows[2]).getByText('Initech')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Acme Corp/ })).toHaveAttribute('href', '/customers/acme');
    expect(getCustomers).toHaveBeenCalledWith({ archived: true, range: '30d' });
  });

  it('default account: lock when locked, "inherits global", layer error note', async () => {
    mount();
    await screen.findByText('Acme Corp');
    const [acme, globex, initech] = bodyRows();
    expect(within(acme).getByText('acme-prod')).toBeInTheDocument();
    expect(within(acme).getByLabelText(/Locked by customer/)).toBeInTheDocument();
    expect(within(globex).getByText('inherits global')).toBeInTheDocument();
    expect(within(globex).queryByLabelText(/Locked by/)).toBeNull();
    const err = within(initech).getByText('layer error');
    expect(err).toHaveAttribute('title', 'bad toml at line 3');
  });

  it('shows the pricing mark on a cost cell and err tone for open findings', async () => {
    mount();
    await screen.findByText('Acme Corp');
    const [acme, globex] = bodyRows();
    expect(within(globex).getByRole('img', { name: /priced at global rates/ })).toBeInTheDocument();
    expect(within(acme).queryByRole('img', { name: /Pricing unavailable/ })).toBeNull();
    expect(within(acme).getByText('5').className).toContain('text-err');
  });

  it('computes the four tiles', async () => {
    mount();
    await screen.findByText('Acme Corp');
    const tile = (id: string) => screen.getByTestId(`tile-${id}`);
    expect(within(tile('customers')).getByText('3')).toBeInTheDocument();
    expect(within(tile('customers')).getByText('0 archived')).toBeInTheDocument();
    expect(within(tile('assigned')).getByText('5 / 8')).toBeInTheDocument();
    expect(within(tile('assigned')).getByText('2 unassigned')).toBeInTheDocument();
    expect(within(tile('cost')).getByText('$40.00')).toBeInTheDocument();
    expect(within(tile('cost')).getByText('75% from Acme Corp')).toBeInTheDocument();
    expect(within(tile('findings')).getByText('5')).toBeInTheDocument();
    expect(within(tile('findings')).getByText('across 1 customer')).toBeInTheDocument();
  });

  it('says the cost total excludes a customer whose usage is unpriced', async () => {
    const UNPRICED = row({
      slug: 'globex',
      name: 'Globex',
      rollup: {
        projects: 1,
        run_count: 6,
        usage: { ...USAGE, cost_usd: null, priced: false, runs: 6 },
        findings_open: 0,
        last_active: null,
      },
    });
    getCustomers.mockResolvedValue([ACME, UNPRICED]);
    mount();
    await screen.findByText('Acme Corp');
    const cost = screen.getByTestId('tile-cost');
    expect(within(cost).getByText('$30.00')).toBeInTheDocument();
    expect(within(cost).getByText('excludes unpriced usage from Globex')).toBeInTheDocument();
    expect(within(cost).queryByText(/% from/)).toBeNull();
  });

  it('counts the customers when several have unpriced usage', async () => {
    const mk = (slug: string, name: string) =>
      row({
        slug,
        name,
        rollup: {
          projects: 1,
          run_count: 2,
          usage: { ...USAGE, cost_usd: 5, priced: true, partial: true, runs: 2 },
          findings_open: 0,
          last_active: null,
        },
      });
    getCustomers.mockResolvedValue([ACME, mk('a', 'Aaa'), mk('b', 'Bbb')]);
    mount();
    await screen.findByText('Acme Corp');
    expect(
      within(screen.getByTestId('tile-cost')).getByText('excludes unpriced usage from 2 customers'),
    ).toBeInTheDocument();
  });

  it('status views are derived from one list: no refetch, archived count on Active', async () => {
    const OLD = row({ slug: 'old', name: 'Old Co', archived: true });
    getCustomers.mockResolvedValue([ACME, OLD]);
    mount();
    await screen.findByText('Acme Corp');
    expect(screen.queryByText('Old Co')).toBeNull();
    expect(within(screen.getByTestId('tile-customers')).getByText('1 archived')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Archived' }));
    expect(screen.getByText('Old Co')).toBeInTheDocument();
    expect(screen.queryByText('Acme Corp')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'All' }));
    expect(screen.getByText('Acme Corp')).toBeInTheDocument();
    expect(screen.getByText('Old Co')).toBeInTheDocument();
    expect(getCustomers).toHaveBeenCalledTimes(1);
  });

  it('range refetches with range, showing loading instead of stale rows', async () => {
    mount();
    await screen.findByText('Acme Corp');
    let resolve!: (r: CustomerRow[]) => void;
    getCustomers.mockReturnValue(new Promise((r) => (resolve = r)));
    fireEvent.click(screen.getByRole('button', { name: '7d' }));
    expect(getCustomers).toHaveBeenLastCalledWith({ archived: true, range: '7d' });
    expect(screen.queryByText('Acme Corp')).toBeNull();
    expect(screen.getByText('Loading customers…')).toBeInTheDocument();
    resolve([GLOBEX]);
    await screen.findByText('Cost · 7d');
    expect(screen.getByText('Globex')).toBeInTheDocument();
    expect(screen.queryByText('Acme Corp')).toBeNull();
  });

  it('filter narrows rows by name or slug', async () => {
    mount();
    await screen.findByText('Acme Corp');
    fireEvent.change(screen.getByPlaceholderText('Filter customers…'), { target: { value: 'glob' } });
    expect(bodyRows()).toHaveLength(1);
    expect(screen.getByText('Globex')).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText('Filter customers…'), { target: { value: 'zzz' } });
    expect(screen.getByText(/No customers match/)).toBeInTheDocument();
    // The unassigned footer stays even with no table.
    expect(screen.getByRole('link', { name: /Review unassigned/ })).toBeInTheDocument();
  });

  it('footer counts unassigned projects and its link sets scope none', async () => {
    mount();
    await screen.findByText('Acme Corp');
    expect(screen.getByText(/2 projects have no customer/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('link', { name: /Review unassigned/ }));
    expect(setScope).toHaveBeenCalledWith('none');
    expect(screen.getByTestId('where')).toHaveTextContent('/projects');
  });

  it('names hosts whose runs are left out in the warn banner, by their registered name', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([{ id: 'mini', name: 'Mini studio', transport_kind: 'ssh' }]);
    mount();
    await screen.findByText('Acme Corp');
    await waitFor(() =>
      expect(screen.getByTestId('hosts-without-customer')).toHaveTextContent(
        /^Mini studio runs an older rupu .* can’t tag every run with a customer — those runs are left out of this view, not counted as zero/,
      ),
    );
  });

  it('falls back to the host id when the registered hosts can’t be read', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('down'));
    mount();
    await screen.findByText('Acme Corp');
    expect(screen.getByTestId('hosts-without-customer')).toHaveTextContent(/^mini runs an older rupu/);
  });

  it('empty state offers New customer, which opens the dialog', async () => {
    getCustomers.mockResolvedValue([]);
    mount();
    expect(await screen.findByText('No customers yet')).toBeInTheDocument();
    const buttons = screen.getAllByRole('button', { name: 'New customer' });
    fireEvent.click(buttons[buttons.length - 1]);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('shows an error banner when loading fails', async () => {
    getCustomers.mockRejectedValue(new ApiError(500, 'x', '{"error":"unreadable assignment"}'));
    mount();
    expect(await screen.findByRole('alert')).toHaveTextContent('unreadable assignment');
  });

  it('a created customer reloads the list', async () => {
    vi.spyOn(api, 'createCustomer').mockResolvedValue(ACME);
    mount();
    await screen.findByText('Acme Corp');
    fireEvent.click(screen.getByRole('button', { name: 'New customer' }));
    fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: 'Newco' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create customer' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(getCustomers.mock.calls.length).toBeGreaterThanOrEqual(2);
  });
});
