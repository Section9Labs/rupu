// @vitest-environment jsdom
// AutoflowRuns — the Claims tab lists active autoflow claims, each with
// Requeue + Release controls. The page's mount fetch (events + cycles) is
// stubbed so the test can switch to Claims and drive the per-row actions.
// Also tests the host filter that drives server-side fetch scope for the
// Launched runs and Cycles tabs.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowClaim, type AutoflowEventRow, type HostView } from '../../lib/api';
import AutoflowRuns from './AutoflowRuns';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};
const REMOTE_HOST: HostView = {
  id: 'host_prod',
  name: 'prod',
  transport_kind: 'http_cp',
  status: 'online',
  active_run_count: 1,
};

const REMOTE_EVENT: AutoflowEventRow = {
  event_id: 'evt-1',
  cycle_id: 'cyc-1',
  at: '2026-06-01T00:00:00Z',
  kind: 'run_launched',
  workflow: 'fix-issue',
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 1 },
  host_id: 'host_prod',
};

const CLAIM: AutoflowClaim = {
  issue_ref: 'github:acme/widgets#42',
  issue_display_ref: 'acme/widgets#42',
  repo_ref: 'github:acme/widgets',
  issue_title: 'Flaky retry path',
  issue_url: 'https://example.test/issues/42',
  workflow: 'fix-issue',
  status: 'await_human',
  last_run_id: 'run-9',
  last_error: null,
  last_summary: 'Waiting on reviewer',
  pr_url: 'https://example.test/pr/7',
  claim_owner: 'worker-1',
  lease_expires_at: null,
  updated_at: '2026-06-01T00:00:00Z',
};

function stubPage() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
  vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <AutoflowRuns />
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — Claims tab', () => {
  it('renders a claim with its workflow and status, and lists Requeue + Release', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));

    await waitFor(() => expect(screen.getByText('acme/widgets#42')).toBeInTheDocument());
    expect(screen.getByText('fix-issue')).toBeInTheDocument();
    expect(screen.getByText('Await Human')).toBeInTheDocument();
    expect(screen.getByText('Requeue')).toBeInTheDocument();
    expect(screen.getByText('Release')).toBeInTheDocument();
  });

  it('shows the empty state when there are no claims', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));

    await waitFor(() => expect(screen.getByText('No active claims')).toBeInTheDocument());
  });

  it('calls releaseClaim(issue_ref) when Release is confirmed', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);
    const release = vi.spyOn(api, 'releaseClaim').mockResolvedValue({ released: true });
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));
    await waitFor(() => expect(screen.getByText('Release')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Release'));
    await waitFor(() => expect(release).toHaveBeenCalledWith('github:acme/widgets#42'));
  });

  it('calls requeueClaim(issue_ref) when Requeue is confirmed', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);
    const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'wake-1' });
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));
    await waitFor(() => expect(screen.getByText('Requeue')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Requeue'));
    await waitFor(() => expect(requeue).toHaveBeenCalledWith('github:acme/widgets#42'));
  });
});

describe('AutoflowRuns host filter — server-driven (runs + cycles tabs)', () => {
  it('default fetch passes host: "local" to both events and runs', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    const eventsSpy = vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    const runsSpy = vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(eventsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' })),
    );
    expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' }));
  });

  it('"All hosts" option fetches without a host param', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    const eventsSpy = vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: '__all__' } });

    await waitFor(() => {
      const calls = eventsSpy.mock.calls;
      const lastParams = calls[calls.length - 1]?.[0];
      expect(lastParams?.host).toBeUndefined();
    });
  });

  it('Host column renders host_id on the Launched runs tab', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([REMOTE_EVENT]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByText('host_prod')).toBeInTheDocument());
  });

  it('a launched-run row links to the shared /runs/:id route (RunDetail)', async () => {
    const eventWithRun: AutoflowEventRow = {
      event_id: 'evt-2',
      cycle_id: 'cyc-2',
      at: '2026-06-01T00:00:00Z',
      kind: 'run_launched',
      workflow: 'fix-issue',
      run_id: 'run-9',
      usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 1 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([eventWithRun]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    const link = await screen.findByRole('link', { name: /run-9/ });
    expect(link).toHaveAttribute('href', '/runs/run-9');
  });

  it('a launched-run row on a remote host links to /runs/:id?host=<id>', async () => {
    const remoteRunEvent: AutoflowEventRow = {
      event_id: 'evt-3',
      cycle_id: 'cyc-3',
      at: '2026-06-01T00:00:00Z',
      kind: 'run_launched',
      workflow: 'fix-issue',
      run_id: 'run-10',
      host_id: 'host_prod',
      usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 1 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([remoteRunEvent]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    const link = await screen.findByRole('link', { name: /run-10/ });
    expect(link).toHaveAttribute('href', '/runs/run-10?host=host_prod');
  });

  it('host filter is NOT shown on the Claims tab', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);

    renderPage();
    fireEvent.click(screen.getByText('Claims'));

    await waitFor(() => expect(screen.getByText('No active claims')).toBeInTheDocument());
    expect(screen.queryByLabelText('Host filter')).not.toBeInTheDocument();
  });
});
