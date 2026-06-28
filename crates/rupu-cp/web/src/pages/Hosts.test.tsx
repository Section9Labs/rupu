// @vitest-environment jsdom
// Hosts list — rows from mocked getHosts; local pinned first; Add Host form
// calls addHost; Remove button absent for local.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type HostView } from '../lib/api';

import Hosts from './Hosts';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'local',
  transport_kind: 'local',
  status: 'online',
  version: '0.9.0',
  active_run_count: 0,
  last_seen_at: new Date().toISOString(),
};

const REMOTE_HOST: HostView = {
  id: 'host-abc123',
  name: 'prod-east',
  transport_kind: 'http_cp',
  base_url: 'https://rupu.prod-east.example.com',
  status: 'online',
  version: '0.9.0',
  active_run_count: 2,
  last_seen_at: new Date().toISOString(),
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('Hosts page', () => {
  it('renders both hosts from getHosts', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // "local" appears multiple times (name, shortId, transport badge) — use
    // findAllByText and just assert at least one element is in the document.
    const localEls = await screen.findAllByText('local');
    expect(localEls.length).toBeGreaterThan(0);
    // "prod-east" is unique — appears only once as the host name
    expect(await screen.findByText('prod-east')).toBeInTheDocument();
  });

  it('shows local host before remote host (local pinned first)', async () => {
    // Pass remote first, then local — the sort should still put local first
    vi.spyOn(api, 'getHosts').mockResolvedValue([REMOTE_HOST, LOCAL_HOST]);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Wait for both rows to be in the DOM
    await screen.findByText('prod-east');
    // "local" appears multiple times — findAllByText works here
    await screen.findAllByText('local');

    // tbody rows — first data row should be the local host
    const rows = document.querySelectorAll('tbody tr');
    expect(rows.length).toBeGreaterThanOrEqual(2);
    const firstRowText = rows[0].textContent ?? '';
    expect(firstRowText).toContain('local');
    // Remote host should be in the second row
    const secondRowText = rows[1].textContent ?? '';
    expect(secondRowText).toContain('prod-east');
  });

  it('does NOT render a Remove button for the local host', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    await screen.findByText('prod-east');

    // Only one Remove button — for the remote host, not for local
    const removeButtons = screen.queryAllByRole('button', { name: /remove host/i });
    expect(removeButtons).toHaveLength(1);
    expect(removeButtons[0]).toHaveAccessibleName('Remove host prod-east');
  });

  it('Add Host form submits and calls api.addHost', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    const addSpy = vi.spyOn(api, 'addHost').mockResolvedValue({
      ...REMOTE_HOST,
      id: 'host-new1',
      name: 'staging',
      base_url: 'https://rupu.staging.example.com',
    });

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    // Open the Add host form
    fireEvent.click(await screen.findByRole('button', { name: 'Add host' }));

    // Fill in the form
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'staging' } });
    fireEvent.change(screen.getByLabelText('Base URL'), {
      target: { value: 'https://rupu.staging.example.com' },
    });
    fireEvent.change(screen.getByLabelText(/Token/i), {
      target: { value: 'my-token' },
    });

    // Submit
    fireEvent.click(screen.getByRole('button', { name: 'Add host' }));

    await waitFor(() => expect(addSpy).toHaveBeenCalledTimes(1));
    expect(addSpy.mock.calls[0][0]).toMatchObject({
      name: 'staging',
      base_url: 'https://rupu.staging.example.com',
      token: 'my-token',
    });

    // List must refresh after a successful add (initial load + post-add refresh)
    await waitFor(() => expect(vi.mocked(api.getHosts)).toHaveBeenCalledTimes(2));
  });

  it('shows loading state before data arrives', () => {
    // Never resolves
    vi.spyOn(api, 'getHosts').mockImplementation(() => new Promise(() => {}));

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    expect(screen.getByText(/loading hosts/i)).toBeInTheDocument();
  });

  it('shows error when getHosts fails', async () => {
    vi.spyOn(api, 'getHosts').mockRejectedValue(new Error('network failure'));

    render(
      <MemoryRouter initialEntries={['/hosts']}>
        <Hosts />
      </MemoryRouter>,
    );

    expect(await screen.findByText(/network failure/i)).toBeInTheDocument();
  });
});
