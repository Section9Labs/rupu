// @vitest-environment jsdom
// ProjectRunsTab — Turns-before-Duration fix (I6): this was the only
// run-like table left disagreeing with the canonical run-table standard
// after table-standardization Task 3 put Turns before Duration everywhere
// else. Pure reorder — see ProjectRunsTab.test.tsx for behavioral coverage.

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
  it('renders headers as Workflow, Run, Trigger, Status, In, Out, Cached, Cost, Turns, Duration, Started', async () => {
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
      'Workflow',
      'Run',
      'Trigger',
      'Status',
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
