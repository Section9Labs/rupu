// @vitest-environment jsdom
// RunDetail — archive/delete buttons. Mocks the same heavy children as the
// sibling RunDetail.test.tsx so we only drive the header action cluster.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Routes, Route } from 'react-router-dom';
import { api, type RunGraphResponse, type FindingsResponse } from '../lib/api';

// ---- Mocks for heavy children (mirror RunDetail.test.tsx) ------------------

vi.mock('../components/RunGraph', () => ({
  __esModule: true,
  default: () => <div data-testid="run-graph-mock" />,
}));

vi.mock('../components/TranscriptPanel', () => ({
  __esModule: true,
  default: () => <div data-testid="transcript-panel" />,
}));

vi.mock('../components/run/StepTranscriptBrowser', () => ({
  __esModule: true,
  default: () => <div data-testid="step-transcript-browser" />,
}));

vi.mock('../components/RunEventFeed', () => ({
  __esModule: true,
  default: () => <div data-testid="event-feed" />,
}));

vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-timeline-mock" />,
}));

// Imported after vi.mock declarations so hoisted factories resolve first.
import RunDetail from './RunDetail';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---- Fixtures (real shapes from RunDetail.test.tsx) -----------------------

// A completed (terminal, non-running) run — Archive + Delete buttons render.
const COMPLETED_GRAPH: RunGraphResponse = {
  run: {
    id: 'run_01X',
    workflow_name: 'nightly-scan',
    status: 'completed',
    started_at: '2026-06-01T00:00:00Z',
    finished_at: '2026-06-01T00:05:00Z',
  } as RunGraphResponse['run'],
  workflow: {
    steps: [{ id: 'step_a', kind: 'step', agent: 'reviewer' }],
  },
  step_results: [
    { step_id: 'step_a', success: true, transcript_path: '/t/step-a.jsonl' } as RunGraphResponse['step_results'][number],
  ],
  units: [],
};

const FINDINGS: FindingsResponse = {
  findings: [],
  summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
};

// ---- Render helpers --------------------------------------------------------

function renderAt(id: string) {
  return render(
    <MemoryRouter initialEntries={[`/runs/${id}`]}>
      <Routes>
        <Route path="/runs/:id" element={<RunDetail />} />
        {/* Destination after navigate('/runs') */}
        <Route path="/runs" element={<div data-testid="runs-list" />} />
      </Routes>
    </MemoryRouter>,
  );
}

/** Stub the read-only dependencies that RunDetail always calls on mount. */
function stubBase() {
  vi.spyOn(api, 'getRunGraph').mockResolvedValue(COMPLETED_GRAPH);
  vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue(FINDINGS);
  vi.spyOn(api, 'subscribeRunLog').mockImplementation(() => () => {});
}

// ---- Tests -----------------------------------------------------------------

describe('RunDetail archive/delete', () => {
  it('archives without confirm and navigates to /runs', async () => {
    stubBase();
    const archive = vi.spyOn(api, 'archiveRun').mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, 'confirm');

    renderAt('run_01X');

    // Wait for the graph to load (run is terminal, so Archive button appears).
    const archiveBtn = await screen.findByRole('button', { name: /archive/i });
    fireEvent.click(archiveBtn);

    await waitFor(() => expect(archive).toHaveBeenCalledWith('run_01X'));
    // Archive must NOT gate behind window.confirm.
    expect(confirmSpy).not.toHaveBeenCalled();
    // Navigation to /runs after success.
    await waitFor(() => expect(screen.getByTestId('runs-list')).toBeInTheDocument());
  });

  it('deletes after confirm and navigates to /runs', async () => {
    stubBase();
    const del = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(true);

    renderAt('run_01X');

    const deleteBtn = await screen.findByRole('button', { name: /delete/i });
    fireEvent.click(deleteBtn);

    expect(window.confirm).toHaveBeenCalled();
    await waitFor(() => expect(del).toHaveBeenCalledWith('run_01X'));
    await waitFor(() => expect(screen.getByTestId('runs-list')).toBeInTheDocument());
  });

  it('does not delete when confirm is cancelled', async () => {
    stubBase();
    const del = vi.spyOn(api, 'deleteRun').mockResolvedValue(undefined);
    vi.spyOn(window, 'confirm').mockReturnValue(false);

    renderAt('run_01X');

    const deleteBtn = await screen.findByRole('button', { name: /delete/i });
    fireEvent.click(deleteBtn);

    expect(window.confirm).toHaveBeenCalled();
    // After a tick, deleteRun should NOT have been called.
    await new Promise((r) => setTimeout(r, 50));
    expect(del).not.toHaveBeenCalled();
    // Still on the run detail page (no navigation).
    expect(screen.queryAllByTestId('runs-list')).toHaveLength(0);
  });
});
