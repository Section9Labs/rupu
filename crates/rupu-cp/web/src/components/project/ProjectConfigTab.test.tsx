// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { api, type ConfigView } from '../../lib/api';
import ProjectConfigTab from './ProjectConfigTab';

function view(over: Partial<ConfigView> = {}): ConfigView {
  return {
    effective: { default_model: 'claude-sonnet-4-6', permission_mode: 'ask' },
    provenance: {
      default_model: { source: 'global', locked: false },
      permission_mode: { source: 'customer', locked: true, locked_by: 'customer' },
    },
    raw_global: '',
    raw_project: null,
    cp: {},
    status: { bind: '127.0.0.1:7878', token_set: false, restart_required_keys: [] },
    ...over,
  };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ProjectConfigTab', () => {
  it('shows no banner for a healthy config, and a customer-locked key reads as enforced by the customer', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(view());
    render(<ProjectConfigTab wsId="ws1" />);
    await screen.findByLabelText('Default model');
    expect(screen.queryByText(/doesn't parse/)).not.toBeInTheDocument();
    expect(screen.getByText('enforced by customer policy')).toBeInTheDocument();
    expect(screen.queryByLabelText('Permission mode')).not.toBeInTheDocument();
  });

  it('surfaces ConfigView.layer_error as a banner', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(view({ layer_error: 'expected `=` at line 3' }));
    render(<ProjectConfigTab wsId="ws1" />);
    const banner = await screen.findByRole('alert');
    expect(banner).toHaveTextContent("doesn't parse");
    expect(banner).toHaveTextContent('expected `=` at line 3');
  });

  it('a broken project layer under a customer says the values are global + that customer', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(
      view({
        layer_error: 'expected `=` at line 3',
        layer_error_kept: 'global_customer',
        customer: { slug: 'acme', name: 'Acme Corp', tint: { light: '#111111', dark: '#eeeeee' }, archived: false },
      }),
    );
    render(<ProjectConfigTab wsId="ws1" />);
    const banner = await screen.findByRole('alert');
    expect(banner).toHaveTextContent(
      "This project's config doesn't parse — the values below are the global config plus Acme Corp's layer, without this project's own settings",
    );
    expect(banner).not.toHaveTextContent('global config alone');
    expect(banner).toHaveTextContent('Fix it in the Raw tab.');
  });

  it('a broken customer layer says the values are the global config alone', async () => {
    vi.spyOn(api, 'getConfig').mockResolvedValue(
      view({
        layer_error: 'customers/acme/config.toml: expected `=`',
        layer_error_kept: 'global',
        customer: { slug: 'acme', name: 'Acme Corp', tint: { light: '#111111', dark: '#eeeeee' }, archived: false },
      }),
    );
    render(<ProjectConfigTab wsId="ws1" />);
    const banner = await screen.findByRole('alert');
    expect(banner).toHaveTextContent('the values below are the global config alone');
    expect(banner).toHaveTextContent("or the customer's Config tab");
  });
});
