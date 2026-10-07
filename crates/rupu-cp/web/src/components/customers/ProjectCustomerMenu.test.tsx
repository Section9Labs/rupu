// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { act, render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { useState } from 'react';
import { api, ApiError, type ConfigView, type CustomerRef, type ProjectRow } from '../../lib/api';
import { customerRow, withCustomerScope } from '../../lib/customerScopeTestUtils';
import { ProjectCustomerMenu } from './ProjectCustomerMenu';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ACME = customerRow('acme', {
  default_account: { account: 'anthropic-acme', locked_by: 'customer', inherited: false },
});
const GLOBEX = customerRow('globex', {
  default_account: { account: 'anthropic-main', locked_by: null, inherited: true },
});
const INITECH = customerRow('initech', {
  default_account: { account: 'anthropic-initech', locked_by: null, inherited: false },
});
const ref = (c: { slug: string; name: string; tint: CustomerRef['tint'] }): CustomerRef => ({
  slug: c.slug,
  name: c.name,
  tint: c.tint,
  archived: false,
});

function projectRow(customer: CustomerRef | null): ProjectRow {
  return {
    ws_id: 'ws1',
    name: 'Svc',
    path: '/srv/svc',
    created_at: '2026-10-01T00:00:00Z',
    usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: 0, priced: true, runs: 0 },
    run_count: 0,
    customer,
  };
}

function Harness({ initial }: { initial: CustomerRef | null | undefined }) {
  const [c, setC] = useState<CustomerRef | null | undefined>(initial);
  return <ProjectCustomerMenu wsId="ws1" customer={c} onChange={setC} />;
}

/** The project's own config view: `default_provider` from `source`. */
function projectView(source: 'global' | 'customer' | 'project', value = 'anthropic-personal', locked = false): ConfigView {
  return {
    effective: { default_provider: value },
    provenance: { default_provider: { source, locked, ...(locked ? { locked_by: 'customer' } : {}) } },
    raw_global: '',
    raw_project: null,
  } as unknown as ConfigView;
}

function mount(
  initial: CustomerRef | null | undefined,
  customers = [ACME, GLOBEX, INITECH],
  project: ConfigView | Error = projectView('global', 'anthropic-main'),
) {
  vi.spyOn(api, 'getCustomers').mockResolvedValue(customers);
  if (!vi.isMockFunction(api.getConfig)) {
    vi.spyOn(api, 'getConfig').mockImplementation(async () => {
      if (project instanceof Error) throw project;
      return project;
    });
  }
  const utils = render(<MemoryRouter>{withCustomerScope(<Harness initial={initial} />, { customers })}</MemoryRouter>);
  return utils;
}

const allItems = () =>
  Array.from(document.querySelectorAll<HTMLElement>('[role="menuitem"], [role="menuitemradio"]'));

async function open() {
  fireEvent.click(screen.getByRole('button', { name: /customer/i }));
  await screen.findByRole('menu', { name: 'Assign to customer' });
}

describe('ProjectCustomerMenu', () => {
  it('shows a dashed No customer trigger and lists the active customers with their default account', async () => {
    mount(null);
    expect(screen.getByText('No customer')).toBeInTheDocument();
    await open();
    const items = await screen.findAllByRole('menuitemradio');
    const names = items.map((i) => i.textContent);
    expect(names.some((t) => t?.includes('Acme') && t.includes('anthropic-acme'))).toBe(true);
    expect(names.some((t) => t?.includes('Globex'))).toBe(true);
    expect(screen.getByRole('menuitem', { name: /new customer/i })).toBeInTheDocument();
    // not assigned → no Unassign
    expect(screen.queryByRole('menuitem', { name: 'Unassign' })).toBeNull();
  });

  it('shows the muted unknown state when the CP cannot say', () => {
    mount(undefined);
    expect(screen.getByText('Unknown customer')).toBeInTheDocument();
    expect(screen.queryByText('No customer')).toBeNull();
  });

  it('previews the account switch on focus, with its tag', async () => {
    mount(null);
    await open();
    await waitFor(() => expect(api.getConfig).toHaveBeenCalledWith('ws1'));
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    // A locked key: the project can't override it.
    expect(
      screen.getByText(
        'Assigning to Acme: new runs use anthropic-acme (locked by Acme, so the project can’t override it). An agent that names its own provider still uses it. Runs already finished keep their history.',
      ),
    ).toBeInTheDocument();
    fireEvent.focus(screen.getByRole('menuitemradio', { name: /Globex/ }));
    // Unlocked, and the project's own config doesn't set default_provider.
    expect(
      await screen.findByText(
        'Assigning to Globex: new runs default to anthropic-main (inherits global). An agent that names its own provider still uses it. Runs already finished keep their history.',
      ),
    ).toBeInTheDocument();
    fireEvent.focus(screen.getByRole('menuitemradio', { name: /Initech/ }));
    expect(screen.getByText(/new runs default to anthropic-initech \(customer default\)\./)).toBeInTheDocument();
  });

  it('says the project keeps its own account when its config sets default_provider and the customer does not lock it', async () => {
    mount(null, [ACME, GLOBEX, INITECH], projectView('project', 'anthropic-personal'));
    await open();
    await waitFor(() => expect(api.getConfig).toHaveBeenCalledWith('ws1'));
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Initech/ }));
    expect(
      await screen.findByText(
        /its default account is anthropic-initech \(customer default\), but this project’s own config sets default_provider = anthropic-personal, so new runs keep that\./,
      ),
    ).toBeInTheDocument();
    // A lock still wins over the project's own value.
    fireEvent.focus(screen.getByRole('menuitemradio', { name: /Acme/ }));
    expect(screen.getByText(/new runs use anthropic-acme \(locked by Acme, so the project can’t override it\)/)).toBeInTheDocument();
  });

  it('hedges when it cannot tell whether the project sets default_provider', async () => {
    mount(null, [ACME, GLOBEX, INITECH], new Error('boom'));
    await open();
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Initech/ }));
    expect(
      screen.getByText(
        'Assigning to Initech: new runs default to anthropic-initech (customer default) unless this project’s own config sets default_provider. An agent that names its own provider still uses it. Runs already finished keep their history.',
      ),
    ).toBeInTheDocument();
  });

  it('hedges when a lock above the project could hide its own value', async () => {
    mount(ref(GLOBEX), [ACME, GLOBEX, INITECH], projectView('customer', 'anthropic-main', true));
    await open();
    await waitFor(() => expect(api.getConfig).toHaveBeenCalledWith('ws1'));
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Initech/ }));
    expect(screen.getByText(/unless this project’s own config sets default_provider/)).toBeInTheDocument();
  });

  it('adds SCM routing to the preview only when the customer layer sets scm.rules', async () => {
    const view = {
      effective: { scm: { rules: [{ owner: 'acme-corp', account: 'github-acme' }] } },
      provenance: { 'scm.rules': { source: 'customer', locked: false } },
      raw_global: '',
      raw_project: null,
    } as unknown as ConfigView;
    const spy = vi.spyOn(api, 'getCustomerConfig').mockImplementation(async (slug) =>
      slug === 'acme' ? view : ({ ...view, provenance: {}, raw_customer: null } as ConfigView),
    );
    mount(null);
    await open();
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    expect(await screen.findByText(/, and its SCM rules route acme-corp\/\* repos to github-acme\./)).toBeInTheDocument();
    expect(spy).toHaveBeenCalledWith('acme');
    fireEvent.focus(screen.getByRole('menuitemradio', { name: /Globex/ }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith('globex'));
    expect(screen.queryByText(/routes/)).toBeNull();
  });

  it('does not claim SCM routing from raw TOML when provenance says the rules are not the customer’s', async () => {
    const view = {
      effective: { scm: { rules: [{ owner: 'global-org', account: 'github-main' }] } },
      provenance: { 'scm.rules': { source: 'global', locked: false } },
      raw_global: '',
      raw_project: null,
      raw_customer: '# mentions scm.rules in a comment\n',
    } as unknown as ConfigView;
    const spy = vi.spyOn(api, 'getCustomerConfig').mockResolvedValue(view);
    mount(null);
    await open();
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    await waitFor(() => expect(spy).toHaveBeenCalled());
    await Promise.resolve();
    expect(screen.queryByText(/routes/)).toBeNull();
  });

  it('omits SCM routing when the config does not load', async () => {
    vi.spyOn(api, 'getCustomerConfig').mockRejectedValue(new Error('boom'));
    mount(null);
    await open();
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    expect(screen.getByText(/Assigning to Acme/)).toBeInTheDocument();
    expect(screen.queryByText(/routes/)).toBeNull();
  });

  it('assigning calls the API and updates the chip', async () => {
    const spy = vi.spyOn(api, 'assignProject').mockResolvedValue(projectRow(ref(GLOBEX)));
    mount(ref(ACME));
    await open();
    expect(screen.getByRole('menuitemradio', { name: /Acme/ })).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(await screen.findByRole('menuitemradio', { name: /Globex/ }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith('globex', 'ws1'));
    await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
    expect(screen.getByRole('button', { name: /Customer: Globex/ })).toBeInTheDocument();
  });

  it('shows a refusal inline and leaves the chip alone', async () => {
    vi.spyOn(api, 'assignProject').mockRejectedValue(new ApiError(409, 'customer is archived'));
    mount(null);
    await open();
    fireEvent.click(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/archived/);
    expect(screen.getByRole('menu')).toBeInTheDocument();
    expect(screen.getByText('No customer')).toBeInTheDocument();
  });

  it('unassigns and clears the chip', async () => {
    const spy = vi.spyOn(api, 'unassignProject').mockResolvedValue(undefined);
    mount(ref(ACME));
    await open();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Unassign' }));
    await waitFor(() => expect(spy).toHaveBeenCalledWith('acme', 'ws1'));
    await waitFor(() => expect(screen.getByText('No customer')).toBeInTheDocument());
  });

  it('says "Already unassigned" on a 404 and reconciles the chip', async () => {
    vi.spyOn(api, 'unassignProject').mockRejectedValue(new ApiError(404, 'not assigned'));
    mount(ref(ACME));
    await open();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Unassign' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Already unassigned');
    expect(screen.getByText('No customer')).toBeInTheDocument();
  });

  it('"New customer…" creates then assigns the new customer', async () => {
    const dto = { ...customerRow('hooli'), notes: null };
    const create = vi.spyOn(api, 'createCustomer').mockResolvedValue(dto);
    const assign = vi.spyOn(api, 'assignProject').mockResolvedValue(projectRow(ref(dto)));
    mount(null);
    await open();
    fireEvent.click(screen.getByRole('menuitem', { name: /new customer/i }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.change(within(dialog).getByLabelText(/^name/i), { target: { value: 'Hooli' } });
    fireEvent.click(within(dialog).getByRole('button', { name: /create|save/i }));
    await waitFor(() => expect(create).toHaveBeenCalled());
    await waitFor(() => expect(assign).toHaveBeenCalledWith('hooli', 'ws1'));
    await waitFor(() => expect(screen.getByRole('button', { name: /Customer: Hooli/ })).toBeInTheDocument());
  });

  it('Escape closes the menu and returns focus to the trigger', async () => {
    mount(null);
    await open();
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(screen.getByRole('button', { name: /customer/i })).toHaveFocus();
  });

  it('arrow keys move between items', async () => {
    mount(null);
    await open();
    const items = allItems();
    act(() => items[0].focus());
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'ArrowDown' });
    expect(items[1]).toHaveFocus();
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'ArrowUp' });
    expect(items[0]).toHaveFocus();
  });

  it('keeps the note and the error outside the menu element', async () => {
    vi.spyOn(api, 'assignProject').mockRejectedValue(new ApiError(409, 'customer is archived'));
    mount(null);
    await open();
    fireEvent.focus(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    fireEvent.click(screen.getByRole('menuitemradio', { name: /Acme/ }));
    const alert = await screen.findByRole('alert');
    const menu = screen.getByRole('menu');
    expect(menu.contains(alert)).toBe(false);
    expect(menu.contains(screen.getByRole('note'))).toBe(false);
    expect(menu).toHaveAttribute('aria-describedby', screen.getByRole('note').id);
  });

  it('puts focus back on the menu after a refusal, so Escape still closes it', async () => {
    vi.spyOn(api, 'assignProject').mockRejectedValue(new ApiError(409, 'customer is archived'));
    mount(null);
    await open();
    fireEvent.click(await screen.findByRole('menuitemradio', { name: /Acme/ }));
    await screen.findByRole('alert');
    await waitFor(() => expect(screen.getAllByRole('menuitemradio')[0]).toHaveFocus());
    fireEvent.keyDown(document.activeElement as Element, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('ArrowUp with nothing focused goes to the last item', async () => {
    mount(null);
    await open();
    act(() => (document.activeElement as HTMLElement).blur());
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'ArrowUp' });
    const items = allItems();
    expect(items[items.length - 1]).toHaveFocus();
  });

  it('a mouse leaving another row does not clear the preview keyboard focus is showing', async () => {
    mount(null);
    await open();
    const acme = await screen.findByRole('menuitemradio', { name: /Acme/ });
    act(() => acme.focus());
    expect(screen.getByText(/Assigning to Acme/)).toBeInTheDocument();
    const globex = screen.getByRole('menuitemradio', { name: /Globex/ });
    fireEvent.mouseEnter(globex);
    expect(screen.getByText(/Assigning to Globex/)).toBeInTheDocument();
    fireEvent.mouseLeave(globex);
    expect(screen.getByText(/Assigning to Acme/)).toBeInTheDocument();
  });
});
