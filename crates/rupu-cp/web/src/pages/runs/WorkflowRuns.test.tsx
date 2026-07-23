// @vitest-environment jsdom
// WorkflowRuns — One Control Language migration (Phase 2, Task C).
// Covers: host filter drives the server request (not client-side fan-out
// filtering); the FilterBar slot order; lifecycle/trigger FilterPills
// driving fetch params + client-side filtering; kit empty/loading states;
// and the row-action (archive/restore/delete) ring buttons.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { HostView, RunListRow } from '../../lib/api';
import WorkflowRuns from './WorkflowRuns';

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
  active_run_count: 2,
};

function stubDeps() {
  vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST, REMOTE_HOST]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      <WorkflowRuns />
    </MemoryRouter>,
  );
}

function makeRun(overrides: Partial<RunListRow>): RunListRow {
  return {
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

describe('WorkflowRuns host filter — server-driven', () => {
  it('default fetch is called with host: "local" (fast path, not fan-out)', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();

    await waitFor(() =>
      expect(runsSpy).toHaveBeenCalledWith(expect.objectContaining({ host: 'local' })),
    );
  });

  it('renders This host, registered (non-local) hosts, and All hosts — via the shared HostSelect', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument());

    const options = screen.getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.textContent)).toEqual(['This host', 'All hosts', 'prod']);
  });

  it('"All hosts" option fetches without a host param (fan-out branch)', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();
    await waitFor(() => expect(screen.getByLabelText('Host filter')).toBeInTheDocument());

    fireEvent.change(screen.getByLabelText('Host filter'), { target: { value: '__all__' } });

    await waitFor(() => {
      const calls = runsSpy.mock.calls;
      const lastParams = calls[calls.length - 1]?.[0];
      expect(lastParams?.host).toBeUndefined();
    });
  });

  it('remote host option fetches with that host id', async () => {
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);

    renderPage();
    // Wait for the remote host option to appear (fetched from api.getHosts).
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'prod' })).toBeInTheDocument(),
    );

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

  it('only the active/Running lifecycle polls every 5s (unchanged semantics)', async () => {
    vi.useFakeTimers();
    stubDeps();
    const runsSpy = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    renderPage();

    await vi.waitFor(() => expect(runsSpy).toHaveBeenCalledTimes(1));

    await vi.advanceTimersByTimeAsync(5000);
    expect(runsSpy.mock.calls.length).toBeGreaterThanOrEqual(2);

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
  it('Archive fires api.archiveRun and refreshes the list', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    const archiveSpy = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Archive run run_x'));

    await waitFor(() => expect(archiveSpy).toHaveBeenCalledWith('run_x'));
  });

  it('Delete confirms, then fires api.deleteRun', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([makeRun({ id: 'run_x' })]);
    const deleteSpy = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Delete run run_x'));

    await waitFor(() => expect(deleteSpy).toHaveBeenCalledWith('run_x'));
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

  it('Restore appears in Archived mode and fires api.restoreRun', async () => {
    stubDeps();
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([]);
    vi.spyOn(api, 'getArchivedRuns').mockResolvedValue([makeRun({ id: 'run_arch' })]);
    const restoreSpy = vi.spyOn(api, 'restoreRun').mockResolvedValue(undefined);
    renderPage();
    await waitFor(() => expect(screen.getByText('Archived')).toBeInTheDocument());
    fireEvent.click(screen.getByText('Archived'));
    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('Restore run run_arch'));

    await waitFor(() => expect(restoreSpy).toHaveBeenCalledWith('run_arch'));
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
