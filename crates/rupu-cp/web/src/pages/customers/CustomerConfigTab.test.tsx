// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type ConfigView } from '../../lib/api';
import CustomerConfigTab from './CustomerConfigTab';

const TINT = { light: '#111111', dark: '#eeeeee' };

const AZURE_KEY = 'providers."azure.eastus".base_url';

function view(over: Partial<ConfigView> = {}): ConfigView {
  return {
    effective: {
      default_provider: 'acme-prod',
      default_model: 'claude-sonnet-4-6',
      permission_mode: 'ask',
      log_level: 'info',
      providers: {
        'acme-prod': { kind: 'anthropic' },
        'azure.eastus': { base_url: 'https://az.example' },
      },
      scm: {},
      issues: {},
      autoflow: {},
      pricing: { agents: {} },
      policy: { lock: ['permission_mode'] },
    },
    provenance: {
      default_provider: { source: 'customer', locked: true, locked_by: 'customer' },
      default_model: { source: 'global', locked: false },
      permission_mode: { source: 'global', locked: true, locked_by: 'global' },
      log_level: { source: 'customer', locked: false },
      [AZURE_KEY]: { source: 'customer', locked: false },
    },
    raw_global: '',
    raw_project: null,
    raw_customer: 'default_provider = "acme-prod"\n',
    customer: { slug: 'acme', name: 'Acme Corp', tint: TINT, archived: false },
    customer_lock: ['default_provider'],
    layer_error: null,
    cp: {},
    status: { bind: '127.0.0.1:7878', token_set: false, restart_required_keys: [] },
    ...over,
  };
}

function mount(onChanged?: () => void) {
  return render(
    <MemoryRouter>
      <CustomerConfigTab
        slug="acme"
        name="Acme Corp"
        projectCount={2}
        layerPath="~/.rupu/customers/acme/config.toml"
        onChanged={onChanged}
      />
    </MemoryRouter>,
  );
}

let get: ReturnType<typeof vi.spyOn>;
let put: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  get = vi.spyOn(api, 'getCustomerConfig').mockResolvedValue(view());
  put = vi.spyOn(api, 'putCustomerConfig').mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('CustomerConfigTab', () => {
  it('describes the layer and renders customer / global provenance chips', async () => {
    mount();
    await screen.findByLabelText('Default provider');
    expect(get).toHaveBeenCalledWith('acme');
    expect(screen.getByText(/applies to Acme Corp's 2 projects/)).toBeInTheDocument();
    const customerChip = screen.getAllByText('customer')[0];
    expect(customerChip.className).toContain('text-brand-700');
    const globalChip = screen.getAllByText('global')[0];
    expect(globalChip.className).not.toContain('text-brand-700');
    expect(screen.getByText('locked by customer')).toBeInTheDocument();
  });

  it('an inherited key is read-only until "Override for <name>" stages it', async () => {
    mount();
    await screen.findByLabelText('Default provider');
    expect(screen.queryByLabelText('Default model')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Override default_model for Acme Corp' }));
    const input = (await screen.findByLabelText('Default model')) as HTMLInputElement;
    expect(input.value).toBe('claude-sonnet-4-6');
    fireEvent.change(input, { target: { value: 'claude-opus-5' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() =>
      expect(put).toHaveBeenCalledWith('acme', { patch: { default_model: 'claude-opus-5' } }),
    );
  });

  it('a customer-owned key has a lock switch; toggling off writes policy.lock', async () => {
    mount();
    await screen.findByLabelText('Default provider');
    const sw = screen.getByRole('switch', { name: 'Lock default_provider for projects' });
    expect(sw).toBeChecked();
    fireEvent.click(sw);
    await waitFor(() => expect(put).toHaveBeenCalledWith('acme', { patch: { 'policy.lock': [] } }));
    // and the layer is re-read afterwards
    await waitFor(() => expect(get.mock.calls.length).toBeGreaterThan(1));
  });

  it('locks a dotted-segment key with the canonical quoted encoding', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Providers' }));
    const sw = await screen.findByRole('switch', {
      name: 'Lock providers."azure.eastus".base_url for projects',
    });
    fireEvent.click(sw);
    await waitFor(() =>
      expect(put).toHaveBeenCalledWith('acme', {
        patch: { 'policy.lock': ['default_provider', 'providers."azure.eastus".base_url'] },
      }),
    );
  });

  it('a globally locked key is read-only with the warn chip and no lock switch', async () => {
    mount();
    await screen.findByLabelText('Default provider');
    expect(screen.queryByLabelText('Permission mode')).not.toBeInTheDocument();
    expect(screen.getByText('locked by global policy')).toBeInTheDocument();
    expect(
      screen.getByText("The global [policy].lock pins this — a customer can't change it."),
    ).toBeInTheDocument();
    expect(screen.queryByRole('switch', { name: /permission_mode/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Override permission_mode/ })).not.toBeInTheDocument();
  });

  it('names how a customer-sourced named default provider is declared', async () => {
    mount();
    await screen.findByLabelText('Default provider');
    expect(screen.getByText('rupu auth login --account acme-prod --kind anthropic')).toBeInTheDocument();
  });

  it('omits the declaration line when the account kind is unknown', async () => {
    get.mockResolvedValue(view({ effective: { ...view().effective, providers: {} } }));
    mount();
    await screen.findByLabelText('Default provider');
    expect(screen.queryByText(/rupu auth login/)).not.toBeInTheDocument();
  });

  it('warns that the layer\'s scm.rules replace the global ones', async () => {
    get.mockResolvedValue(view({ raw_customer: '[[scm.rules]]\nowner = "acme"\n' }));
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'SCM / Issues' }));
    expect(
      await screen.findByText(/These rules replace the global \[\[scm\.rules\]\] for Acme Corp's projects/),
    ).toBeInTheDocument();
  });

  it('no scm.rules note when the layer sets none', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'SCM / Issues' }));
    await screen.findByText('Default SCM');
    expect(screen.queryByText(/These rules replace/)).not.toBeInTheDocument();
  });

  it('shows the 400 from a save inline', async () => {
    put.mockRejectedValue(new ApiError(400, 'key log_level is enforced by global policy'));
    mount();
    const input = (await screen.findByLabelText('Log level')) as HTMLSelectElement;
    fireEvent.change(input, { target: { value: 'debug' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByText('key log_level is enforced by global policy')).toBeInTheDocument();
  });

  it('a layer_error shows the banner and opens the Raw tab', async () => {
    get.mockResolvedValue(view({ layer_error: 'expected `=` at line 2', raw_customer: 'oops\n' }));
    mount();
    const banner = await screen.findByRole('alert');
    expect(banner).toHaveTextContent("doesn't parse");
    expect(banner).toHaveTextContent('expected `=` at line 2');
    expect(await screen.findByText(/this customer\)/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Default provider')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeDisabled();
  });

  it('saving Raw sends { raw } and reloads', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Raw' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Edit' }));
    const editor = await screen.findByLabelText('Edit raw TOML');
    fireEvent.change(editor, { target: { value: 'log_level = "debug"\n' } });
    const before = get.mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(put).toHaveBeenCalledWith('acme', { raw: 'log_level = "debug"\n' }));
    await waitFor(() => expect(get.mock.calls.length).toBeGreaterThan(before));
  });

  it('the Policy tab lists the customer locks and the globally pinned keys', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Policy' }));
    const own = await screen.findByLabelText('default_provider');
    expect(own).toBeChecked();
    expect(screen.getByText('permission_mode')).toBeInTheDocument();
    const pinned = screen.getByText('Pinned by the global policy').closest('section')!;
    expect(within(pinned).getByText('locked by global policy')).toBeInTheDocument();
  });

  it('a 501 shows the read-only note instead of an error', async () => {
    put.mockRejectedValue(new ApiError(501, 'no launcher'));
    mount();
    const input = (await screen.findByLabelText('Log level')) as HTMLSelectElement;
    fireEvent.change(input, { target: { value: 'debug' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByText(/read-only deploy/)).toBeInTheDocument();
  });
});
