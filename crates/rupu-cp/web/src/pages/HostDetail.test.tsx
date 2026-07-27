// @vitest-environment jsdom
// HostDetail — Task 4 (table-standardization plan): the host's run rows adopt
// whole-row navigation (rowHref) to /runs/:id, replacing the dedicated Run-id
// link column.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, type HostView, type RunListRow } from '../lib/api';
import HostDetail from './HostDetail';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const HOST: HostView = {
  id: 'host-abc123',
  name: 'prod-east',
  transport_kind: 'http_cp',
  base_url: 'https://rupu.prod-east.example.com',
  status: 'online',
  version: '0.9.0',
  active_run_count: 1,
  last_seen_at: new Date().toISOString(),
};

const RUN: RunListRow = {
  id: 'run-1',
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
  host_id: 'host-abc123',
};

function renderPage(id = 'host-abc123') {
  return render(
    <MemoryRouter initialEntries={[`/hosts/${id}`]}>
      <Routes>
        <Route path="/hosts/:id" element={<HostDetail />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('HostDetail — run rows adopt whole-row navigation (rowHref)', () => {
  it('renders each run row as a link to /runs/:id', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([HOST]);
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([RUN]);

    renderPage();

    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());
    const link = screen.getByText('deploy-prod').closest('a');
    expect(link).toHaveAttribute('href', '/runs/run-1?host=host-abc123');
  });

  it('the Run id cell rides the shared row link rather than a separately-styled link of its own', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([HOST]);
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([RUN]);

    renderPage();

    const idCell = await screen.findByText(/run-1/);
    // Every cell in a rowHref row is individually link-wrapped by
    // SortableTable, so `closest('a')` always resolves to AN anchor — the
    // meaningful assertion is that it's the SAME destination as the row
    // (not a bespoke second link to somewhere else), and no longer carries
    // the old link-only styling (brand color + underline-on-hover).
    const anchor = idCell.closest('a');
    expect(anchor).toHaveAttribute('href', '/runs/run-1?host=host-abc123');
    expect(idCell.className).not.toMatch(/brand-600/);
    expect(idCell.className).not.toMatch(/hover:underline/);
  });

  // Final re-review: HostDetail's run table now leads with Status, matching
  // the canonical status-first order every other top-level run table
  // follows (pages/runs/WorkflowRuns.tsx is the reference implementation).
  it('renders run-table headers as Status, Workflow, Run, Started', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([HOST]);
    vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([RUN]);

    const { container } = renderPage();

    await waitFor(() => expect(screen.getByText('deploy-prod')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual(['Status', 'Workflow', 'Run', 'Started']);
  });
});
