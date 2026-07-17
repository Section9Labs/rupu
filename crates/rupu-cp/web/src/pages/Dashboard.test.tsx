// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Dashboard from './Dashboard';
import { api, type DashboardResponse, type RegisteredHostView } from '../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// `getDashboard` resolves `DashboardResponse` on the wire (`DashboardSummary`
// flattened with `hosts` / `findings_partial` / `cycles_partial` — see
// useDashboardData.test.ts, which this mirrors). The hook reads BOTH: the
// flattened `DashboardSummary` fields, and `resp.hosts` to find its OWN
// per-host entry and honor its authoritative `state` (a 200 response is not
// proof of health). So the default here seeds a matching
// `hosts: [{ host_id: hostId, state: 'ok', ... }]` entry for the healthy case.
function summary(overrides: Partial<DashboardResponse> = {}, hostId = 'local'): DashboardResponse {
  const captured_at = overrides.captured_at ?? new Date().toISOString();
  return {
    active: { running: 2, awaiting_approval: 1, paused: 0, pending: 0 },
    active_longest: null,
    terminal_buckets: [],
    throughput_buckets: [],
    cycles: { total: 0, clean: 0, with_failures: 0 },
    findings_open: 3,
    captured_at,
    hosts: [{ host_id: hostId, name: hostId, transport_kind: 'local', state: 'ok', captured_at, reason: null }],
    findings_partial: false,
    cycles_partial: false,
    ...overrides,
  };
}

const LOCAL_HOST: RegisteredHostView = { id: 'local', name: 'local', transport_kind: 'local' };

describe('Dashboard', () => {
  it('renders the freshness strip from the host list and the key-point tiles from a mocked payload', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('local')).toBeInTheDocument());
    // Key-point tiles: awaiting-you count from the mocked payload.
    await waitFor(() => expect(screen.getByTestId('tile-awaiting')).toHaveTextContent('1'));
    expect(screen.getByTestId('tile-findings')).toHaveTextContent('3');
  });

  it('does not render any of the removed per-item surfaces (attention row, active-status tiles)', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByTestId('tile-awaiting')).toBeInTheDocument());
    // AttentionRow's distinct "Blocked on you" label and ActiveStatusTiles'
    // "Pending" tile must not appear — KeyPointTiles is the single count
    // surface now.
    expect(screen.queryByText(/Blocked on you/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/^Pending$/)).not.toBeInTheDocument();
  });

  it('renders a slow (still-loading) second host without blanking the page once local has data', async () => {
    const SSH_HOST: RegisteredHostView = { id: 'ssh1', name: 'staging-box', transport_kind: 'ssh' };
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    vi.spyOn(api, 'getDashboard').mockImplementation((_range, host) => {
      if (host === 'local') return Promise.resolve(summary());
      return new Promise<DashboardResponse>(() => {}); // hangs forever
    });
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByTestId('tile-awaiting')).toHaveTextContent('1'));
    // Both hosts show in the freshness strip; the hung one reads as loading,
    // not as an error, and does not block the tiles above from rendering.
    expect(screen.getByText('local')).toBeInTheDocument();
    expect(screen.getByText('staging-box')).toBeInTheDocument();
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
  });

  it('subscribes to the event stream for invalidation', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    const sub = vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        <Dashboard />
      </MemoryRouter>,
    );

    await waitFor(() => expect(sub).toHaveBeenCalled());
  });
});
