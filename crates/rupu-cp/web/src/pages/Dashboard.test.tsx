// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Dashboard from './Dashboard';
import { api, ApiError, type DashboardResponse, type RegisteredHostView } from '../lib/api';
import { useCustomerScope } from '../lib/customerScope';
import { emptyFleet } from '../lib/dashboard/mergeSummaries';
import { customerRow, scopedEntry, withCustomerScope, ACME } from '../lib/customerScopeTestUtils';

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
    fleet: emptyFleet(),
    findings_partial: false,
    cycles_partial: false,
    fleet_partial: false,
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
        {withCustomerScope(<Dashboard />)}
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
        {withCustomerScope(<Dashboard />)}
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
        {withCustomerScope(<Dashboard />)}
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
        {withCustomerScope(<Dashboard />)}
      </MemoryRouter>,
    );

    await waitFor(() => expect(sub).toHaveBeenCalled());
  });
});

describe('Dashboard fleet strip', () => {
  it('renders the fleet strip, with real counts and em-dashes for unreported fields', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(
      summary({
        fleet: {
          ...emptyFleet(),
          autoflows_enabled: 6,
          autoflows_disabled: 2,
          workers: 3,
          claims_active: 9,
        },
      }),
    );
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(
      <MemoryRouter>
        {withCustomerScope(<Dashboard />)}
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByTestId('fleet-strip')).toBeInTheDocument());
    expect(screen.getByTestId('fleet-workers')).toHaveTextContent('3 workers');
    expect(screen.getByTestId('fleet-claims')).toHaveTextContent('9 claimed');
    expect(screen.getByTestId('fleet-autoflows')).toHaveTextContent('2 off');
    // repos / providers / issues are Plan 2 and Plan 3 territory: unreported,
    // so they must render as em-dashes rather than fabricated zeros.
    expect(screen.getByTestId('fleet-repos')).toHaveTextContent('—');
    expect(screen.getByTestId('fleet-providers')).toHaveTextContent('—');
    expect(screen.getByTestId('fleet-issues')).toHaveTextContent('—');
  });
});

describe('Dashboard customer scope', () => {
  const PROD_HOST: RegisteredHostView = { id: 'host_prod', name: 'prod', transport_kind: 'http_cp' };

  function ScopeProbe() {
    const { scope, notice } = useCustomerScope();
    return (
      <>
        <span data-testid="scope">{String(scope)}</span>
        <span data-testid="notice">{notice ?? ''}</span>
      </>
    );
  }

  function mountScoped(scope = 'acme', customers = [ACME]) {
    return render(
      <MemoryRouter initialEntries={[scopedEntry(scope)]}>
        {withCustomerScope(
          <>
            <Dashboard />
            <ScopeProbe />
          </>,
          { customers },
        )}
      </MemoryRouter>,
    );
  }

  it('fetches every host with the scope, shows the ScopeChip, and names a remote host it could not count', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, PROD_HOST]);
    const getDashboard = vi.spyOn(api, 'getDashboard').mockImplementation((_r, host) =>
      host === 'host_prod'
        ? Promise.reject(
            new ApiError(
              501,
              'x',
              JSON.stringify({ error: "host host_prod can't be filtered by customer: its totals are summed remotely" }),
            ),
          )
        : Promise.resolve(summary({ hosts_without_customer: ['worker-7'] })),
    );
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    mountScoped();

    await waitFor(() => expect(getDashboard).toHaveBeenCalledWith('30d', 'local', 'acme'));
    expect(getDashboard).toHaveBeenCalledWith('30d', 'host_prod', 'acme');
    expect(await screen.findByText('Acme')).toBeInTheDocument();
    const banner = await screen.findByTestId('hosts-without-customer');
    expect(banner).toHaveTextContent(/worker-7 runs an older rupu/);
    expect(banner).toHaveTextContent(/prod can’t be filtered by customer \(its totals are summed on the host\)/);
  });

  it('clearing the chip sets the scope to null and refetches unfiltered', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    const getDashboard = vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    mountScoped();
    await waitFor(() => expect(getDashboard).toHaveBeenCalledWith('30d', 'local', 'acme'));
    fireEvent.click(await screen.findByRole('button', { name: 'Clear customer scope' }));

    await waitFor(() => expect(screen.getByTestId('scope')).toHaveTextContent('null'));
    // The unscoped request is exactly the old one: no third argument.
    await waitFor(() => expect(getDashboard.mock.lastCall).toEqual(['30d', 'local']));
    expect(screen.queryByRole('button', { name: 'Clear customer scope' })).not.toBeInTheDocument();
  });

  it('a scope the backend rejects (400) is cleared and the page refetches unfiltered', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    const getDashboard = vi.spyOn(api, 'getDashboard').mockImplementation((_r, _h, customer) =>
      customer
        ? Promise.reject(new ApiError(400, 'x', JSON.stringify({ error: 'customer: invalid slug "Bad Slug"' })))
        : Promise.resolve(summary()),
    );
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    mountScoped('bad', [ACME, customerRow('bad')]);
    await waitFor(() => expect(screen.getByTestId('scope')).toHaveTextContent('null'));
    expect(screen.getByTestId('notice')).toHaveTextContent(/The customer filter was rejected \(customer: invalid slug/);
    await waitFor(() => expect(getDashboard.mock.lastCall).toEqual(['30d', 'local']));
    await waitFor(() => expect(screen.getByTestId('tile-awaiting')).toHaveTextContent('1'));
  });
});
