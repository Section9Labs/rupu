// @vitest-environment jsdom
// AutoflowRuns — mirrors WorkflowRuns: the primary "Runs" tab is a clean run
// list (row click → /runs/:id); "Cycles" (batch view) and "Claims"
// (requeue/release) are secondary tabs. The page's mount fetch (events +
// cycles) is stubbed so tests can switch tabs and drive per-row actions.
// Also tests the host filter that drives server-side fetch scope for the
// Runs and Cycles tabs.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import {
  api,
  type AutoflowClaim,
  type AutoflowCycleRow,
  type AutoflowEventRow,
  type HostView,
} from '../../lib/api';
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

const CYCLE: AutoflowCycleRow = {
  cycle_id: 'cyc-42',
  mode: 'bypass',
  worker_name: 'worker-1',
  started_at: '2026-06-01T00:00:00Z',
  finished_at: '2026-06-01T00:05:00Z',
  workflow_count: 2,
  ran_cycles: 2,
  skipped_cycles: 0,
  failed_cycles: 0,
  run_ids: ['run-9'],
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 2 },
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

  it('renders This host, registered (non-local) hosts, and All hosts — via the shared HostSelect', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());

    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
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

  it('Host column renders host_id on the Runs tab', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([REMOTE_EVENT]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByText('host_prod')).toBeInTheDocument());
  });

  it('a run row links to the shared /runs/:id route (RunDetail)', async () => {
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

  it('a run row on a remote host links to /runs/:id?host=<id>', async () => {
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

  it('the whole row is clickable, not just the Run/Workflow cells — a plain cell (Host) is also link-wrapped to the same /runs/:id', async () => {
    const eventWithRun: AutoflowEventRow = {
      event_id: 'evt-4',
      cycle_id: 'cyc-4',
      at: '2026-06-01T00:00:00Z',
      kind: 'run_launched',
      workflow: 'fix-issue',
      run_id: 'run-11',
      usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 1 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([eventWithRun]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    // The Host cell has no special link handling of its own (it's a plain
    // `<span>local</span>`) — SortableTable's rowHref wraps EVERY cell of a
    // non-expandable row, so this proves the whole row is link-wrapped, not
    // just cells that happen to render their own navigation.
    const link = await screen.findByRole('link', { name: 'local' });
    expect(link).toHaveAttribute('href', '/runs/run-11');
  });

  it('Runs is the default/primary tab (rendered without clicking a tab)', async () => {
    stubPage();
    renderPage();

    await waitFor(() => expect(screen.getByText('No autoflow activity yet')).toBeInTheDocument());
    // Cycles/Claims content is not rendered until their tab is selected.
    expect(screen.queryByText('No autoflow cycles yet')).not.toBeInTheDocument();
  });

  it('Cycles tab is reachable and renders the batch cycle view', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([CYCLE]);

    renderPage();
    fireEvent.click(screen.getByText('Cycles'));

    await waitFor(() => expect(screen.getByText('worker-1')).toBeInTheDocument());
    // The cycle's spawned run id is exposed via the expandable detail row.
    fireEvent.click(screen.getByLabelText('Expand row'));
    const link = await screen.findByRole('link', { name: /run-9/ });
    expect(link).toHaveAttribute('href', '/runs/run-9');
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

describe('AutoflowRuns — Segmented view control (One Control Language kit)', () => {
  it('renders as a single Segmented group with Runs/Cycles/Claims options, Runs active by default', async () => {
    stubPage();
    renderPage();
    await waitFor(() => expect(screen.getByText('No autoflow activity yet')).toBeInTheDocument());

    const group = screen.getByRole('group', { name: 'View' });
    const options = within(group).getAllByRole('button');
    expect(options.map((o) => o.textContent)).toEqual(['Runs', 'Cycles', 'Claims']);
    expect(within(group).getByRole('button', { name: 'Runs' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(group).getByRole('button', { name: 'Cycles' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('clicking a Segmented option switches the active tab (aria-pressed flips)', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByText('No autoflow activity yet')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Claims' }));

    await waitFor(() => expect(screen.getByText('No active claims')).toBeInTheDocument());
    const group = screen.getByRole('group', { name: 'View' });
    expect(within(group).getByRole('button', { name: 'Claims' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(group).getByRole('button', { name: 'Runs' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('FilterBar slot order: Segmented view control first, then the host scope select', async () => {
    stubPage();
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    const controls = Array.from(container.querySelectorAll('button, select')).filter(
      (el) => el.tagName === 'SELECT' || ['Runs', 'Cycles', 'Claims'].includes(el.textContent ?? ''),
    );
    const order = controls.map((el) => (el.tagName === 'SELECT' ? 'HOST_SELECT' : el.textContent));
    expect(order).toEqual(['Runs', 'Cycles', 'Claims', 'HOST_SELECT']);
  });
});

describe('AutoflowRuns — kit loading state', () => {
  it('shows the kit Spinner while the Runs tab first page is in flight', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    let resolveFn: (v: AutoflowEventRow[]) => void = () => {};
    vi.spyOn(api, 'getAutoflowEvents').mockReturnValue(
      new Promise((r) => {
        resolveFn = r;
      }),
    );
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading autoflow activity…')).toBeInTheDocument();

    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });
});

describe('AutoflowRuns — table rules (fit/subject columns)', () => {
  it('the Workflow column is the one flexible/truncating subject column on the Runs tab', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([REMOTE_EVENT]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    const subjectCell = screen.getByText('fix-issue').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="fix-issue"]')).toBeInTheDocument();
  });

  it('Run/Event/Host/Status columns are fit (nowrap) on the Runs tab', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([REMOTE_EVENT]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    for (const label of ['Run', 'Event', 'Host']) {
      const th = Array.from(container.querySelectorAll('thead th')).find((el) =>
        el.textContent?.includes(label),
      );
      expect(th?.className).toMatch(/whitespace-nowrap/);
    }
  });

  it('Cycle detail table columns are all fit (no dominant subject column)', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([CYCLE]);
    renderPage();
    fireEvent.click(screen.getByRole('button', { name: 'Cycles' }));
    await waitFor(() => expect(screen.getByText('worker-1')).toBeInTheDocument());

    const cell = screen.getByText('worker-1').closest('td');
    expect(cell?.className).toMatch(/whitespace-nowrap/);
    expect(cell?.className).not.toMatch(/max-w-0/);
  });

  it('the Claims Issue Ref column is the subject column (max-w-0 + title tooltip)', async () => {
    stubPage();
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([CLAIM]);
    renderPage();
    fireEvent.click(screen.getByText('Claims'));
    await waitFor(() => expect(screen.getByText('acme/widgets#42')).toBeInTheDocument());

    const subjectCell = screen.getByText('acme/widgets#42').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
  });
});

describe('AutoflowRuns — Event column (cycle_failed detail + issue ref fallback)', () => {
  it('a cycle_failed row with detail expands and shows the error text', async () => {
    const failedEvent: AutoflowEventRow = {
      event_id: 'evt-fail-1',
      cycle_id: 'cyc-fail-1',
      at: '2026-06-01T00:00:00Z',
      kind: 'cycle_failed',
      detail: 'workflow validation failed: missing step "build"',
      usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([failedEvent]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() => expect(screen.getByText('CYCLE FAILED')).toBeInTheDocument());
    expect(screen.getByText('Event')).toBeInTheDocument();
    expect(screen.queryByText(/missing step/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByLabelText('Expand row'));
    expect(await screen.findByText(/missing step "build"/)).toBeInTheDocument();
  });

  it('issue ref renders via the issue_ref fallback when issue_display_ref is absent', async () => {
    const failedEvent: AutoflowEventRow = {
      event_id: 'evt-fail-2',
      cycle_id: 'cyc-fail-2',
      at: '2026-06-01T00:00:00Z',
      kind: 'cycle_failed',
      issue_display_ref: 'github:acme/widgets#7',
      usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([failedEvent]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    expect(await screen.findByText('github:acme/widgets#7')).toBeInTheDocument();
  });

  it('a run_launched row still shows its status/usage, with no Event-column detail row', async () => {
    const launched: AutoflowEventRow = {
      event_id: 'evt-launch-1',
      cycle_id: 'cyc-launch-1',
      at: '2026-06-01T00:00:00Z',
      kind: 'run_launched',
      workflow: 'fix-issue',
      run_id: 'run-77',
      status: 'running',
      usage: { input_tokens: 120, output_tokens: 40, cached_tokens: 0, total_tokens: 160, cost_usd: 0.05, priced: true, runs: 1 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([launched]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();

    expect(await screen.findByText('Running')).toBeInTheDocument();
    expect(screen.getByText('120')).toBeInTheDocument();

    // No `detail` on this event → the row is NOT expandable (no chevron);
    // it falls through to rowHref's per-row link-wrapping instead.
    expect(screen.queryByLabelText('Expand row')).not.toBeInTheDocument();
    const link = await screen.findByRole('link', { name: 'Running' });
    expect(link).toHaveAttribute('href', '/runs/run-77');
  });

  it('a non-run event (no run_id) renders empty Run/Worker/Status/tokens/Cost cells, not dashes', async () => {
    const failedEvent: AutoflowEventRow = {
      event_id: 'evt-fail-3',
      cycle_id: 'cyc-fail-3',
      at: '2026-06-01T00:00:00Z',
      kind: 'cycle_failed',
      detail: 'boom',
      issue_display_ref: 'github:acme/widgets#9',
      usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
    };
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([failedEvent]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByText('CYCLE FAILED')).toBeInTheDocument());

    // Scope to this row specifically (not the whole document — the Issue
    // Ref column elsewhere in the table is allowed its own '—' fallback):
    // none of the run-shaped cells (Run/Worker/Status/tokens/Cost) should
    // render a literal dash for a non-run event.
    const row = screen.getByText('CYCLE FAILED').closest('tr');
    expect(row).not.toBeNull();
    expect(within(row as HTMLElement).queryByText('—')).not.toBeInTheDocument();
  });
});
