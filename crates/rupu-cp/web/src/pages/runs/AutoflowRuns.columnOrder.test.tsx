// @vitest-environment jsdom
// AutoflowRuns — the Workflows column order (spec 2026-10-14, CP autoflow
// runs as one table): `WORKFLOW_RUN_COLUMNS` with Issue after Workflow, a
// leading expand column (an attempt with no run to open expands in place)
// and the trailing row-actions column. An attempt row never ran, so its
// usage, Turns and Duration are dashes, never a fabricated zero.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AutoflowRunRow, HostView } from '../../lib/api';
import AutoflowRuns from './AutoflowRuns';
import { withCustomerScope } from '../../lib/customerScopeTestUtils';

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

const RUN: AutoflowRunRow = {
  id: 'run-9',
  workflow_name: 'fix-issue',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  finished_at: '2026-06-01T00:02:00Z',
  trigger: 'event',
  turns: 4,
  duration_ms: 120_000,
  codename: 'cobalt-harbor/heron',
  codename_derived: false,
  host_id: 'local',
  usage: { input_tokens: 100, output_tokens: 50, cached_tokens: 0, total_tokens: 150, cost_usd: null, priced: false, runs: 1 },
  autoflow: { issue_ref: 'github:acme/widgets/issues/42', issue_display_ref: 'acme/widgets#42' },
};

/** The server's attempt row: zero usage, `finished_at` = `started_at`. */
const ATTEMPT: AutoflowRunRow = {
  ...RUN,
  id: 'afe_1',
  status: 'failed',
  started_at: '2026-06-01T00:10:00Z',
  finished_at: '2026-06-01T00:10:00Z',
  turns: 0,
  duration_ms: null,
  usage: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 },
  attempt: { detail: 'dispatch failed', cycle_id: 'cyc_1', worker: 'local', claim_status: 'retry_backoff' },
};

function stubPage(rows: AutoflowRunRow[]) {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
  vi.spyOn(api, 'getAutoflowRunList').mockResolvedValue(rows);
  vi.spyOn(api, 'getAutoflowProblems').mockResolvedValue([]);
}

function renderPage() {
  return render(
    <MemoryRouter>
      {withCustomerScope(<AutoflowRuns />, { customers: [] })}
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — the Workflows column order', () => {
  it('renders the Workflows headers: Status, Workflow, Run, Trigger, Host, In, Out, Cached, Cost, Turns, Duration, Started', async () => {
    stubPage([RUN]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getByText('fix-issue')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map((th) => th.textContent?.trim() ?? '');
    expect(headers).toEqual([
      '', // leading expand column (an attempt with no run expands in place)
      'Status',
      'Workflow', // its second line carries the issue (or an attempt's error)
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
      '', // trailing column: show-only filter, PR link, row actions
    ]);
  });

  it('renders Turns and Duration for a run; dashes, not zeros, on an attempt', async () => {
    stubPage([RUN, ATTEMPT]);
    const { container } = renderPage();
    await waitFor(() => expect(screen.getAllByText('fix-issue').length).toBe(2));

    // Scope the row assertions to the table — the usage summary strip above it
    // legitimately prints a run's zero token total (e.g. "Cached 0").
    const table = within(container.querySelector('table')!);
    expect(table.getByText('4')).toBeInTheDocument();
    expect(table.getByText('2m 0s')).toBeInTheDocument();
    // The attempt's zero-filled usage and its equal start/finish stamps never
    // show as "0" tokens or a "0ms" duration in the table.
    expect(table.queryAllByText('0').length).toBe(0);
    expect(table.queryByText('0ms')).not.toBeInTheDocument();
  });
});
