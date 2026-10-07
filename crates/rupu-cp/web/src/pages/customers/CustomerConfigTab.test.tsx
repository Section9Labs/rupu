// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type ConfigView } from '../../lib/api';
import CustomerConfigTab from './CustomerConfigTab';

// The real editor lazy-loads CodeMirror (which jsdom can't measure); a plain
// textarea keeps the Raw-tab tests deterministic.
vi.mock('../../components/CodeEditor', () => ({
  default: ({ value, onChange, ariaLabel }: { value: string; onChange: (v: string) => void; ariaLabel?: string }) => (
    <textarea aria-label={ariaLabel} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

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
      'providers.acme-prod.kind': { source: 'global', locked: false },
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

  describe('the default_provider account note', () => {
    function withProvenance(kindSource: 'global' | 'customer' | 'project' | null, acct = 'acme-prod') {
      const base = view();
      const provenance = { ...base.provenance };
      if (kindSource) provenance[`providers.${acct}.kind`] = { source: kindSource, locked: false };
      return view({
        effective: {
          ...base.effective,
          default_provider: acct,
          providers: { [acct]: kindSource ? { kind: 'anthropic' } : {} },
        },
        provenance,
      });
    }

    it('says declared globally only when the provider table came from the global layer', async () => {
      get.mockResolvedValue(withProvenance('global'));
      mount();
      await screen.findByLabelText('Default provider');
      expect(screen.getByText(/Declared globally by/)).toBeInTheDocument();
      expect(screen.getByText('rupu auth login --account acme-prod --kind anthropic')).toBeInTheDocument();
    });

    it("says it is declared in this customer's layer when the customer layer wrote it", async () => {
      get.mockResolvedValue(withProvenance('customer'));
      mount();
      await screen.findByLabelText('Default provider');
      expect(screen.getByText(/Declared in this customer's layer/)).toBeInTheDocument();
      expect(screen.queryByText(/Declared globally/)).toBeNull();
    });

    it('names a project layer plainly', async () => {
      get.mockResolvedValue(withProvenance('project'));
      mount();
      await screen.findByLabelText('Default provider');
      expect(screen.getByText(/Declared in a project's layer/)).toBeInTheDocument();
      expect(screen.queryByText(/Declared globally/)).toBeNull();
    });

    it('warns when the account is declared nowhere', async () => {
      get.mockResolvedValue(withProvenance(null));
      mount();
      await screen.findByLabelText('Default provider');
      const note = screen.getByText(/isn't declared/);
      expect(note.className).toContain('text-warn');
      expect(screen.getByText('rupu auth login --account acme-prod --kind <vendor>')).toBeInTheDocument();
      expect(screen.queryByText(/Declared globally/)).toBeNull();
    });

    it('keys a dotted account name by its quoted provenance path', async () => {
      const base = view();
      get.mockResolvedValue(
        view({
          effective: {
            ...base.effective,
            default_provider: 'azure.eastus',
            providers: { 'azure.eastus': { kind: 'openai-compatible', base_url: 'https://az.example' } },
          },
          provenance: {
            ...base.provenance,
            'providers."azure.eastus".kind': { source: 'customer', locked: false },
          },
        }),
      );
      mount();
      await screen.findByLabelText('Default provider');
      expect(screen.getByText(/Declared in this customer's layer/)).toBeInTheDocument();
    });

    it('stays quiet for every built-in vendor account name, codex included', async () => {
      for (const acct of ['codex', 'openai_codex', 'google_gemini', 'github_copilot']) {
        get.mockResolvedValue(withProvenance(null, acct));
        const { unmount } = mount();
        await screen.findByLabelText('Default provider');
        expect(screen.queryByText(/isn't declared/)).toBeNull();
        unmount();
      }
    });

    it('still says what the kind is when no layer claims it', async () => {
      const base = withProvenance('global');
      const provenance = { ...base.provenance };
      delete provenance['providers.acme-prod.kind'];
      get.mockResolvedValue(view({ effective: base.effective, provenance }));
      mount();
      await screen.findByLabelText('Default provider');
      expect(screen.getByText(/Declared as/)).toBeInTheDocument();
      expect(screen.queryByText(/Declared globally/)).toBeNull();
    });

    it('stays quiet for a built-in vendor account, whose name is the vendor', async () => {
      get.mockResolvedValue(withProvenance(null, 'anthropic'));
      mount();
      await screen.findByLabelText('Default provider');
      expect(screen.queryByText(/isn't declared/)).toBeNull();
    });
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

  it('flags an account declared nowhere instead of omitting the line', async () => {
    get.mockResolvedValue(view({ effective: { ...view().effective, providers: {} } }));
    mount();
    await screen.findByLabelText('Default provider');
    expect(screen.queryByText(/Declared globally/)).not.toBeInTheDocument();
    expect(screen.getByText(/isn't declared/)).toBeInTheDocument();
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

  it('serialises lock writes: a second toggle in the PUT window is blocked, not dropped', async () => {
    let release!: () => void;
    put.mockImplementation(() => new Promise<void>((r) => (release = r)));
    get.mockResolvedValue(
      view({ customer_lock: ['default_provider'] }),
    );
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Providers' }));
    const sw = await screen.findByRole('switch', {
      name: 'Lock providers."azure.eastus".base_url for projects',
    });
    fireEvent.click(sw);
    await waitFor(() => expect(put).toHaveBeenCalledTimes(1));
    // Every lock control is inert while the write is in flight.
    expect(screen.getByRole('switch', { name: 'Lock providers."azure.eastus".base_url for projects' })).toBeDisabled();
    fireEvent.click(screen.getByRole('switch', { name: 'Lock providers."azure.eastus".base_url for projects' }));
    expect(put).toHaveBeenCalledTimes(1);
    // The reload reports the first write; the next toggle builds on it.
    get.mockResolvedValue(
      view({ customer_lock: ['default_provider', AZURE_KEY] }),
    );
    release();
    await waitFor(() =>
      expect(screen.getByRole('switch', { name: 'Lock providers."azure.eastus".base_url for projects' })).not.toBeDisabled(),
    );
    put.mockResolvedValue(undefined);
    fireEvent.click(screen.getByRole('button', { name: 'Policy' }));
    fireEvent.click(await screen.findByLabelText('default_provider'));
    await waitFor(() => expect(put).toHaveBeenCalledTimes(2));
    expect(put).toHaveBeenLastCalledWith('acme', { patch: { 'policy.lock': [AZURE_KEY] } });
  });

  it('the Policy Add key control is disabled while a lock write is in flight', async () => {
    put.mockImplementation(() => new Promise<void>(() => {}));
    mount();
    fireEvent.click(await screen.findByRole('switch', { name: 'Lock default_provider for projects' }));
    await waitFor(() => expect(put).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'Policy' }));
    expect(await screen.findByLabelText('Key to lock')).toBeDisabled();
    expect(screen.getByLabelText('default_provider')).toBeDisabled();
  });

  it('a failed reload after a save keeps the view and the staged edit, with Retry', async () => {
    mount();
    const input = (await screen.findByLabelText('Log level')) as HTMLSelectElement;
    fireEvent.change(input, { target: { value: 'debug' } });
    get.mockRejectedValueOnce(new Error('network down'));
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByText(/Couldn't refresh this layer: network down/)).toBeInTheDocument();
    expect(put).toHaveBeenCalledWith('acme', { patch: { log_level: 'debug' } });
    // Still on the form, still showing the edit.
    expect((screen.getByLabelText('Log level') as HTMLSelectElement).value).toBe('debug');
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(screen.queryByText(/Couldn't refresh/)).not.toBeInTheDocument());
    expect(screen.getByLabelText('Log level')).toBeInTheDocument();
  });

  it('a failed first load offers Retry', async () => {
    get.mockRejectedValueOnce(new Error('boom'));
    mount();
    expect(await screen.findByText('boom')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByLabelText('Default provider')).toBeInTheDocument();
  });

  it('a key in customer_lock gets an unlockable switch even when its value is inherited', async () => {
    get.mockResolvedValue(view({ customer_lock: ['default_provider', 'default_model'] }));
    put.mockResolvedValue(undefined);
    mount();
    await screen.findByLabelText('Default provider');
    // default_model is global-sourced (read-only, "Override") but locked by the customer.
    const sw = screen.getByRole('switch', { name: 'Lock default_model for projects' });
    expect(sw).toBeChecked();
    fireEvent.click(sw);
    await waitFor(() =>
      expect(put).toHaveBeenCalledWith('acme', { patch: { 'policy.lock': ['default_provider'] } }),
    );
  });

  it('Add key rejects a non-canonical key inline and accepts the quoted form', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Policy' }));
    const input = await screen.findByLabelText('Key to lock');
    fireEvent.change(input, { target: { value: 'providers.azure.eastus.base_url.' } });
    expect(screen.getByText(/Not a canonical key/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Lock key' })).toBeDisabled();
    fireEvent.change(input, { target: { value: 'providers."azure.eastus".base_url' } });
    expect(screen.queryByText(/Not a canonical key/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Lock key' }));
    await waitFor(() =>
      expect(put).toHaveBeenCalledWith('acme', {
        patch: { 'policy.lock': ['default_provider', AZURE_KEY] },
      }),
    );
  });

  it('under layer_error, Override and the lock controls are disabled', async () => {
    get.mockResolvedValue(view({ layer_error: 'bad toml', raw_customer: 'oops\n' }));
    mount();
    await screen.findByText(/this customer\)/);
    fireEvent.click(screen.getByRole('button', { name: 'General' }));
    const override = await screen.findByRole('button', { name: 'Override default_model for Acme Corp' });
    expect(override).toBeDisabled();
    expect(screen.getByRole('switch', { name: 'Lock default_provider for projects' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Policy' }));
    expect(await screen.findByLabelText('Key to lock')).toBeDisabled();
    expect(screen.getByText(/Fix the layer in the Raw tab first/)).toBeInTheDocument();
    // Raw stays usable.
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));
    expect(screen.getByRole('button', { name: 'Edit' })).toBeEnabled();
  });

  it('a successful Raw save clears staged edits', async () => {
    mount();
    const input = (await screen.findByLabelText('Log level')) as HTMLSelectElement;
    fireEvent.change(input, { target: { value: 'debug' } });
    expect(screen.getByText('1 unsaved change')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Edit' }));
    fireEvent.change(await screen.findByLabelText('Edit raw TOML'), { target: { value: 'log_level = "warn"\n' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(put).toHaveBeenCalledWith('acme', { raw: 'log_level = "warn"\n' }));
    await waitFor(() => expect(screen.queryByText('1 unsaved change')).not.toBeInTheDocument());
  });
});
