// @vitest-environment jsdom
// ProjectRunsTab — client-side status + trigger filter chips applied to the
// loaded rows. api.getProjectRuns is spied to return a mixed-status/-trigger
// page; chip clicks narrow the rendered list and re-clicking the active status
// chip returns to "All".

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

function mockRuns(): void {
  vi.spyOn(api, 'getProjectRuns').mockImplementation((_wsId, params) => {
    // Page 0 returns all rows; any subsequent offset returns empty.
    if (params?.offset && params.offset > 0) return Promise.resolve([]);
    return Promise.resolve(ROWS);
  });
  // ProjectUsageTimeline (Task U4) fetches independently of the run list
  // above — mock it too so this test isn't tripped up by an unmocked call.
  vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
}

describe('ProjectRunsTab', () => {
  it('renders all rows then narrows via status and trigger chips', async () => {
    mockRuns();
    render(
      <MemoryRouter>
        <ProjectRunsTab wsId="x" />
      </MemoryRouter>,
    );

    // All four rows show once loaded.
    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());
    expect(screen.getByText('wf-completed-cron')).toBeInTheDocument();
    expect(screen.getByText('wf-failed-event')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();

    // Click "Running" status chip → only running-group rows
    // (running + awaiting_approval), the other two drop out.
    fireEvent.click(screen.getByRole('button', { name: 'Running' }));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
    expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument();
    expect(screen.queryByText('wf-failed-event')).not.toBeInTheDocument();

    // Add a Manual trigger filter → narrows to the manual running row only.
    fireEvent.click(screen.getByRole('button', { name: 'Manual' }));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
    // Both running-group rows are manual, so both remain; confirm cron/event gone.
    expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument();
    expect(screen.queryByText('wf-failed-event')).not.toBeInTheDocument();

    // Click the active "Running" status chip again → back to all-status
    // (still trigger=Manual, so the two manual rows of any status show).
    fireEvent.click(screen.getByRole('button', { name: 'Running' }));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
    expect(screen.queryByText('wf-completed-cron')).not.toBeInTheDocument();
    expect(screen.queryByText('wf-failed-event')).not.toBeInTheDocument();

    // Clear the trigger filter too → all four rows back.
    fireEvent.click(screen.getByRole('button', { name: 'Manual' }));
    expect(screen.getByText('wf-running-manual')).toBeInTheDocument();
    expect(screen.getByText('wf-completed-cron')).toBeInTheDocument();
    expect(screen.getByText('wf-failed-event')).toBeInTheDocument();
    expect(screen.getByText('wf-awaiting-manual')).toBeInTheDocument();
  });
});
