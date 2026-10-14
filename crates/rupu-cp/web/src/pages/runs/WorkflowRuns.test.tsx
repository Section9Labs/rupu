// @vitest-environment jsdom
// WorkflowRuns — One Control Language migration (Phase 2, Task C), then
// per-host progressive loading (spec 2026-10-01).
// Covers: every registered host with one request each and honest per-host
// loading/offline state; kit empty/loading states; the row-action
// (archive/restore/delete) ring buttons; the customer scope. The query line
// itself is WorkflowRuns.query.test.tsx.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError } from '../../lib/api';
import type { RunListRow } from '../../lib/api';
import { REG_LOCAL, REG_PROD, callsFor, onlyHost } from '../../lib/perHost/testUtils';
import WorkflowRuns from './WorkflowRuns';
import { customerRow, scopedEntry, withCustomerScope, ACME } from '../../lib/customerScopeTestUtils';
import { useCustomerScope } from '../../lib/customerScope';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function stubDeps() {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      {withCustomerScope(<WorkflowRuns />)}
    </MemoryRouter>,
  );
}

function makeRun(overrides: Partial<RunListRow>): RunListRow {
  return {
    codename: 'cobalt-harbor/heron#1', codename_derived: false,
    id: 'run_1',
    workflow_name: 'deploy-prod',
    status: 'completed',
    started_at: '2026-07-20T10:00:00Z',
    finished_at: '2026-07-20T10:05:00Z',
    trigger: 'manual',
    turns: 3,
    duration_ms: 300_000,
    usage: {
      input_tokens: 1000,
      output_tokens: 500,
      cached_tokens: 0,
      total_tokens: 1500,
      cost_usd: 0.12,
      priced: true,
      runs: 1,
    },
    host_id: 'local',
    ...overrides,
  };
}

describe('WorkflowRuns — per-host loading (spec 2026-10-01)', () => {
  it('fetches every registered host with its own host param', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
  });

  it('paints local rows while a remote host is still loading', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([makeRun({ id: 'run_l' })]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });

  it('shows an offline remote honestly without hiding local rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([makeRun({ id: 'run_l' })])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent('prod offline: its runs are missing');
    expect(screen.getByText('deploy-prod')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

});

describe('WorkflowRuns — polling', () => {
  it('polls local every 5s, remotes on the 60s cadence', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await vi.waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    await vi.advanceTimersByTimeAsync(5000);
    expect(callsFor(runsSpy, 'local').length).toBeGreaterThanOrEqual(2);
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1); // remotes wait for the 60s cadence
    vi.useRealTimers();
  });
});

describe('WorkflowRuns — kit empty/loading states', () => {
  it('renders the kit EmptyState with the existing copy when there are no runs at all', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();

    await waitFor(() => expect(screen.getByText('No workflow runs')).toBeInTheDocument());
    expect(screen.getByText(/is:archived lists the archive/)).toBeInTheDocument();
  });

  it('says nothing matches when a query keeps no run', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    render(
      <MemoryRouter initialEntries={['/?q=trigger:cron']}>
        {withCustomerScope(<WorkflowRuns />)}
      </MemoryRouter>,
    );
    expect(await screen.findByText('No workflow runs match the filter.')).toBeInTheDocument();
  });

  it('shows the kit Spinner while the first page is in flight', async () => {
    stubDeps();
    let resolveFn: (v: RunListRow[]) => void = () => {};
    vi.spyOn(api, 'getWorkflowRuns').mockReturnValue(
      new Promise((r) => {
        resolveFn = r;
      }),
    );
    renderPage();

    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.getByText('Loading runs…')).toBeInTheDocument();

    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });
});

describe('WorkflowRuns — per-host empty states', () => {
  it('names the host that is still loading while nothing has painted yet', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('Waiting on prod…')).toBeInTheDocument());
    expect(screen.queryByText('No workflow runs')).not.toBeInTheDocument();
  });

  it('says which host is missing when one could not be reached', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent('prod offline: its runs are missing');
  });

  it('says no host answered when every host failed', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockRejectedValue(new ApiError(502, 'x', '{"error":"down"}'));
    renderPage();
    await waitFor(() => expect(screen.getByText('No hosts answered')).toBeInTheDocument());
    expect(screen.getByTestId('host-alert')).toHaveTextContent('Local offline: its runs are missing · prod offline: its runs are missing');
  });
});

describe('WorkflowRuns — table rules (fit columns)', () => {
  it('the Status column is a fit (nowrap) column and renders via StatusPill', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'r1' })]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const statusHeader = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Status'),
    );
    expect(statusHeader?.className).toMatch(/whitespace-nowrap/);

    const statusCell = Array.from(container.querySelectorAll('tbody td')).find(
      (td) => td.textContent?.trim() === 'Completed',
    );
    expect(statusCell?.className).toMatch(/whitespace-nowrap/);
  });

  it('the workflow-name column is the one flexible/truncating subject column', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'r1' })]);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const subjectCell = screen.getByText('deploy-prod').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    // Title tooltip carries the untruncated value.
    expect(subjectCell?.querySelector('[title="deploy-prod"]')).toBeInTheDocument();
  });
});

describe('WorkflowRuns — row actions (ring buttons)', () => {
  it('Archive fires api.archiveRun with the row host and refreshes the list', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    const archiveSpy = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Archive run run_x'));

    await waitFor(() => expect(archiveSpy).toHaveBeenCalledWith('run_x', 'local'));
  });

  it('Delete confirms, then fires api.deleteRun with the row host', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Delete run run_x'));

    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('run_x', 'local'));
  });

  it('Delete does nothing when the confirmation dialog is declined', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Delete run run_x'));

    expect(deleteSpy).not.toHaveBeenCalled();
  });

  it('Archive on a remote-host row fires api.archiveRun with that host id', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_remote', host_id: 'host_prod' }),
    ]);
    const archiveSpy = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Archive run run_remote'));

    await waitFor(() => expect(archiveSpy).toHaveBeenCalledWith('run_remote', 'host_prod'));
  });

  it('Delete on a remote-host row fires api.deleteRun with that host id', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_remote', host_id: 'host_prod' }),
    ]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Delete run run_remote'));

    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('run_remote', 'host_prod'));
  });

  it('Archive on a remote-host row re-syncs only that host, not every host', async () => {
    stubDeps();
    const runsSpy = vi
      .spyOn(api, 'getWorkflowRuns')
      .mockImplementation(onlyHost('host_prod', [makeRun({ id: 'run_remote', host_id: 'host_prod' })]));
    vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());
    expect(callsFor(runsSpy, 'local')).toHaveLength(1);
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);

    fireEvent.click(screen.getByLabelText('Archive run run_remote'));

    await waitFor(() => expect(callsFor(runsSpy, 'host_prod')).toHaveLength(2));
    expect(callsFor(runsSpy, 'local')).toHaveLength(1);
  });
});

// ── Amendment #1 (2026-07-23 feedback round): Find on every table ──────────

describe('WorkflowRuns — Turns/Duration column order (table-standardization Task 3)', () => {
  it('Turns precedes Duration in the header row (matches the canonical Sessions order)', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({})]);

    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    const turnsIdx = headers.indexOf('Turns');
    const durationIdx = headers.indexOf('Duration');
    expect(turnsIdx).toBeGreaterThanOrEqual(0);
    expect(durationIdx).toBeGreaterThan(turnsIdx);
  });
});

describe('WorkflowRuns — codenames', () => {
  it('renders the crew name', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_a', workflow_name: 'deploy-prod', codename: 'cobalt-harbor/heron#1' }),
      makeRun({ id: 'run_b', workflow_name: 'lint-repo', codename: 'amber-fjord/kite#1' }),
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
    expect(screen.getByText('amber-fjord')).toBeInTheDocument();
  });
});

describe('WorkflowRuns — the global customer scope', () => {
  function ScopeProbe() {
    const { scope, notice } = useCustomerScope();
    return (
      <>
        <span data-testid="scope">{String(scope)}</span>
        <span data-testid="notice">{notice ?? ''}</span>
      </>
    );
  }
  function renderScoped(scope = 'acme', customers = [ACME]) {
    return render(
      <MemoryRouter initialEntries={[scopedEntry(scope)]}>
        {withCustomerScope(
          <>
            <WorkflowRuns />
            <ScopeProbe />
          </>,
          { customers },
        )}
      </MemoryRouter>,
    );
  }

  it('passes customer: "acme" on every per-host request, beside the query', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getWorkflowRuns').mockImplementation(onlyHost('local', [makeRun({ id: 'run_a' })]));
    renderScoped();
    await waitFor(() => expect(callsFor(spy, 'local').length).toBeGreaterThan(0));
    for (const host of ['local', 'host_prod']) {
      await waitFor(() => expect(callsFor(spy, host).length).toBeGreaterThan(0));
      expect(callsFor(spy, host)[0][0]).toEqual(
        expect.objectContaining({ customer: 'acme', q: '', onHeaders: expect.any(Function) }),
      );
    }
  });

  it('an unscoped page sends no customer', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getWorkflowRuns').mockImplementation(onlyHost('local', []));
    renderPage();
    await waitFor(() => expect(callsFor(spy, 'local').length).toBeGreaterThan(0));
    expect(callsFor(spy, 'local')[0][0]).not.toHaveProperty('customer');
    expect(screen.queryByTestId('host-alert')).not.toBeInTheDocument();
  });

  it('a slice unavailable with the customer reason is named in the line under the bar', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'host_prod'
        ? Promise.reject(
            new ApiError(501, 'x', JSON.stringify({ error: "host host_prod can't report a customer for every run" })),
          )
        : Promise.resolve([makeRun({ id: 'run_a' })]),
    );
    renderScoped();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent("prod can't filter by customer: its runs are left out");
  });

  it('names the hosts the X-Rupu-Hosts-Without-Customer header lists, by name', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) => {
      if (p?.host === 'local') p.onHeaders?.({ withoutCustomer: ['host_prod'], unfilterable: [] });
      return Promise.resolve([]);
    });
    renderScoped();
    expect(await screen.findByTestId('host-alert')).toHaveTextContent("prod can't filter by customer: its runs are left out");
  });

  it('a 400 for the scope clears it and refetches unfiltered', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.customer
        ? Promise.reject(new ApiError(400, 'x', JSON.stringify({ error: 'customer: invalid slug' })))
        : Promise.resolve([makeRun({ id: 'run_a', workflow_name: 'unscoped-wf' })]),
    );
    // A slug the customer list knows, so only the 400 can clear it.
    renderScoped('Bad', [ACME, customerRow('Bad')]);
    await waitFor(() => expect(screen.getByTestId('scope')).toHaveTextContent('null'));
    expect(screen.getByTestId('notice')).toHaveTextContent(
      'The customer filter was rejected (customer: invalid slug) — showing all customers.',
    );
    expect(await screen.findByText('unscoped-wf')).toBeInTheDocument();
    const last = spy.mock.lastCall?.[0];
    expect(last).not.toHaveProperty('customer');
  });
});

describe('WorkflowRuns — the Customer column', () => {
  const GLOBEX = customerRow('globex');
  function renderWith(path: string, customers = [ACME, GLOBEX]) {
    return render(
      <MemoryRouter initialEntries={[path]}>{withCustomerScope(<WorkflowRuns />, { customers })}</MemoryRouter>,
    );
  }
  const headers = (c: HTMLElement) =>
    Array.from(c.querySelectorAll('thead th')).map((th) => th.textContent?.trim() ?? '');

  it('shows a Customer column after Host when unscoped: dot + name, "—" for none, Unknown when absent', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation(
      onlyHost('local', [
        makeRun({ id: 'run_a', workflow_name: 'wf-a', customer: 'acme' }),
        makeRun({ id: 'run_b', workflow_name: 'wf-b', customer: 'globex', customer_derived: true }),
        makeRun({ id: 'run_c', workflow_name: 'wf-c', customer: null }),
        makeRun({ id: 'run_d', workflow_name: 'wf-d' }),
      ]),
    );
    const { container } = renderWith('/');
    await waitFor(() => expect(screen.getByText('wf-a')).toBeInTheDocument());
    await waitFor(() => expect(headers(container)).toContain('Customer'));
    const h = headers(container);
    expect(h.indexOf('Customer')).toBe(h.indexOf('Host') + 1);
    const row = (name: string) => screen.getByText(name).closest('tr') as HTMLElement;
    expect(row('wf-a')).toHaveTextContent('Acme');
    expect(row('wf-b')).toHaveTextContent('Globex');
    expect(within(row('wf-b')).getByText('Globex').closest('[title]')).toHaveAttribute(
      'title',
      expect.stringMatching(/current customer/i),
    );
    expect(row('wf-c')).not.toHaveTextContent('Unknown');
    expect(row('wf-d')).toHaveTextContent('Unknown');
  });

  it('has no Customer column while scoped (every row is that customer)', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation(
      onlyHost('local', [makeRun({ id: 'run_a', workflow_name: 'wf-a', customer: 'acme' })]),
    );
    const { container } = renderWith(scopedEntry('acme'));
    await waitFor(() => expect(screen.getByText('wf-a')).toBeInTheDocument());
    expect(headers(container)).not.toContain('Customer');
  });

  it('has no Customer column when no customers exist', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation(
      onlyHost('local', [makeRun({ id: 'run_a', workflow_name: 'wf-a' })]),
    );
    const { container } = renderWith('/', []);
    await waitFor(() => expect(screen.getByText('wf-a')).toBeInTheDocument());
    expect(headers(container)).not.toContain('Customer');
  });

  it('shows the column (and names archived customers) when only archived customers exist', async () => {
    stubDeps();
    vi.spyOn(api, 'getCustomers').mockImplementation(async (o) =>
      o?.archived ? [customerRow('oldco', { name: 'Old Co', archived: true })] : [],
    );
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation(
      onlyHost('local', [makeRun({ id: 'run_a', workflow_name: 'wf-a', customer: 'oldco' })]),
    );
    const { container } = renderWith('/', []);
    await waitFor(() => expect(screen.getByText('wf-a')).toBeInTheDocument());
    await waitFor(() => expect(headers(container)).toContain('Customer'));
    expect(await screen.findByText('Old Co')).toBeInTheDocument();
  });
});
