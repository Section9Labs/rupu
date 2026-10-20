// @vitest-environment jsdom
// The launcher sheets' Plan 3b fields: the Engagement profiles picker and the
// Model & provider section.

import '@testing-library/jest-dom/vitest';
import { useState } from 'react';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import { api, type CatalogProvider } from '../../lib/api';
import { launchApi, type EngagementProfiles } from '../../lib/api3b/launch';
import EngagementProfilePicker from './EngagementProfilePicker';
import ModelProviderSection, { type ModelChoice } from './ModelProviderSection';

const PROFILES: EngagementProfiles = {
  profiles: [
    {
      id: 'network',
      name: 'Network assessment',
      layer: 'builtin',
      includes: [],
      asset_kinds: [
        { id: 'host', label: '{host}' },
        { id: 'port', label: '{host}:{port}' },
      ],
      bundle: { agents: ['recon', 'service-analyst'], tools: [], workflows: [] },
    },
    {
      id: 'web',
      name: 'Web application',
      layer: 'global',
      includes: [],
      asset_kinds: [{ id: 'endpoint', label: '{url}' }],
      bundle: { agents: ['crawler'], tools: [], workflows: [] },
    },
  ],
  errors: [],
};

const CATALOG: CatalogProvider[] = [
  {
    provider: 'anthropic',
    fetched_at: null,
    stale: false,
    models: [
      { id: 'claude-opus-5-5', input_tokens: null, output_tokens: null, source: 'live' },
      { id: 'claude-sonnet-5-5', input_tokens: null, output_tokens: null, source: 'live' },
    ],
  },
  {
    provider: 'openai',
    fetched_at: null,
    stale: false,
    models: [{ id: 'gpt-6', input_tokens: null, output_tokens: null, source: 'live' }],
  },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function Picker({ reason, onChange }: { reason?: string | null; onChange?: (v: string[]) => void }) {
  const [v, setV] = useState<string[]>([]);
  return (
    <EngagementProfilePicker
      value={v}
      onChange={(n) => {
        setV(n);
        onChange?.(n);
      }}
      disabledReason={reason ?? null}
    />
  );
}

describe('EngagementProfilePicker', () => {
  beforeEach(() => {
    vi.spyOn(launchApi, 'getEngagementProfiles').mockResolvedValue(PROFILES);
  });

  it('adds a profile as a chip and shows what it brings', async () => {
    const onChange = vi.fn();
    render(<Picker onChange={onChange} />);
    const add = await screen.findByRole('combobox', { name: 'Add an engagement profile' });
    fireEvent.change(add, { target: { value: 'network' } });
    expect(onChange).toHaveBeenLastCalledWith(['network']);
    expect(screen.getByRole('button', { name: 'Remove network' })).toBeInTheDocument();
    expect(screen.getByText('recon, service-analyst · asset kinds host, port')).toBeInTheDocument();
    // the added profile leaves the add list
    expect(within(add).queryByRole('option', { name: /network/ })).toBeNull();
  });

  it('removes a chip', async () => {
    const onChange = vi.fn();
    render(<Picker onChange={onChange} />);
    fireEvent.change(await screen.findByRole('combobox', { name: 'Add an engagement profile' }), {
      target: { value: 'web' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Remove web' }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it('is disabled with the reason', async () => {
    render(<Picker reason="A session doesn’t take engagement profiles." />);
    expect(await screen.findByText('A session doesn’t take engagement profiles.')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Add an engagement profile' })).toBeDisabled();
  });

  it('warns when profile files don’t load', async () => {
    vi.spyOn(launchApi, 'getEngagementProfiles').mockResolvedValue({
      ...PROFILES,
      errors: ['profiles/broken.toml: missing field `name`'],
    });
    render(<Picker />);
    expect(await screen.findByText(/profiles\/broken\.toml: missing field `name`/)).toBeInTheDocument();
  });
});

function Section({ reason, onChange }: { reason?: string | null; onChange?: (c: ModelChoice) => void }) {
  const [v, setV] = useState<ModelChoice>({ provider: '', model: '' });
  return (
    <ModelProviderSection
      agentProvider="anthropic"
      agentModel="claude-opus-5-5"
      value={v}
      onChange={(c) => {
        setV(c);
        onChange?.(c);
      }}
      disabledReason={reason ?? null}
    />
  );
}

describe('ModelProviderSection', () => {
  beforeEach(() => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
  });

  it('defaults to the agent’s own provider and model', async () => {
    render(<Section />);
    const provider = await screen.findByRole('combobox', { name: 'Provider' });
    expect(provider).toHaveValue('');
    expect(within(provider).getByRole('option', { name: 'anthropic (agent default)' })).toBeInTheDocument();
    const model = screen.getByRole('combobox', { name: 'Model' });
    expect(model).toHaveValue('');
    expect(await within(model).findByRole('option', { name: 'claude-sonnet-5-5' })).toBeInTheDocument();
  });

  it('picks another provider’s first model when the provider changes', async () => {
    const onChange = vi.fn();
    render(<Section onChange={onChange} />);
    const provider = await screen.findByRole('combobox', { name: 'Provider' });
    await within(provider).findByRole('option', { name: 'openai' });
    fireEvent.change(provider, { target: { value: 'openai' } });
    expect(onChange).toHaveBeenLastCalledWith({ provider: 'openai', model: 'gpt-6' });
    fireEvent.change(provider, { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith({ provider: '', model: '' });
  });

  it('picks a model on the agent’s provider', async () => {
    const onChange = vi.fn();
    render(<Section onChange={onChange} />);
    const model = await screen.findByRole('combobox', { name: 'Model' });
    await within(model).findByRole('option', { name: 'claude-sonnet-5-5' });
    fireEvent.change(model, { target: { value: 'claude-sonnet-5-5' } });
    expect(onChange).toHaveBeenLastCalledWith({ provider: '', model: 'claude-sonnet-5-5' });
  });

  it('is disabled with the reason', async () => {
    render(<Section reason="Local host only: no host transport carries a model or provider override." />);
    expect(await screen.findByText(/Local host only/)).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Provider' })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: 'Model' })).toBeDisabled();
  });

  it('says so when the model list is unavailable', async () => {
    vi.spyOn(api, 'getModelCatalog').mockRejectedValue(new Error('needs rupu cp serve'));
    render(<Section />);
    expect(await screen.findByText(/Model list unavailable: needs rupu cp serve/)).toBeInTheDocument();
  });
});
