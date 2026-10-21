// @vitest-environment jsdom
// RunDetail's message and board layer (spec 2026-10-21 goal 6): a run with a
// message space gets the time axis above its graph, the graph an `overlay`
// (its messages on the step cards, its board, the replay's reveal) and the
// Messages and Board panels below it. A run without a space renders exactly
// today's graph. RunGraph is mocked (xyflow can't mount under jsdom) to
// expose the overlay it is handed.
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError, type MessagesView } from '../lib/api';
import type { RunGraphOverlay } from '../components/RunGraph';
import {
  RUN_BOARD,
  RUN_EVENTS,
  RUN_GRAPH_RESPONSE,
  RUN_ID,
  RUN_MESSAGES,
  RUN_PARTICIPANTS,
} from '../lib/flow/runFixtures';

const seen = vi.hoisted(() => ({
  overlay: undefined as RunGraphOverlay | undefined,
  renders: 0,
  /** Whether the graph's first render already had its layer. */
  first: null as boolean | null,
  /** Renders of the usage chart: the page around the graph. */
  pageRenders: 0,
}));

vi.mock('../components/RunGraph', () => ({
  __esModule: true,
  default: (props: { overlay?: RunGraphOverlay }) => {
    seen.overlay = props.overlay;
    seen.renders += 1;
    if (seen.first === null) seen.first = !!props.overlay;
    return (
      <div data-testid="run-graph-mock" data-overlay={props.overlay ? 'on' : 'none'}>
        {props.overlay && (
          <>
            <span data-testid="overlay-arcs">
              {props.overlay.messages.map((m) => `${m.id}:${m.from}->${m.to.join('+')}`).join(' ')}
            </span>
            <span data-testid="overlay-selected">{props.overlay.selected ?? ''}</span>
            <span data-testid="overlay-board">
              {props.overlay.board === null ? 'none' : 'kind' in props.overlay.board ? 'unavailable' : 'view'}
            </span>
          </>
        )}
      </div>
    );
  },
}));
vi.mock('../components/TranscriptPanel', () => ({ __esModule: true, default: () => <div data-testid="transcript-panel" /> }));
vi.mock('../components/run/StepTranscriptBrowser', () => ({ __esModule: true, default: () => <div /> }));
vi.mock('../components/RunEventFeed', () => ({ __esModule: true, default: () => <div data-testid="event-feed" /> }));
vi.mock('../components/charts/RunUsageTimeline', () => ({
  __esModule: true,
  default: () => {
    seen.pageRenders += 1;
    return <div />;
  },
}));

import RunDetail from './RunDetail';

function view(space: 'present' | 'absent'): MessagesView {
  return space === 'present'
    ? { participants: RUN_PARTICIPANTS, messages: RUN_MESSAGES, legacy: false, space, next: null }
    : { participants: [], messages: [], legacy: false, space, next: null };
}

function stub(space: 'present' | 'absent') {
  vi.spyOn(api, 'getRunAutoflow').mockResolvedValue(null);
  vi.spyOn(api, 'getRunUsage').mockRejectedValue(new ApiError(404, 'not found'));
  vi.spyOn(api, 'getRunGraph').mockResolvedValue(RUN_GRAPH_RESPONSE);
  vi.spyOn(api, 'getRunUsageTimeline').mockResolvedValue([]);
  vi.spyOn(api, 'subscribeRunLog').mockImplementation((_id, onEvent) => {
    for (const e of RUN_EVENTS) onEvent(e);
    return () => {};
  });
  vi.spyOn(api, 'getMessages').mockResolvedValue({ kind: 'ok', view: view(space) });
  return vi.spyOn(api, 'getRunBoard').mockResolvedValue({ kind: 'ok', board: RUN_BOARD });
}

function renderAt(entry: string) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path="/runs/:id" element={<RunDetail />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  seen.overlay = undefined;
  seen.renders = 0;
  seen.first = null;
  seen.pageRenders = 0;
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('RunDetail layer', () => {
  it('shows_the_layer_when_the_run_has_a_space', async () => {
    const board = stub('present');
    renderAt(`/runs/${RUN_ID}`);
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toHaveAttribute('data-overlay', 'on'));
    // The messages land on the steps' cards.
    await waitFor(() => expect(screen.getByTestId('overlay-arcs')).toHaveTextContent('msg_r2:probe->report'));
    expect(screen.getByTestId('overlay-arcs')).toHaveTextContent('msg_r4:operator->collect+probe+report');
    await waitFor(() => expect(screen.getByTestId('overlay-board')).toHaveTextContent('view'));
    expect(board).toHaveBeenCalledWith(RUN_ID, expect.objectContaining({ host: undefined }));
    // The time axis sits above the graph, with a band per step.
    const chrome = screen.getByTestId('run-graph-chrome');
    const track = within(chrome).getByTestId('time-track');
    expect(track.compareDocumentPosition(screen.getByTestId('run-graph-mock')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(track).getByTitle('probe')).toBeInTheDocument();
    // The Messages and Board panels below it; a row picked selects its arc.
    const list = screen.getByTestId('run-layer-messages');
    fireEvent.click(within(list).getByTestId('msg-row-msg_r2'));
    expect(screen.getByTestId('overlay-selected')).toHaveTextContent('msg_r2');
    expect(within(screen.getByTestId('run-layer-board')).getByText('package-lock.json')).toBeInTheDocument();
    // Replay idle: the whole run, as today.
    expect(seen.overlay?.revealed).toBeNull();
  });

  it('replay_reveals_from_the_start', async () => {
    stub('present');
    renderAt(`/runs/${RUN_ID}`);
    await waitFor(() => expect(screen.getByTestId('overlay-board')).toHaveTextContent('view'));
    await waitFor(() => expect(within(screen.getByTestId('run-layer-messages')).getByTestId('msg-row-msg_r6')).toBeInTheDocument());
    // Scrubbing to the start shows only what had happened by then.
    fireEvent.keyDown(screen.getByTestId('time-track'), { key: 'Home' });
    expect(seen.overlay?.revealed).toEqual(new Set(['collect']));
    expect(within(screen.getByTestId('run-layer-messages')).queryByTestId('msg-row-msg_r2')).toBeNull();
  });

  it('a_replay_renders_the_graph_not_the_page', async () => {
    stub('present');
    renderAt(`/runs/${RUN_ID}`);
    await waitFor(() => expect(within(screen.getByTestId('run-layer-messages')).getByTestId('msg-row-msg_r6')).toBeInTheDocument());
    await new Promise((r) => setTimeout(r, 50));
    const page = seen.pageRenders;
    const graph = seen.renders;
    // Scrubbing through the run moves the timeline many times.
    const track = screen.getByTestId('time-track');
    fireEvent.keyDown(track, { key: 'Home' });
    for (let i = 0; i < 8; i++) fireEvent.keyDown(track, { key: 'ArrowRight', shiftKey: true });
    expect(seen.renders).toBeGreaterThan(graph);
    expect(seen.pageRenders).toBe(page);
  });

  it('no_space_no_layer', async () => {
    const board = stub('absent');
    renderAt(`/runs/${RUN_ID}`);
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    await waitFor(() => expect(api.getMessages).toHaveBeenCalled());
    // Let the probe's answer land.
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.getByTestId('run-graph-mock')).toHaveAttribute('data-overlay', 'none');
    expect(seen.overlay).toBeUndefined();
    expect(screen.queryByTestId('time-track')).toBeNull();
    expect(screen.queryByTestId('run-layer-messages')).toBeNull();
    expect(screen.queryByTestId('run-layer-board')).toBeNull();
    expect(board).not.toHaveBeenCalled();
  });

  it('board_unavailable_is_said_in_place', async () => {
    const board = stub('present');
    board.mockResolvedValue({ kind: 'unavailable', reason: "This host can't show the board: host h-abc has no board route" });
    renderAt(`/runs/${RUN_ID}?host=h-abc`);
    await waitFor(() => expect(screen.getByTestId('overlay-board')).toHaveTextContent('unavailable'));
    expect(board).toHaveBeenCalledWith(RUN_ID, expect.objectContaining({ host: 'h-abc' }));
    const panel = screen.getByTestId('run-layer-board');
    expect(within(panel).getByText("This host can't show the board: host h-abc has no board route")).toBeInTheDocument();
    // The posts are messages: still listed.
    expect(within(panel).getByText('claim: package-lock.json')).toBeInTheDocument();
  });

  it('the_graph_is_drawn_once_the_space_is_known', async () => {
    // The probe answers after the graph loaded: the graph waits, then is
    // drawn with its layer from its first render (no height or axis jump).
    stub('present');
    let answer: (v: Awaited<ReturnType<typeof api.getMessages>>) => void = () => {};
    vi.mocked(api.getMessages).mockImplementationOnce(() => new Promise((r) => (answer = r)));
    renderAt(`/runs/${RUN_ID}`);
    await waitFor(() => expect(api.getRunGraph).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId('run-graph-mock')).toBeNull();
    expect(screen.getByText('Loading run…')).toBeInTheDocument();
    answer({ kind: 'ok', view: view('present') });
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument());
    expect(seen.first).toBe(true);
    expect(screen.getByTestId('time-track')).toBeInTheDocument();
  });

  it('a_slow_probe_does_not_hold_the_graph', async () => {
    stub('present');
    vi.mocked(api.getMessages).mockImplementation(() => new Promise(() => {}));
    renderAt(`/runs/${RUN_ID}`);
    await waitFor(() => expect(screen.getByTestId('run-graph-mock')).toBeInTheDocument(), { timeout: 3000 });
    expect(screen.getByTestId('run-graph-mock')).toHaveAttribute('data-overlay', 'none');
  });
});
