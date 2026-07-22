// @vitest-environment jsdom
// Live Events (Situation Room) page — the global wall that merges the SSE +
// history event firehose with the REST findings list, plus a project roster
// and fleet vitals.
//
// These guard the data-owner behaviors that survived the redesign: history
// loads on mount, a live SSE frame prepends, an SSE replay of an already-
// loaded history event is deduped, the connection badge flips to Live, and
// "Load older events" pages backward via the (ts, run_id, pos) cursor. The
// aggregate calls (findings / projects / dashboard / run) are mocked to empty
// so the test exercises the event pipeline in isolation.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import type { RunEvent, RunStartedEvent } from '../lib/api';
import Events from './Events';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// Default the aggregate endpoints to empty so only the event pipeline is under
// test (the page swallows their rejections, but stubbing avoids real fetches).
beforeEach(() => {
  vi.spyOn(api, 'getFindings').mockResolvedValue({
    findings: [],
    summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
  });
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
  vi.spyOn(api, 'getDashboard').mockRejectedValue(new Error('no dashboard in test'));
  vi.spyOn(api, 'getRun').mockRejectedValue(new Error('no run detail in test'));
});

function runStarted(runId: string, ts: number, pos = 0): RunStartedEvent & { ts: number; pos: number } {
  return {
    type: 'run_started', run_id: runId, event_version: 1, workflow_path: 'wf.yaml',
    started_at: new Date(ts).toISOString(), ts, pos,
  };
}

const FULL_PAGE_SIZE = 200;
function fullHistoryPage(): (RunStartedEvent & { ts: number; pos: number })[] {
  return Array.from({ length: FULL_PAGE_SIZE }, (_, i) =>
    runStarted(`run_hist_${FULL_PAGE_SIZE - i}`, (FULL_PAGE_SIZE - i) * 1000, i),
  );
}

function renderPage() {
  return render(<MemoryRouter><Events /></MemoryRouter>);
}

describe('Live Events (Situation Room) page', () => {
  it('loads history on mount and renders it — an idle page is not empty', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([runStarted('run_hist_1', 1_000)]);
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();

    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());
    expect(await screen.findByText('Workflow run started')).toBeInTheDocument();
    expect(screen.queryByText(/Waiting for events/)).not.toBeInTheDocument();
  });

  it('a live SSE event prepends on top of loaded history', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([runStarted('run_hist_1', 1_000)]);
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => { emit = onEvent; return () => {}; });

    renderPage();
    await screen.findByText('Workflow run started');

    emit?.({ type: 'run_failed', run_id: 'run_live_1', error: 'boom-live-xyz', finished_at: new Date().toISOString() });

    expect(await screen.findByText('boom-live-xyz')).toBeInTheDocument();
    // The history row is still present — the live event prepended, not replaced.
    expect(screen.getByText('Workflow run started')).toBeInTheDocument();
  });

  it('an SSE replay of an already-loaded history event renders once, not twice', async () => {
    const historyEvent = runStarted('run_active', 5_000, 7);
    vi.spyOn(api, 'getEvents').mockResolvedValue([historyEvent]);
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => { emit = onEvent; return () => {}; });

    renderPage();
    await screen.findByText('Workflow run started');
    expect(screen.getByText('1 events')).toBeInTheDocument();

    // Same occurrence, replayed via SSE with no ts/pos — identity dedup must
    // recognize it (see identityOf in Events.tsx) and NOT add a second row.
    emit?.({
      type: 'run_started', run_id: historyEvent.run_id, event_version: historyEvent.event_version,
      workflow_path: historyEvent.workflow_path, started_at: historyEvent.started_at,
    });

    await waitFor(() => expect(screen.getByText('Live')).toBeInTheDocument());
    expect(screen.getAllByText('Workflow run started')).toHaveLength(1);
    expect(screen.getByText('1 events')).toBeInTheDocument();
  });

  it('flips the connection badge to Live once an SSE frame arrives', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([]);
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => { emit = onEvent; return () => {}; });

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());

    emit?.({ type: 'run_started', run_id: 'r1', event_version: 1, workflow_path: 'wf.yaml', started_at: 'x' });
    expect(await screen.findByText('Live')).toBeInTheDocument();
  });

  it('"Load older events" pages backward using the oldest row\'s (ts, run_id, pos) cursor', async () => {
    const firstPage = fullHistoryPage();
    const oldest = firstPage[firstPage.length - 1];
    vi.spyOn(api, 'getEvents')
      .mockResolvedValueOnce(firstPage) // full page → more history may exist
      .mockResolvedValue([runStarted('run_older', oldest.ts - 1000)]);
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalledTimes(1));

    const btn = await screen.findByText('Load older events');
    fireEvent.click(btn);

    // The oldest loaded row's own ts/run_id/pos — not a fresh seq — drives the
    // cursor (EventsCursor::Compound), so paging resumes past a boundary that
    // lands mid-run at a shared fallback ts.
    await waitFor(() =>
      expect(api.getEvents).toHaveBeenCalledWith(expect.any(Number), oldest.ts, oldest.run_id, oldest.pos),
    );
  });

  it('a short first page means no more older history', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([runStarted('run_only', 1_000)]);
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();
    expect(await screen.findByText(/Beginning of history/i)).toBeInTheDocument();
  });

  it('a history-load failure does not block live events from arriving', async () => {
    vi.spyOn(api, 'getEvents').mockRejectedValue(new Error('boom'));
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => { emit = onEvent; return () => {}; });

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());

    emit?.({ type: 'run_failed', run_id: 'r1', error: 'after-error-xyz', finished_at: new Date().toISOString() });
    expect(await screen.findByText('after-error-xyz')).toBeInTheDocument();
  });
});
