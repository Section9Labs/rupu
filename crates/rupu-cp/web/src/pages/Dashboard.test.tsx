// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Dashboard from './Dashboard';
import { api } from '../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const payload = {
  hosts: [
    {
      host_id: 'local',
      name: 'local',
      transport_kind: 'local',
      state: 'ok' as const,
      captured_at: new Date().toISOString(),
      reason: null,
    },
  ],
  findings_partial: false,
  active: { running: 2, awaiting_approval: 1, paused: 0, pending: 0 },
  terminal_buckets: [],
  active_runs: [],
  cycles: [],
  recent_manual: [],
  findings_open: 3,
  captured_at: new Date().toISOString(),
};

describe('Dashboard', () => {
  it('renders the freshness strip and active counts', async () => {
    vi.spyOn(api, 'getDashboard').mockResolvedValue(payload);
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
    expect(screen.getByText('Running')).toBeInTheDocument();
  });

  it('subscribes to the event stream for invalidation', async () => {
    vi.spyOn(api, 'getDashboard').mockResolvedValue(payload);
    const sub = vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(sub).toHaveBeenCalled());
  });
});
