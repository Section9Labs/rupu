// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Dashboard from './Dashboard';
import { api, ApiError, type DashboardResponse, type RegisteredHostView } from '../lib/api';
import { useCustomerScope } from '../lib/customerScope';
import { emptyFleet } from '../lib/dashboard/mergeSummaries';
import { customerRow, scopedEntry, withCustomerScope, ACME } from '../lib/customerScopeTestUtils';

beforeEach(() => {
  // The inbox pointer and the recent-activity feed read existing run-list
  // endpoints per host; keep them empty and quiet here so these tests exercise
  // the dashboard's own data path.
  vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// `getDashboard` resolves `DashboardResponse` on the wire (`DashboardSummary`
// flattened with `hosts` / `findings_partial` / `cycles_partial`). The hook
// reads BOTH the flattened fields and `resp.hosts` (a 200 is not proof of
// health), so the default seeds a matching `ok` per-host entry.
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
  it('renders the fleet host cards and the type-first KPIs from a mocked payload', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(<MemoryRouter>{withCustomerScope(<Dashboard />)}</MemoryRouter>);

    // The host appears as a fleet card; the KPI strip shows open findings.
    await waitFor(() => expect(screen.getByTestId('host-local')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByTestId('kpi-findings')).toHaveTextContent('3'));
    expect(screen.getByTestId('kpi-active')).toHaveTextContent('2'); // running
  });

  it('points at the Inbox instead of carrying the action list', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(<MemoryRouter>{withCustomerScope(<Dashboard />)}</MemoryRouter>);
    // Nothing is waiting (no awaiting/paused runs mocked) → the pointer says so
    // and links to the Inbox; the action list itself is NOT on the overview.
    await waitFor(() => expect(screen.getByText('Nothing waiting on you.')).toBeInTheDocument());
    expect(screen.queryByText('Needs you')).not.toBeInTheDocument();
    const inboxLinks = screen.getAllByRole('link', { name: /open inbox/i });
    expect(inboxLinks[0]).toHaveAttribute('href', '/inbox');
  });

  it('renders a slow (still-loading) second host without blanking the page once local has data', async () => {
    const SSH_HOST: RegisteredHostView = { id: 'ssh1', name: 'staging-box', transport_kind: 'ssh' };
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    vi.spyOn(api, 'getDashboard').mockImplementation((_range, host) => {
      if (host === 'local') return Promise.resolve(summary());
      return new Promise<DashboardResponse>(() => {}); // hangs forever
    });
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(<MemoryRouter>{withCustomerScope(<Dashboard />)}</MemoryRouter>);

    await waitFor(() => expect(screen.getByTestId('kpi-findings')).toBeInTheDocument());
    expect(screen.getByText('local')).toBeInTheDocument();
    expect(screen.getByText('staging-box')).toBeInTheDocument();
    // The hung host reads as loading in its card, not as an error.
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
  });

  it('subscribes to the event stream for invalidation', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    const sub = vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    render(<MemoryRouter>{withCustomerScope(<Dashboard />)}</MemoryRouter>);
    await waitFor(() => expect(sub).toHaveBeenCalled());
  });
});

describe('Dashboard inventory strip', () => {
  it('renders the inventory strip, with real counts and em-dashes for unreported fields', async () => {
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

    render(<MemoryRouter>{withCustomerScope(<Dashboard />)}</MemoryRouter>);

    await waitFor(() => expect(screen.getByTestId('inv-workers')).toBeInTheDocument());
    expect(screen.getByTestId('inv-workers')).toHaveTextContent('3');
    expect(screen.getByTestId('inv-claims')).toHaveTextContent('9');
    expect(screen.getByTestId('inv-autoflows')).toHaveTextContent('2 off');
    // repos / providers / issues are unreported here → em-dashes, not zeros.
    expect(screen.getByTestId('inv-repos')).toHaveTextContent('—');
    expect(screen.getByTestId('inv-providers')).toHaveTextContent('—');
    expect(screen.getByTestId('inv-pending')).toHaveTextContent('—');
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
    // The fleet counts are not run-scoped: said so, not passed off as the customer's.
    expect(screen.getByRole('note')).toHaveTextContent('Fleet counts aren’t per customer.');
  });

  it('a remote 501 under the scope is a host state: no failure text, even before local answers', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, PROD_HOST]);
    let answerLocal: (v: DashboardResponse) => void = () => {};
    vi.spyOn(api, 'getDashboard').mockImplementation((_r, host) =>
      host === 'host_prod'
        ? Promise.reject(
            new ApiError(
              501,
              'x',
              JSON.stringify({ error: "host host_prod can't be filtered by customer: its totals are summed remotely" }),
            ),
          )
        : new Promise<DashboardResponse>((resolve) => {
            answerLocal = resolve;
          }),
    );
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    mountScoped();
    expect(await screen.findByTestId('hosts-without-customer')).toHaveTextContent(/prod can’t be filtered by customer/);
    expect(screen.queryByText(/Could not load dashboard/)).not.toBeInTheDocument();

    answerLocal(summary());
    await waitFor(() => expect(screen.getByTestId('kpi-findings')).toHaveTextContent('3'));
    expect(screen.queryByText(/refresh failed/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Could not load dashboard/)).not.toBeInTheDocument();
  });

  it('clearing the chip sets the scope to null and refetches unfiltered', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    const getDashboard = vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    mountScoped();
    await waitFor(() => expect(getDashboard).toHaveBeenCalledWith('30d', 'local', 'acme'));
    fireEvent.click(await screen.findByRole('button', { name: 'Clear customer scope' }));

    await waitFor(() => expect(screen.getByTestId('scope')).toHaveTextContent('null'));
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
    await waitFor(() => expect(screen.getByTestId('kpi-findings')).toHaveTextContent('3'));
  });
});
