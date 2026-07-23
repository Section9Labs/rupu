// @vitest-environment jsdom
// ProjectRunsTab — One Control Language migration (Phase 3, Task G).
// Covers: the FilterBar renders with no host slot (project tabs are already
// workspace-scoped); the Status/Trigger FilterPills groups narrow the loaded
// rows client-side (composing with each other); Find narrows further with
// the matches footer; fit/subject table-rules columns; kit loading/empty/
// error states.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type RunListRow } from '../../lib/api';
import ProjectRunsTab from './ProjectRunsTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function usage(): RunListRow['usage'] {
  return {
    input_tokens: 100,
    output_tokens: 20,
    cached_tokens: 0,
    total_tokens: 120,
    cost_usd: 0.01,
    priced: true,
    runs: 1,
  };
}

const ROWS: RunListRow[] = [
  {
    id: 'r-run-manual',
    workflow_name: 'wf-running-manual',
    status: 'running',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'manual',
    turns: 1,
    usage: usage(),
  },
  {
    id: 'r-done-cron',
    workflow_name: 'wf-completed-cron',
    status: 'completed',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'cron',
    turns: 2,
    usage: usage(),
  },
  {
    id: 'r-fail-event',
    workflow_name: 'wf-failed-event',
    status: 'failed',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'event',
    turns: 3,
    usage: usage(),
  },
  {
    id: 'r-await-manual',
    workflow_name: 'wf-awaiting-manual',
    status: 'awaiting_approval',
    started_at: '2026-06-01T00:00:00Z',
    trigger: 'manual',
    turns: 1,
    usage: usage(),
  },
];

function mockRuns(rows: RunListRow[] = ROWS): void {
  vi.spyOn(api, 'getProjectRuns').mockImplementation((_wsId, params) => {
    // Page 0 returns all rows; any subsequent offset returns empty.
    if (params?.offset && params.offset > 0) return Promise.resolve([]);
    return Promise.resolve(rows);
  });
  // ProjectUsageTimeline (Task U4) fetches independently of the run list
  // above — mock it too so this test isn't tripped up by an unmocked call.
  vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
}

function renderTab(wsId = 'x') {
  return render(
    <MemoryRouter>
      <ProjectRunsTab wsId={wsId} />
    </MemoryRouter>,
  );
}

describe('ProjectRunsTab — FilterBar (no host slot)', () => {
  it('renders the Status and Trigger FilterPills groups and a Find input, with no HostSelect', async () => {
    mockRuns();
    renderTab();

    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());

    const statusAll = screen.getAllByRole('button', { name: 'All' })[0];
    expect(statusAll).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Running' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Manual' })).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Find runs…')).toBeInTheDocument();
    expect(screen.queryByLabelText('Host filter')).not.toBeInTheDocument();
  });
});

describe('ProjectRunsTab — Status/Trigger FilterPills narrow rows', () => {
  it('renders all rows then narrows via status and trigger pills, composing the two groups', async () => {
    mockRuns();
    renderTab();

    // All four rows show once loaded.
    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());
    expect(screen.getByText('wf-completed-cron')).toBeInTheDocument();
    expect(screen.getByText('wf-failed-event')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();

    // Click "Running" status pill → only running-group rows
    // (running + awaiting_approval), the other two drop out.
    fireEvent.click(screen.getByRole('button', { name: 'Running' }));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
    expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument();
    expect(screen.queryByText('wf-failed-event')).not.toBeInTheDocument();

    // Add a Manual trigger filter → still both manual running-group rows.
    fireEvent.click(screen.getByRole('button', { name: 'Manual' }));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
    expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument();
    expect(screen.queryByText('wf-failed-event')).not.toBeInTheDocument();

    // Reset status back to "All" (status group's All pill) → still Manual
    // trigger filter narrows to the two manual rows of any status.
    fireEvent.click(screen.getAllByRole('button', { name: 'All' })[0]);
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
    expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument();
    expect(screen.queryByText('wf-failed-event')).not.toBeInTheDocument();

    // Reset trigger back to "All" too → all four rows back.
    fireEvent.click(screen.getAllByRole('button', { name: 'All' })[1]);
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-completed-cron')).toBeInTheDocument();
    expect(screen.getByText('wf-failed-event')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
  });
});

describe('ProjectRunsTab — Find', () => {
  it('typing narrows rows by workflow name, run id, or trigger', async () => {
    mockRuns();
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-completed-cron')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'cron' } });

    await waitFor(() => expect(screen.queryByText('wf-running-manual')).not.toBeInTheDocument());
    expect(screen.getByText('wf-completed-cron')).toBeInTheDocument();
  });

  it('footer shows "N matches of M loaded" while a query is active', async () => {
    mockRuns();
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-completed-cron')).toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'cron' } });

    await waitFor(() => expect(screen.getByText('1 matches of 4 loaded')).toBeInTheDocument());
  });

  it('Esc clears the query', async () => {
    mockRuns();
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-completed-cron')).toBeInTheDocument());

    const input = screen.getByPlaceholderText('Find runs…') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'cron' } });
    await waitFor(() => expect(screen.queryByText('wf-running-manual')).not.toBeInTheDocument());

    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(input.value).toBe(''));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
  });

  it('composes with the pills: searching narrows within the active pill filter', async () => {
    mockRuns();
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-completed-cron')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Running' }));
    await waitFor(() => expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument());

    fireEvent.change(screen.getByPlaceholderText('Find runs…'), { target: { value: 'cron' } });

    await waitFor(() => expect(screen.getByText('No matches')).toBeInTheDocument());
  });
});

describe('ProjectRunsTab — table rules (fit/subject columns)', () => {
  it('the workflow column is the one flexible/truncating subject column', async () => {
    mockRuns();
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());

    const subjectCell = screen.getByText('wf-running-manual').closest('td');
    expect(subjectCell?.className).toMatch(/max-w-0/);
    expect(subjectCell?.querySelector('[title="wf-running-manual"]')).toBeInTheDocument();
  });

  it('the Status column is a fit (nowrap) column', async () => {
    mockRuns();
    const { container } = renderTab();
    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());

    const statusHeader = Array.from(container.querySelectorAll('thead th')).find((th) =>
      th.textContent?.includes('Status'),
    );
    expect(statusHeader?.className).toMatch(/whitespace-nowrap/);
  });
});

describe('ProjectRunsTab — kit loading/empty/error states', () => {
  it('shows the kit Spinner before data resolves', async () => {
    let resolveFn: (v: RunListRow[]) => void = () => {};
    vi.spyOn(api, 'getProjectRuns').mockReturnValue(
      new Promise((resolve) => {
        resolveFn = resolve;
      }),
    );
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);

    renderTab();

    expect(screen.getByRole('status')).toBeInTheDocument();
    resolveFn([]);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('shows the kit EmptyState when there are no runs at all', async () => {
    mockRuns([]);
    renderTab();

    await waitFor(() => expect(screen.getByText('No runs for this project yet')).toBeInTheDocument());
  });

  it('shows the kit EmptyState "no match" copy when a pill filter narrows to zero', async () => {
    mockRuns([ROWS[1]]); // completed/cron only
    renderTab();
    await waitFor(() => expect(screen.getByText('wf-completed-cron')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Running' }));

    await waitFor(() => expect(screen.getByText('No runs match this filter')).toBeInTheDocument());
  });

  it('shows the kit ErrorBanner when the fetch fails', async () => {
    vi.spyOn(api, 'getProjectRuns').mockRejectedValue(new Error('network down'));
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);

    renderTab();

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('network down'));
  });
});
