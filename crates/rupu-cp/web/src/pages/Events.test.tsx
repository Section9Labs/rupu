// @vitest-environment jsdom
// Events page — the global Live Events feed now loads HISTORY on mount (so
// an idle page is never empty) in addition to the live SSE firehose.
//
// Covers: getEvents is called on mount and historical rows render; a live
// SSE event prepends on top; "Load older events" pages backward via
// before_ts; a history-fetch error surfaces as a banner without blocking
// live events.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';
import type { RunEvent, RunStartedEvent } from '../lib/api';
import Events from './Events';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// `pos` mirrors the field the real backend stamps onto every history row
// (see `EventsCursor` / `recent_events` in crates/rupu-cp/src/api/events.rs)
// — the 0-based line index within that event's own run's `events.jsonl`,
// used together with `ts` and `run_id` to build the "load older" cursor.
function runStarted(
  runId: string,
  ts: number,
  pos = 0,
): RunStartedEvent & { ts: number; pos: number } {
  return {
    type: 'run_started',
    run_id: runId,
    event_version: 1,
    workflow_path: 'wf.yaml',
    started_at: new Date(ts).toISOString(),
    ts,
    pos,
  };
}

// The page treats a full page (length >= its internal PAGE_SIZE, 200) from
// `getEvents` as a signal that more history may exist. Build a first page of
// exactly that size, newest-first (descending ts, matching the real
// backend's ordering), so `hasMoreOlder` stays true and the "Load older
// events" control renders.
const FULL_PAGE_SIZE = 200;
function fullHistoryPage(): (RunStartedEvent & { ts: number; pos: number })[] {
  return Array.from({ length: FULL_PAGE_SIZE }, (_, i) =>
    runStarted(`run_hist_${FULL_PAGE_SIZE - i}`, (FULL_PAGE_SIZE - i) * 1000, i),
  );
}

function renderPage() {
  return render(
    <MemoryRouter>
      <Events />
    </MemoryRouter>,
  );
}

describe('Events page', () => {
  it('loads history on mount and renders it — an idle page is not empty', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([runStarted('run_hist_1', 1_000)]);
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();

    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());
    expect(await screen.findByText('Run started')).toBeInTheDocument();
    expect(screen.queryByText(/No events yet/)).not.toBeInTheDocument();
  });

  it('a live SSE event prepends on top of loaded history', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([runStarted('run_hist_1', 1_000)]);
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => {
      emit = onEvent;
      return () => {};
    });

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());
    await screen.findByText('Run started');

    // Only one "Run started" row so far (the history one).
    expect(screen.getAllByText('Run started')).toHaveLength(1);

    expect(emit).toBeDefined();
    emit?.({ type: 'run_completed', run_id: 'run_live_1', status: 'completed', finished_at: new Date().toISOString() });

    expect(await screen.findByText('Run completed')).toBeInTheDocument();
    // History row is still present — the live event was prepended, not a replacement.
    expect(screen.getAllByText('Run started')).toHaveLength(1);
  });

  it('an SSE replay of an already-loaded history event renders once, not twice', async () => {
    // Reproduces the firehose's initial-drain: `FileTailRunSource::open`
    // replays a currently-active run's entire `events.jsonl` from offset 0
    // before tailing new appends (see `tail_all_events_sse`), so an active
    // run's already-written event arrives BOTH via the history fetch AND as
    // one of the first SSE frames — at two different `ts` (history uses
    // file-mtime/own-ts, SSE stamps client arrival time), so a merge keyed
    // on `ts` (or a fresh `seq`) can't recognize them as the same
    // occurrence and renders it twice.
    const historyEvent = runStarted('run_active', 5_000, 7);
    vi.spyOn(api, 'getEvents').mockResolvedValue([historyEvent]);
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => {
      emit = onEvent;
      return () => {};
    });

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());
    await screen.findByText('Run started');
    expect(screen.getAllByText('Run started')).toHaveLength(1);
    expect(screen.getByText(/^1 event$/)).toBeInTheDocument();

    // The SAME occurrence, replayed via SSE: identical content, but no
    // `ts`/`pos` (a raw SSE frame never carries either — see `identityOf`
    // in Events.tsx).
    expect(emit).toBeDefined();
    emit?.({
      type: 'run_started',
      run_id: historyEvent.run_id,
      event_version: historyEvent.event_version,
      workflow_path: historyEvent.workflow_path,
      started_at: historyEvent.started_at,
    });

    // Wait for a render this SSE frame definitely triggers (the connection
    // badge), then assert the duplicate did NOT add a second row.
    await waitFor(() => expect(screen.getByText('Live')).toBeInTheDocument());
    expect(screen.getAllByText('Run started')).toHaveLength(1);
    expect(screen.getByText(/^1 event$/)).toBeInTheDocument();
  });

  it('shows the connection badge as live once an SSE frame arrives', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([]);
    let emit: ((e: RunEvent) => void) | undefined;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => {
      emit = onEvent;
      return () => {};
    });

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());

    emit?.({ type: 'run_started', run_id: 'r1', event_version: 1, workflow_path: 'wf.yaml', started_at: 'x' });
    expect(await screen.findByText('Live')).toBeInTheDocument();
  });

  it('lazy-loads older history using the (ts, run_id, pos) cursor of the oldest loaded row', async () => {
    // jsdom reports zero layout geometry, so EventTimeline's bottom sentinel
    // (useInfiniteScroll) fires its auto-load-more check as soon as it's in
    // the DOM — the same underlying handler the visible "Load older events"
    // button calls. Assert on the request this produces (right cursor,
    // older row rendered) rather than the trigger mechanism.
    const firstPage = fullHistoryPage();
    const oldest = firstPage[firstPage.length - 1];
    vi.spyOn(api, 'getEvents')
      .mockResolvedValueOnce(firstPage) // full page → hasMoreOlder stays true
      .mockResolvedValue([runStarted('run_older', oldest.ts - 1000)]); // older page(s)
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalledTimes(1));

    await waitFor(() => expect(screen.getByTitle('Open run run_older')).toBeInTheDocument());
    // The oldest row's own ts/run_id/pos — not a fresh monotonic seq — must
    // drive the next page's cursor (see `EventsCursor::Compound` in
    // crates/rupu-cp/src/api/events.rs): this is what lets "load older"
    // resume past a page boundary that lands mid-run at a shared fallback
    // `ts` instead of permanently skipping the rest of that run's events.
    expect(api.getEvents).toHaveBeenCalledWith(expect.any(Number), oldest.ts, oldest.run_id, oldest.pos);
  });

  it('a short first page (less than the page size) means no more older history', async () => {
    vi.spyOn(api, 'getEvents').mockResolvedValue([runStarted('run_only', 1_000)]);
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();
    await waitFor(() => expect(api.getEvents).toHaveBeenCalled());
    expect(await screen.findByText(/end of history/)).toBeInTheDocument();
  });

  it('a history-load error surfaces as a banner', async () => {
    vi.spyOn(api, 'getEvents').mockRejectedValue(new Error('boom'));
    vi.spyOn(api, 'subscribeEvents').mockImplementation(() => () => {});

    renderPage();

    expect(await screen.findByText(/Could not load event history/)).toBeInTheDocument();
  });
});
