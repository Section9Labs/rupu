// @vitest-environment jsdom
// RunDetail — Retry on a terminal run (operator MCP Plan 3b, UI-1, mock §1).
// A failed / rejected / cancelled run gets a Retry popover (mode defaulting to
// the run's recorded one, "Restart interrupted steps"); after the 202 a
// `retry requested` chip stands in for the button until the run record says
// the retry took it. Mocks the same heavy children as RunDetail.archive.test.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import { api, ApiError, type RunGraphResponse, type FindingsResponse, type RunRecord } from '../lib/api';
import { runsApi } from '../lib/api3b/runs';

vi.mock('../components/RunGraph', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/TranscriptPanel', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/run/StepTranscriptBrowser', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/RunEventFeed', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/charts/RunUsageTimeline', () => ({ __esModule: true, default: () => <div /> }));

import RunDetail from './RunDetail';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function graphFor(run: Partial<RunRecord>): RunGraphResponse {
  return {
    run: {
      id: 'run_01X',
      workflow_name: 'security-review',
      status: 'failed',
      started_at: '2026-10-20T14:02:00Z',
      finished_at: '2026-10-20T14:08:00Z',
      ...run,
    } as RunGraphResponse['run'],
    workflow: { steps: [{ id: 'triage', kind: 'step', agent: 'reviewer' }] },
    step_results: [],
    units: [],
  };
}

const FINDINGS: FindingsResponse = {
  facets: {},
  tags_unavailable: [],
  findings: [],
  summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
};

function stubBase(graph: RunGraphResponse) {
  vi.spyOn(api, 'getRunGraph').mockResolvedValue(graph);
  vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
  vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
}

function renderAt(search = '') {
  return render(
    <MemoryRouter initialEntries={[`/runs/run_01X${search}`]}>
      <Routes>
        <Route path="/runs/:id" element={<RunDetail />} />
      </Routes>
    </MemoryRouter>,
  );
}

async function openRetry() {
  fireEvent.click(await screen.findByRole('button', { name: /^retry/i }));
  return screen.getByRole('dialog', { name: 'Retry security-review' });
}

describe('RunDetail retry', () => {
  it.each(['failed', 'rejected', 'cancelled'] as const)('offers Retry on a %s run, before Archive', async (status) => {
    stubBase(graphFor({ status }));
    renderAt();
    const retry = await screen.findByRole('button', { name: /^retry/i });
    const archive = screen.getByRole('button', { name: /archive/i });
    // Retry comes first in the action cluster.
    expect(retry.compareDocumentPosition(archive) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('offers no Retry on a completed run', async () => {
    stubBase(graphFor({ status: 'completed' }));
    renderAt();
    await screen.findByRole('button', { name: /archive/i });
    expect(screen.queryByRole('button', { name: /^retry/i })).not.toBeInTheDocument();
  });

  it('defaults the mode to the recorded one and retries without overriding it', async () => {
    stubBase(graphFor({ permission_mode: 'readonly' }));
    const retry = vi
      .spyOn(runsApi, 'retryRun')
      .mockResolvedValue({ run_id: 'run_01X', retrying: true, restart_interrupted: false, host_id: 'local' });
    vi.spyOn(api, 'getRun').mockResolvedValue({
      run: graphFor({ permission_mode: 'readonly', resume_requested_at: '2026-10-20T15:00:00Z' }).run,
      steps: [],
      usage: undefined as never,
    });
    renderAt();
    const pop = await openRetry();
    const mode = within(pop).getByLabelText('Mode') as HTMLSelectElement;
    expect(mode.selectedOptions[0].textContent).toBe('Read-only (as launched)');
    expect(within(pop).getByText('Off: steps cut off mid-turn continue where they stopped.')).toBeInTheDocument();

    fireEvent.click(within(pop).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(retry).toHaveBeenCalledWith('run_01X', { mode: undefined, restartInterrupted: false }));
    // Pending, not optimistic: the chip replaces the button, the run still reads failed.
    expect(await screen.findByText('retry requested')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^retry/i })).not.toBeInTheDocument();
  });

  it('sends a changed mode and the restart flag', async () => {
    stubBase(graphFor({ permission_mode: 'readonly' }));
    const retry = vi
      .spyOn(runsApi, 'retryRun')
      .mockResolvedValue({ run_id: 'run_01X', retrying: true, restart_interrupted: true, host_id: 'local' });
    vi.spyOn(api, 'getRun').mockResolvedValue({ run: graphFor({}).run, steps: [], usage: undefined as never });
    renderAt();
    const pop = await openRetry();
    fireEvent.change(within(pop).getByLabelText('Mode'), { target: { value: 'bypass' } });
    fireEvent.click(within(pop).getByLabelText('Restart interrupted steps from scratch'));
    fireEvent.click(within(pop).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(retry).toHaveBeenCalledWith('run_01X', { mode: 'bypass', restartInterrupted: true }));
  });

  it('turns live once the run record says the retry took it', async () => {
    stubBase(graphFor({}));
    vi.spyOn(runsApi, 'retryRun').mockResolvedValue({
      run_id: 'run_01X',
      retrying: true,
      restart_interrupted: false,
      host_id: 'local',
    });
    vi.spyOn(api, 'getRun').mockResolvedValue({
      run: graphFor({ status: 'running', finished_at: null }).run,
      steps: [],
      usage: undefined as never,
    });
    renderAt();
    fireEvent.click(within(await openRetry()).getByRole('button', { name: 'Retry' }));
    await screen.findByText('retry requested');
    // The poll sees `running`: the chip goes and the live header (Cancel) shows.
    expect(await screen.findByRole('button', { name: 'Cancel run' }, { timeout: 4000 })).toBeInTheDocument();
    expect(screen.queryByText('retry requested')).not.toBeInTheDocument();
  });

  it('shows a 409 inline under the actions and keeps Retry', async () => {
    stubBase(graphFor({}));
    vi.spyOn(runsApi, 'retryRun').mockRejectedValue(
      new ApiError(409, 'conflict', JSON.stringify({ error: 'run run_01X is `running`: only a failed, rejected or cancelled run can be retried' })),
    );
    renderAt();
    fireEvent.click(within(await openRetry()).getByRole('button', { name: 'Retry' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'run run_01X is `running`: only a failed, rejected or cancelled run can be retried',
    );
    expect(screen.getByRole('button', { name: /^retry/i })).toBeInTheDocument();
  });

  it('disables Retry on a remote run, with the reason', async () => {
    stubBase(graphFor({}));
    renderAt('?host=h1');
    const btn = await screen.findByRole('button', { name: /^retry/i });
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute('title', expect.stringContaining('local-only'));
  });
});
