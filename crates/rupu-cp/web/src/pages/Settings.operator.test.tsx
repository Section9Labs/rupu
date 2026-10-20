// @vitest-environment jsdom
// Settings — the UI-5 operator tabs: Automation, Maintenance and Accounts sit
// after the config tabs and before the Raw/Policy divider, open from
// `?tab=`, read their cadence/retention from the resolved config, and hide
// the config Save (they write through their own endpoints).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type ConfigView } from '../lib/api';
import { admin } from '../lib/api3b/admin';
import Settings from './Settings';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const CONFIG = {
  effective: {
    cp: { cron_tick_enabled: true, cron_tick_interval_secs: 30 },
    storage: { archived_transcript_retention: '14d' },
    policy: { lock: [] },
  },
  provenance: {},
  raw_global: '',
  raw_project: null,
  cp: { max_workspace_bytes: null },
  status: { bind: '127.0.0.1:7999', token_set: false, restart_required_keys: [] },
} as unknown as ConfigView;

function renderAt(url: string) {
  vi.spyOn(api, 'getConfig').mockResolvedValue(CONFIG);
  render(
    <MemoryRouter initialEntries={[url]}>
      <Settings />
    </MemoryRouter>,
  );
}

describe('Settings operator tabs', () => {
  it('places Automation, Maintenance and Accounts before Raw', async () => {
    renderAt('/settings');
    await screen.findByRole('button', { name: 'Automation' });

    const labels = screen.getAllByRole('button').map((b) => b.textContent?.trim());
    const at = (l: string) => labels.indexOf(l);
    expect(at('CP-Runtime')).toBeLessThan(at('Automation'));
    expect(at('Automation')).toBeLessThan(at('Maintenance'));
    expect(at('Maintenance')).toBeLessThan(at('Accounts'));
    expect(at('Accounts')).toBeLessThan(at('Raw'));
  });

  it('opens Automation from ?tab= with the configured tick cadence, without the config Save', async () => {
    vi.spyOn(admin, 'getCronList').mockResolvedValue({ kind: 'cron_list', version: 1, rows: [] });
    vi.spyOn(admin, 'getCronEvents').mockResolvedValue({ kind: 'cron_events', version: 1, rows: [], summary: { poll_sources: [] } });
    renderAt('/settings?tab=automation');

    expect(await screen.findByText('The cp loop ticks every 30 s.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save changes' })).not.toBeInTheDocument();
  });

  it('starts Maintenance at the configured retention', async () => {
    const prune = vi.spyOn(admin, 'prune').mockImplementation(async (b) => ({ kind: b.kind, dry_run: true, would_remove: [], errors: [] }));
    renderAt('/settings');
    fireEvent.click(await screen.findByRole('button', { name: 'Maintenance' }));

    await waitFor(() => expect(prune).toHaveBeenCalledWith({ kind: 'transcripts', older_than: '14d', dry_run: true }));
  });
});
