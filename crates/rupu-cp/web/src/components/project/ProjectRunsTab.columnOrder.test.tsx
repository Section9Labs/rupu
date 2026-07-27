// @vitest-environment jsdom
// ProjectRunsTab — status-first column order: this table now mirrors the
// canonical run-table standard (status leads every top-level run table, per
// pages/runs/WorkflowRuns.tsx) after the final re-review moved Status from
// slot 4 to slot 1. Pure reorder — see ProjectRunsTab.test.tsx for
// behavioral coverage.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type RunListRow } from '../../lib/api';
import ProjectRunsTab from './ProjectRunsTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const ROW: RunListRow = {
  id: 'r-run-manual',
  workflow_name: 'wf-running-manual',
  status: 'running',
  started_at: '2026-06-01T00:00:00Z',
  trigger: 'manual',
  turns: 1,
  usage: {
    input_tokens: 100,
    output_tokens: 20,
    cached_tokens: 0,
    total_tokens: 120,
    cost_usd: 0.01,
    priced: true,
    runs: 1,
  },
};

describe('ProjectRunsTab — column order', () => {
  it('renders headers as Status, Workflow, Run, Trigger, In, Out, Cached, Cost, Turns, Duration, Started', async () => {
    vi.spyOn(api, 'getProjectRuns').mockResolvedValue([ROW]);
    // ProjectUsageTimeline (Task U4) fetches independently of the run list
    // above — mock it too so this test isn't tripped up by an unmocked call.
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);

    const { container } = render(
      <MemoryRouter>
        <ProjectRunsTab wsId="ws-1" />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('wf-running-manual')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual([
      'Status',
      'Workflow',
      'Run',
      'Trigger',
      'In',
      'Out',
      'Cached',
      'Cost',
      'Turns',
      'Duration',
      'Started',
    ]);
  });
});
