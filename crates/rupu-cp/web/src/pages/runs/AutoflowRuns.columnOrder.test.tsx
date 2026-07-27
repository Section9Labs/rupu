// @vitest-environment jsdom
// AutoflowRuns (Runs tab) — canonical run-table column order
// (table-standardization follow-up, I1 + I4): Status leads, then Subject
// (Workflow) → ID (Run) → the Source region (Event, Issue Ref, Worker — kept
// contiguous immediately after ID, per-page columns) → Host → usage → the
// newly-wired Turns/Duration (I4) → Started. Pure reorder plus the new
// columns — see AutoflowRuns.test.tsx for behavioral coverage.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AutoflowEventRow, HostView } from '../../lib/api';
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

const EVENT: AutoflowEventRow = {
  event_id: 'evt-1',
  cycle_id: 'cyc-1',
  at: '2026-06-01T00:00:00Z',
  kind: 'run_launched',
  workflow: 'fix-issue',
  run_id: 'run-9',
  status: 'completed',
  worker_name: 'worker-1',
  turns: 4,
  duration_ms: 120_000,
  usage: {
    input_tokens: 100,
    output_tokens: 50,
    cached_tokens: 0,
    total_tokens: 150,
    cost_usd: null,
    priced: false,
    runs: 1,
  },
  host_id: 'local',
};

describe('AutoflowRuns — canonical column order + Turns/Duration (I4)', () => {
  it('renders headers as Status, Workflow, Run, Event, Issue Ref, Worker, Host, In, Out, Cached, Cost, Turns, Duration, Started', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([EVENT]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    const { container } = render(
      <MemoryRouter>
        <AutoflowRuns />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual([
      // Leading expand/collapse chevron column (`renderDetail` is wired for
      // `cycle_failed`'s detail row) — reserved on every row, header is
      // unlabeled.
      '',
      'Status',
      'Workflow',
      'Run',
      'Event',
      'Issue Ref',
      'Worker',
      'Host',
      'In',
      'Out',
      'Cached',
      'Cost',
      'Turns',
      'Duration',
      'Started',
      '', // trailing row-actions column (unlabeled) — Task 3
    ]);
  });

  it('renders Turns and Duration for a run-bearing event, blank (not zero) when absent', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAutoflowEvents').mockResolvedValue([
      EVENT,
      { ...EVENT, event_id: 'evt-2', run_id: 'run-10', turns: null, duration_ms: null },
    ]);
    vi.spyOn(api, 'getAutoflowRuns').mockResolvedValue([]);

    render(
      <MemoryRouter>
        <AutoflowRuns />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getAllByText('fix-issue').length).toBe(2));

    expect(screen.getByText('4')).toBeInTheDocument();
    expect(screen.getByText('2m 0s')).toBeInTheDocument();
    // Absent turns/duration render an em-dash, never a fabricated "0".
    expect(screen.queryAllByText('0').length).toBe(0);
  });
});
