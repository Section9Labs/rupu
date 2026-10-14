// @vitest-environment jsdom
// AutoflowRuns — one Workflows-shaped table (spec 2026-10-14, CP autoflow
// runs as one table): autoflow-launched runs plus failed attempts, newest
// first, per-host loading, a problems strip above, claim actions on an
// attempt row that has no run to open.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi, type Mock } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError, type AutoflowProblem, type AutoflowRunRow, type HostView } from '../../lib/api';
import { callsFor, onlyHost } from '../../lib/perHost/testUtils';
import AutoflowRuns from './AutoflowRuns';
import { scopedEntry, withCustomerScope } from '../../lib/customerScopeTestUtils';

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

const RUN: AutoflowRunRow = {
  id: 'run-9',
  workflow_name: 'fix-issue',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  finished_at: '2026-06-01T00:05:00Z',
  trigger: 'event',
  turns: 3,
  duration_ms: 300000,
  codename: 'cobalt-harbor/heron',
  codename_derived: false,
  host_id: 'local',
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 1 },
  autoflow: {
    issue_ref: 'github:acme/widgets/issues/42',
    issue_display_ref: 'acme/widgets#42',
    issue_url: 'https://example.test/42',
    pr_url: 'https://example.test/pr/7',
  },
};

/** A failed attempt with no run id: its id is a history event id. */
const ATTEMPT: AutoflowRunRow = {
  ...RUN,
  id: 'afe_1',
  status: 'failed',
  turns: 0,
  duration_ms: null,
  usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
  autoflow: { issue_ref: 'github:acme/widgets/issues/43' },
  attempt: { detail: 'dispatch failed: branch exists', cycle_id: 'cyc_1', worker: 'local', claim_status: 'retry_backoff' },
};

/** A launched run the store never recorded: its id is a run id. */
const GHOST: AutoflowRunRow = {
  ...ATTEMPT,
  id: 'run_ghost',
  run_recorded: false,
  attempt: { detail: 'runner died before the record', cycle_id: 'cyc_2', worker: 'local', claim_status: 'blocked' },
};

const PROBLEM: AutoflowProblem = {
  host_id: 'local',
  subject_kind: 'repo',
  subject: 'github:acme/api',
  detail: 'config bad',
  first_seen: '2026-06-01T00:00:00Z',
  last_seen: '2026-06-01T00:30:00Z',
  count: 24,
};

function stub(rows: AutoflowRunRow[], problems: AutoflowProblem[] = []) {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
  vi.spyOn(api, 'getAutoflowRunList').mockImplementation(onlyHost('local', rows));
  // Each host reports only its own problems (every fixture problem is local's).
  vi.spyOn(api, 'getAutoflowProblems').mockImplementation(onlyHost('local', problems));
}

function renderPage(entry = '/') {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      {withCustomerScope(<AutoflowRuns />)}
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — one table', () => {
  it('shows one table: no Runs/Cycles/Claims switcher', async () => {
    stub([RUN]);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
    expect(screen.queryByText('Cycles')).not.toBeInTheDocument();
    expect(screen.queryByText('Claims')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'acme/widgets#42' })).toHaveAttribute('href', 'https://example.test/42');
    expect(screen.getByRole('link', { name: /PR/ })).toHaveAttribute('href', 'https://example.test/pr/7');
  });

  it('a run row links to the shared /runs/:id route', async () => {
    stub([RUN]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
    expect(container.querySelector('a[href="/runs/run-9"]')).not.toBeNull();
  });

  it('an attempt row shows its error and expands to Requeue / Release', async () => {
    stub([ATTEMPT]);
    const requeue = vi.spyOn(api, 'requeueClaim').mockResolvedValue({ wake_id: 'w' });
    const release = vi.spyOn(api, 'releaseClaim').mockResolvedValue({ released: true });
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('dispatch failed: branch exists')).toBeInTheDocument());
    // No run to open: the row expands in place instead of linking.
    expect(container.querySelector('a[href="/runs/afe_1"]')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Archive run afe_1' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Expand row' }));
    expect(await screen.findByText('cyc_1')).toBeInTheDocument();
    expect(screen.getByText('retry_backoff')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Requeue claim' }));
    await waitFor(() => expect(requeue).toHaveBeenCalledWith('github:acme/widgets/issues/43'));
    fireEvent.click(screen.getByRole('button', { name: 'Release claim' }));
    await waitFor(() => expect(release).toHaveBeenCalledWith('github:acme/widgets/issues/43'));
  });

  it('an attempt whose run id was never recorded links to the run page instead', async () => {
    stub([GHOST]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('runner died before the record')).toBeInTheDocument());
    expect(container.querySelector('a[href="/runs/run_ghost"]')).not.toBeNull();
    expect(screen.queryByRole('button', { name: 'Expand row' })).not.toBeInTheDocument();
  });

  it('a run row\'s workflow cell is the keyboard-focusable link to the run', async () => {
    stub([RUN]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
    // Every cell of a run row links to the run; only the subject cell is in
    // the tab order (the others are mouse-only), and that must be Workflow.
    const focusable = Array.from(container.querySelectorAll('a[href="/runs/run-9"]')).filter(
      (a) => a.getAttribute('tabindex') !== '-1',
    );
    expect(focusable).toHaveLength(1);
    expect(focusable[0]).toHaveTextContent('fix-issue');
  });

  it('the filter button next to a workflow refetches narrowed to it, shown as a removable chip', async () => {
    stub([RUN]);
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Show only fix-issue runs' }));
    await waitFor(() =>
      expect((api.getAutoflowRunList as Mock).mock.calls.some((c) => c[0]?.workflow === 'fix-issue')).toBe(true),
    );
    const chip = screen.getByRole('button', { name: /workflow: fix-issue/ });
    expect(chip).toBeInTheDocument();
    fireEvent.click(chip);
    await waitFor(() => expect(screen.queryByRole('button', { name: /workflow: fix-issue/ })).not.toBeInTheDocument());
  });

  it('renders the problems strip and hides it when empty', async () => {
    stub([RUN], [PROBLEM]);
    renderPage();
    expect(await screen.findByText(/24 cycles/)).toBeInTheDocument();
    expect(screen.getByText('config bad')).toBeInTheDocument();
    cleanup();
    vi.restoreAllMocks();
    stub([RUN], []);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
    expect(screen.queryByText(/cycles/)).not.toBeInTheDocument();
  });

  it('unavailable host slice: a 501 from one host is shown, not an error', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    vi.spyOn(api, 'getAutoflowProblems').mockResolvedValue([]);
    vi.spyOn(api, 'getAutoflowRunList').mockImplementation((p) =>
      p?.host === 'host_prod' ? Promise.reject(new ApiError(501, 'unsupported')) : Promise.resolve([RUN]),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());
    await waitFor(() => expect(screen.getAllByText(/unavailable/i).length).toBeGreaterThan(0));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('All hosts by default: the list is fetched per registered host', async () => {
    stub([]);
    renderPage();
    await waitFor(() => expect(callsFor(api.getAutoflowRunList as never, 'local')).toHaveLength(1));
    expect(callsFor(api.getAutoflowRunList as never, 'host_prod')).toHaveLength(1);
  });

  it('Find narrows over the issue ref and the attempt error too', async () => {
    stub([RUN, ATTEMPT]);
    renderPage();
    await waitFor(() => expect(screen.getByText('dispatch failed: branch exists')).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Find runs'), { target: { value: 'branch exists' } });
    expect(screen.getByText('dispatch failed: branch exists')).toBeInTheDocument();
    expect(screen.queryByText('acme/widgets#42')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Find runs'), { target: { value: 'widgets#42' } });
    expect(screen.getByText('acme/widgets#42')).toBeInTheDocument();
    expect(screen.queryByText('dispatch failed: branch exists')).not.toBeInTheDocument();
  });

  it('under a customer scope the list is scoped and the page says attempts are left out', async () => {
    stub([RUN]);
    renderPage(scopedEntry('acme'));
    await waitFor(() =>
      expect((api.getAutoflowRunList as Mock).mock.calls.some((c) => c[0]?.customer === 'acme')).toBe(true),
    );
    expect(screen.getByText(/Failed attempts aren’t listed while a customer is selected/)).toBeInTheDocument();
  });
});
