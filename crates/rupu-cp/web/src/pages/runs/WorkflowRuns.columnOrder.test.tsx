// @vitest-environment jsdom
// WorkflowRuns — canonical run-table column order (table-standardization
// follow-up, I1): Status leads, then Subject (Workflow) → ID (Run) →
// Source (Trigger) → Host → usage → Turns → Duration → Started → (actions).
// Pure reorder — see WorkflowRuns.test.tsx for behavioral coverage.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
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

const ROW: RunListRow = {
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
};

describe('WorkflowRuns — canonical column order', () => {
  it('renders headers as Status, Workflow, Run, Trigger, Host, In, Out, Cached, Cost, Turns, Duration, Started, (actions)', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([ROW]);

    const { container } = render(
      <MemoryRouter>
        <WorkflowRuns />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual([
      'Status',
      'Workflow',
      'Run',
      'Trigger',
      'Host',
      'In',
      'Out',
      'Cached',
      'Cost',
      'Turns',
      'Duration',
      'Started',
      '',
    ]);
  });
});
