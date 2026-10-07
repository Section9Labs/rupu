// @vitest-environment jsdom
// WorkflowRuns — One Control Language migration (Phase 2, Task C), then
// per-host progressive loading (spec 2026-10-01).
// Covers: All hosts by default with one request per registered host and
// honest per-host loading/offline state; the FilterBar slot order; lifecycle/trigger FilterPills
// driving fetch params + client-side filtering; kit empty/loading states;
// and the row-action (archive/restore/delete) ring buttons.

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

describe('WorkflowRuns archived mode — kind-filtered fetch', () => {
  it('clicking Archived calls getArchivedRuns with kind="workflow"', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    const archivedSpy = vi.spyOn(api, 'getArchivedRuns').mockResolvedValue([]);

    renderPage();
    // Wait for initial active-tab fetch to settle.
    await waitFor(() => expect(screen.getByText('Archived')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Archived'));

    await waitFor(() =>
      expect(archivedSpy).toHaveBeenCalledWith('workflow'),
    );
  });
});

describe('WorkflowRuns host filter — per-host loading (spec 2026-10-01)', () => {
  it('defaults to All hosts and fetches every registered host with its own host param', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(1);
    expect(screen.getByLabelText('Host filter')).toHaveValue('__all__');
  });

  it('renders This host, All hosts, then registered (non-local) hosts', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
  });

  it('paints local rows while a remote host is still loading, naming it in the strip', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve([makeRun({ id: 'run_l' })]) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());
    expect(screen.getByText('loading…')).toBeInTheDocument();
  });

  it('shows an offline remote honestly without hiding local rows', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([makeRun({ id: 'run_l' })])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText(/not included: prod \(offline\)/)).toBeInTheDocument());
    expect(screen.getByText('deploy-prod')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('This host fetches only local', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    runsSpy.mockClear();
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'local' } });
    await waitFor(() => expect(callsFor(runsSpy, 'local')).toHaveLength(1));
    expect(callsFor(runsSpy, 'host_prod')).toHaveLength(0);
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: 'host_prod' } });
    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })),
    );
  });
});

describe('WorkflowRuns — FilterBar slot order', () => {
  it('renders lifecycle pills, then trigger pills, then the host select, in that fixed order', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    const LABELS = [
      'Running', 'Completed', 'Failed / Rejected', 'Archived',
      'All', 'Manual', 'Cron', 'Event',
    ];
    const controls = Array.from(container.querySelectorAll('button, select'))
      .map((el) => (el.tagName === 'SELECT' ? 'HOST_SELECT' : el.textContent))
      .filter((c): c is string => c !== null && (LABELS.includes(c) || c === 'HOST_SELECT'));

    expect(controls).toEqual([...LABELS, 'HOST_SELECT']);
  });

  it('hides the trigger pills and the host select in Archived mode', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    vi.spyOn(api, 'getArchivedRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() => expect(screen.getByText('Archived')).toBeInTheDocument());

    fireEvent.click(screen.getByText('Archived'));

    await waitFor(() => expect(screen.queryByLabelText('Host filter')).not.toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Manual' })).not.toBeInTheDocument();
    // The lifecycle group stays visible — it's how you get back out of Archived.
    expect(screen.getByRole('button', { name: 'Running' })).toBeInTheDocument();
  });
});

describe('WorkflowRuns — lifecycle FilterPills drives the fetch', () => {
  it('clicking the Completed pill re-fetches with lifecycle: "completed"', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();
    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ lifecycle: 'active' })),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Completed' }));

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ lifecycle: 'completed' })),
    );
  });

  it('only the active/Running lifecycle polls local every 5s', async () => {
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

describe('WorkflowRuns — trigger FilterPills filters client-side', () => {
  it('selecting Cron hides non-cron rows without a new lifecycle re-fetch of a different shape', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'r1', workflow_name: 'wf-manual', trigger: 'manual' }),
      makeRun({ id: 'r2', workflow_name: 'wf-cron', trigger: 'cron' }),
    ]);
    renderPage();
    await waitFor(() => expect(screen.getByText('wf-manual')).toBeInTheDocument());
    expect(screen.getByText('wf-cron')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Cron' }));

    await waitFor(() => expect(screen.queryByText('wf-manual')).not.toBeInTheDocument());
    expect(screen.getByText('wf-cron')).toBeInTheDocument();
  });
});

describe('WorkflowRuns — kit empty/loading states', () => {
  it('renders the kit EmptyState with the existing copy when there are no runs at all', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();

    await waitFor(() => expect(screen.getByText('No workflow runs yet')).toBeInTheDocument());
    expect(
      screen.getByText(/Workflow runs will appear here once you dispatch one/),
    ).toBeInTheDocument();
  });

  it('renders the "no match" EmptyState copy when a filter narrows an existing page to zero', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'r1', trigger: 'manual' }),
    ]);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Cron' }));

    await waitFor(() => expect(screen.getByText('No runs match this filter')).toBeInTheDocument());
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
    expect(screen.queryByText('No workflow runs yet')).not.toBeInTheDocument();
  });

  it('does not claim "no runs yet" when a host could not be reached', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local'
        ? Promise.resolve([])
        : Promise.reject(new ApiError(502, 'x', '{"error":"host unreachable: timed out"}')),
    );
    renderPage();
    await waitFor(() =>
      expect(screen.getByText('No workflow runs on the hosts that answered')).toBeInTheDocument(),
    );
    expect(screen.getByText('Not included: prod (offline).')).toBeInTheDocument();
    expect(screen.queryByText('No workflow runs yet')).not.toBeInTheDocument();
  });
});

describe('WorkflowRuns — the trigger filter while hosts are still loading or paging', () => {
  /** `n` manual runs, newest first, one a minute. */
  const manualRuns = (n: number): RunListRow[] =>
    Array.from({ length: n }, (_, i) =>
      makeRun({
        id: `run_m${i}`,
        workflow_name: `wf-manual-${i}`,
        trigger: 'manual',
        started_at: new Date(Date.parse('2026-07-20T12:00:00Z') - i * 60_000).toISOString(),
      }),
    );

  it('says it is still waiting, not "no match", when the filter empties what loaded so far', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'local' ? Promise.resolve(manualRuns(2)) : new Promise(() => {}),
    );
    renderPage();
    await waitFor(() => expect(screen.getByText('wf-manual-0')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Cron' }));
    await waitFor(() => expect(screen.getByText('No matches yet · Waiting on prod…')).toBeInTheDocument());
    expect(screen.queryByText('No runs match this filter')).not.toBeInTheDocument();
    // Said once: the footer below does not repeat who it is waiting on.
    expect(screen.getAllByText(/waiting on prod/i)).toHaveLength(1);
  });

  it('keeps paging while the filter hides every loaded row, so a later page can match', async () => {
    stubDeps();
    const local = [
      ...manualRuns(22),
      makeRun({ id: 'run_cron', workflow_name: 'wf-nightly', trigger: 'cron', started_at: '2026-07-19T00:00:00Z' }),
    ];
    let releaseFirstPage!: () => void;
    const firstPage = new Promise<void>((r) => (releaseFirstPage = r));
    const spy = vi.spyOn(api, 'getWorkflowRuns').mockImplementation(async (p) => {
      if (p?.host !== 'local') return [];
      const offset = p.offset ?? 0;
      if (offset === 0) await firstPage;
      return local.slice(offset, offset + (p.limit ?? 20));
    });
    renderPage();
    // Pick Cron before anything has loaded: page 0 (all manual) lands straight in the filter-empty state.
    fireEvent.click(screen.getByRole('button', { name: 'Cron' }));
    releaseFirstPage();
    await waitFor(() => expect(screen.getByText('wf-nightly')).toBeInTheDocument());
    expect(callsFor(spy, 'local').some((c) => ((c[0] as { offset?: number }).offset ?? 0) > 0)).toBe(true);
  });

  it('says no host answered when every host failed', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockRejectedValue(new ApiError(502, 'x', '{"error":"down"}'));
    renderPage();
    await waitFor(() => expect(screen.getByText('No hosts answered')).toBeInTheDocument());
    expect(screen.queryByText('No workflow runs on the hosts that answered')).not.toBeInTheDocument();
    expect(screen.getByText('Not included: Local (offline), prod (offline).')).toBeInTheDocument();
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

  it('Restore appears in Archived mode and fires api.restoreRun with the row host', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    vi.spyOn(api, 'getArchivedRuns').mockResolvedValue([makeRun({ id: 'run_arch' })]);
    const restoreSpy = vi.spyOn(api, 'restoreRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('Archived')).toBeInTheDocument());
    fireEvent.click(screen.getByText('Archived'));
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Restore run run_arch'));

    await waitFor(() => expect(restoreSpy).toHaveBeenCalledWith('run_arch', 'local'));
  });

  // Operator-reported gap this task fixes: a fanned-out remote-host row's
  // Archive/Delete must proxy to THAT host, not silently hit the local store.
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

describe('WorkflowRuns — Find', () => {
  it('typing narrows rows by workflow name, run id, or host id', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_a', workflow_name: 'deploy-prod' }),
      makeRun({ id: 'run_b', workflow_name: 'lint-repo' }),
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText('lint-repo')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'lint' } });

    await waitFor(() => expect(screen.queryByText('deploy-prod')).not.toBeInTheDocument());
    expect(screen.getByText('lint-repo')).toBeInTheDocument();
  });

  it('footer shows "N matches of M loaded" while a query is active', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_a', workflow_name: 'deploy-prod' }),
      makeRun({ id: 'run_b', workflow_name: 'lint-repo' }),
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText('lint-repo')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'lint' } });

    await waitFor(() => expect(screen.getByText('1 matches of 2 loaded')).toBeInTheDocument());
  });

  it('Esc clears the query', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_a', workflow_name: 'deploy-prod' }),
      makeRun({ id: 'run_b', workflow_name: 'lint-repo' }),
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText('lint-repo')).toBeInTheDocument());

    const input = screen.getByPlaceholderText('Find runs…') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'lint' } });
    await waitFor(() => expect(screen.queryByText('deploy-prod')).not.toBeInTheDocument());

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(input.value).toBe(''));
    expect(screen.getByText('deploy-prod')).toBeInTheDocument();
  });

  it('composes with the trigger pill: searching narrows within the active pill filter', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_a', workflow_name: 'deploy-prod', trigger: 'manual' }),
      makeRun({ id: 'run_b', workflow_name: 'deploy-staging', trigger: 'cron' }),
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-staging')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Cron' }));
    await waitFor(() => expect(screen.queryByText('deploy-prod')).not.toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'prod' } });

    // "prod" matches "deploy-prod" (hidden by the Cron pill) but not the
    // Cron-filtered "deploy-staging" row still loaded.
    await waitFor(() => expect(screen.queryByText('deploy-staging')).not.toBeInTheDocument());
    expect(screen.queryByText('deploy-prod')).not.toBeInTheDocument();
  });
});

// ── Table standardization Task 3: Turns before Duration, matching Sessions ──

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
  it('renders the crew name and Find matches by codename', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([
      makeRun({ id: 'run_a', workflow_name: 'deploy-prod', codename: 'cobalt-harbor/heron#1' }),
      makeRun({ id: 'run_b', workflow_name: 'lint-repo', codename: 'amber-fjord/kite#1' }),
    ]);

    renderPage();
    await waitFor(() => expect(screen.getByText('cobalt-harbor')).toBeInTheDocument());
    expect(screen.getByText('amber-fjord')).toBeInTheDocument();

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'cobalt' } });

    await waitFor(() => expect(screen.queryByText('lint-repo')).not.toBeInTheDocument());
    expect(screen.getByText('deploy-prod')).toBeInTheDocument();
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

  it('passes customer: "acme" on every per-host request, with a header sink', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getWorkflowRuns').mockImplementation(onlyHost('local', [makeRun({ id: 'run_a' })]));
    renderScoped();
    await waitFor(() => expect(callsFor(spy, 'local').length).toBeGreaterThan(0));
    for (const host of ['local', 'host_prod']) {
      await waitFor(() => expect(callsFor(spy, host).length).toBeGreaterThan(0));
      expect(callsFor(spy, host)[0][0]).toEqual(
        expect.objectContaining({ customer: 'acme', lifecycle: 'active', onHostsWithoutCustomer: expect.any(Function) }),
      );
    }
    // The archive can't be filtered by customer: no Archived state, and a hint says so.
    expect(screen.queryByText('Archived')).not.toBeInTheDocument();
    expect(screen.getByText(/Archived runs aren’t listed here/)).toBeInTheDocument();
  });

  it('an unscoped page sends no customer and no header sink (requests unchanged)', async () => {
    stubDeps();
    const spy = vi.spyOn(api, 'getWorkflowRuns').mockImplementation(onlyHost('local', []));
    renderPage();
    await waitFor(() => expect(callsFor(spy, 'local').length).toBeGreaterThan(0));
    expect(Object.keys(callsFor(spy, 'local')[0][0] as object).sort()).toEqual(
      ['host', 'lifecycle', 'limit', 'offset', 'signal'].sort(),
    );
    expect(screen.queryByTestId('hosts-without-customer')).not.toBeInTheDocument();
  });

  it('a slice unavailable with the customer reason renders the banner', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) =>
      p?.host === 'host_prod'
        ? Promise.reject(
            new ApiError(501, 'x', JSON.stringify({ error: "host host_prod can't report a customer for every run" })),
          )
        : Promise.resolve([makeRun({ id: 'run_a' })]),
    );
    renderScoped();
    expect(await screen.findByTestId('hosts-without-customer')).toHaveTextContent(
      'prod runs an older rupu or holds runs from before customers, so it can’t tag every run with a customer — those runs are left out of this view, not counted as zero.',
    );
  });

  it('names the hosts the X-Rupu-Hosts-Without-Customer header lists, by name', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockImplementation((p) => {
      if (p?.host === 'local') p.onHostsWithoutCustomer?.(['host_prod']);
      return Promise.resolve([]);
    });
    renderScoped();
    expect(await screen.findByTestId('hosts-without-customer')).toHaveTextContent(/^prod runs an older rupu/);
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
    // The Archived state is back once unscoped.
    expect(screen.getByText('Archived')).toBeInTheDocument();
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
});
