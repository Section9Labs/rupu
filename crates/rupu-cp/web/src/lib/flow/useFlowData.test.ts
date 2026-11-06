// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, type AgentiflowDetail, type AgentiflowUnit, type RunEvent, type RunGraphResponse } from '../api';
import { CHILDREN_LIVE_MS, eventWindow, reuseChildren, useFlowData } from './useFlowData';
import { CHILDREN, DETAIL, FLOW_BOARD, GANNET_BOARD, RUN } from './fixtures';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

const GRAPH = {
  run: {},
  workflow: {
    steps: [
      { id: 'collect', kind: 'step', agent: 'endpoint-collector' },
      { id: 'report', kind: 'step', agent: 'finding-writer' },
    ],
    edges: [{ from: 'collect', to: 'report' }],
  },
  step_results: [],
  units: [],
} as unknown as RunGraphResponse;

function mockApi(children = CHILDREN) {
  const getChildren = vi.spyOn(api, 'getAgentiflowChildTree').mockResolvedValue({ children, truncated: false });
  const getBoard = vi.spyOn(api, 'getAgentiflowBoard').mockResolvedValue({ kind: 'ok', board: FLOW_BOARD });
  const getRunBoard = vi.spyOn(api, 'getRunBoard').mockResolvedValue({ kind: 'ok', board: GANNET_BOARD });
  const getRunGraph = vi.spyOn(api, 'getRunGraph').mockResolvedValue(GRAPH);
  // A frame follows its child through the shared firehose (every run's
  // events), after reading the child's log once; it never opens a stream
  // of its own.
  let onEvent: ((e: RunEvent) => void) | null = null;
  const unsubscribe = vi.fn();
  const subscribe = vi.spyOn(api, 'subscribeEvents').mockImplementation((cb) => {
    onEvent = cb;
    return unsubscribe;
  });
  const readLog = vi.spyOn(api, 'readRunLog').mockResolvedValue([]);
  const ownStream = vi.spyOn(api, 'subscribeRunLog');
  return {
    getChildren,
    getBoard,
    getRunBoard,
    getRunGraph,
    subscribe,
    unsubscribe,
    readLog,
    ownStream,
    emit: (e: Omit<RunEvent, 'run_id'> & { run_id?: string }) => onEvent?.({ ...e, run_id: e.run_id ?? RUN.gannet1 } as RunEvent),
  };
}

/** A fixed clock (children are re-read past CHILDREN_LIVE_MS while running). */
let clock = 0;
const now = () => clock;
/** A poll whose detail shows `n` units: a change the children follow. */
const withUnits = (n: number): AgentiflowDetail => ({
  ...DETAIL,
  units: Array.from({ length: n }, (_, i) => ({ unit_id: `u${i}`, status: 'running' }) as unknown as AgentiflowUnit),
});

describe('useFlowData', () => {
  it('children_reread_when_the_detail_changes_the_board_on_each_poll', async () => {
    clock = 0;
    const m = mockApi();
    const { result, rerender } = renderHook(({ detail, running }) => useFlowData(detail, { running, now }), {
      initialProps: { detail: DETAIL, running: true },
    });
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));
    expect(result.current.boards.get('flow')).toEqual(FLOW_BOARD);
    expect(m.getChildren).toHaveBeenCalledTimes(1);
    expect(m.getBoard).toHaveBeenCalledTimes(1);

    // A poll that changed nothing: the board is re-read, the children
    // (a walk of every ledger) are not.
    rerender({ detail: { ...DETAIL }, running: true });
    await waitFor(() => expect(m.getBoard).toHaveBeenCalledTimes(2));
    expect(m.getChildren).toHaveBeenCalledTimes(1);
    // A unit appeared: they are.
    rerender({ detail: withUnits(1), running: true });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(2));
    // While it runs, at most every CHILDREN_LIVE_MS without a change.
    clock += CHILDREN_LIVE_MS;
    rerender({ detail: withUnits(1), running: true });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(3));

    // Stopped: once more, then the board and children rest.
    rerender({ detail: withUnits(1), running: false });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(4));
    const boards = m.getBoard.mock.calls.length;
    clock += CHILDREN_LIVE_MS;
    rerender({ detail: { ...withUnits(1) }, running: false });
    await new Promise((r) => setTimeout(r, 20));
    expect(m.getChildren).toHaveBeenCalledTimes(4);
    expect(m.getBoard).toHaveBeenCalledTimes(boards);
  });

  it('a_truncated_walk_is_said', async () => {
    const m = mockApi();
    m.getChildren.mockResolvedValue({ children: CHILDREN, truncated: true });
    const { result } = renderHook(() => useFlowData(DETAIL, { running: false, now }));
    await waitFor(() => expect(result.current.childrenTruncated).toBe(true));
  });

  it('a_frame_loads_once_and_follows_only_while_its_child_runs', async () => {
    const running = CHILDREN.map((c) => (c.run_id === RUN.gannet1 ? { ...c, status: 'running' as const } : c));
    const m = mockApi(running);
    clock = 0;
    const { result, rerender } = renderHook(({ detail }) => useFlowData(detail, { running: true, now }), {
      initialProps: { detail: DETAIL },
    });
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));

    act(() => result.current.loadFrame(RUN.gannet1));
    act(() => result.current.loadFrame(RUN.gannet1));
    await waitFor(() => expect(result.current.childGraphs.get(RUN.gannet1)).toBeDefined());
    expect(result.current.loadedFrames).toEqual([RUN.gannet1]);
    expect(m.getRunGraph).toHaveBeenCalledTimes(1);
    expect(m.getRunGraph).toHaveBeenCalledWith(RUN.gannet1);
    expect(m.subscribe).toHaveBeenCalledTimes(1);
    expect(m.readLog).toHaveBeenCalledWith(RUN.gannet1, expect.anything());
    expect(m.ownStream).not.toHaveBeenCalled();
    expect(result.current.boards.get(RUN.gannet1)).toEqual(GANNET_BOARD);
    expect(result.current.childGraphs.get(RUN.gannet1)!.nodeById('collect')!.state).toBe('pending');

    // A live event rebuilds the frame's graph (batched, about once a second).
    act(() => m.emit({ type: 'step_started', run_id: RUN.gannet1, step_id: 'collect' } as RunEvent));
    await waitFor(() => expect(result.current.childGraphs.get(RUN.gannet1)!.nodeById('collect')!.state).toBe('running'), {
      timeout: 2500,
    });
    // Another run's line on the firehose is not this frame's.
    act(() => m.emit({ type: 'step_completed', run_id: 'run_other', step_id: 'collect' } as RunEvent));
    await new Promise((r) => setTimeout(r, 1200));
    expect(result.current.childGraphs.get(RUN.gannet1)!.nodeById('collect')!.state).toBe('running');
    // An event that changes nothing the map draws keeps the same model, so
    // the map isn't rebuilt.
    const before = result.current.childGraphs;
    act(() => m.emit({ type: 'step_working', run_id: RUN.gannet1, step_id: 'collect' } as RunEvent));
    await new Promise((r) => setTimeout(r, 1200));
    expect(result.current.childGraphs).toBe(before);

    // A poll while the child runs re-reads its board, not its graph.
    rerender({ detail: { ...DETAIL } });
    await waitFor(() => expect(m.getRunBoard).toHaveBeenCalledTimes(2));
    expect(m.getRunGraph).toHaveBeenCalledTimes(1);

    // The child ended (the detail changed with it): one last read of its
    // graph, and the follow stops.
    m.getChildren.mockResolvedValue({ children: CHILDREN, truncated: false });
    rerender({ detail: withUnits(1) });
    await waitFor(() => expect(m.unsubscribe).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(m.getRunGraph).toHaveBeenCalledTimes(2));
    expect(m.subscribe).toHaveBeenCalledTimes(1);
    const boards = m.getRunBoard.mock.calls.length;
    rerender({ detail: withUnits(2) });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(3));
    expect(m.getRunBoard).toHaveBeenCalledTimes(boards);
    expect(m.getRunGraph).toHaveBeenCalledTimes(2);
  });

  it('a_running_frame_starts_from_its_log_then_the_firehose', async () => {
    const running = CHILDREN.map((c) => (c.run_id === RUN.gannet1 ? { ...c, status: 'running' as const } : c));
    const m = mockApi(running);
    const started = { type: 'step_started', run_id: RUN.gannet1, step_id: 'collect' } as RunEvent;
    let replay: (events: RunEvent[]) => void = () => {};
    m.readLog.mockImplementation(() => new Promise((r) => (replay = r)));
    const { result } = renderHook(() => useFlowData(DETAIL, { running: true }));
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));
    act(() => result.current.loadFrame(RUN.gannet1));
    await waitFor(() => expect(result.current.childGraphs.get(RUN.gannet1)).toBeDefined());
    // The firehose brings a line while the log is still replaying; the
    // replay holds it too.
    act(() => m.emit(started));
    await act(async () => replay([started]));
    await waitFor(() => expect(result.current.childGraphs.get(RUN.gannet1)!.nodeById('collect')!.state).toBe('running'), {
      timeout: 2500,
    });
  });

  it('a_frame_keeps_each_line_once_when_the_firehose_replays_it_later', async () => {
    const running = CHILDREN.map((c) => (c.run_id === RUN.gannet1 ? { ...c, status: 'running' as const } : c));
    const m = mockApi(running);
    const warn = { type: 'step_warning', run_id: RUN.gannet1, step_id: 'collect', message: 'slow host' } as RunEvent;
    const started = { type: 'step_started', run_id: RUN.gannet1, step_id: 'collect' } as RunEvent;
    m.readLog.mockResolvedValue([started, warn]);
    const { result } = renderHook(() => useFlowData(DETAIL, { running: true, now }));
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));
    act(() => result.current.loadFrame(RUN.gannet1));
    const collect = () => result.current.childGraphs.get(RUN.gannet1)?.nodeById('collect');
    const warnings = () => collect()?.warnings?.map((w) => w.message);
    await waitFor(() => expect(warnings()).toEqual(['slow host']), { timeout: 2500 });
    // The firehose attaches the run after the replay resolved (or
    // reconnects) and drains its log from the start: the same lines again.
    act(() => {
      m.emit(started);
      m.emit(warn);
    });
    await new Promise((r) => setTimeout(r, 1200));
    expect(warnings()).toEqual(['slow host']);
    // A new line still lands.
    act(() => m.emit({ type: 'step_warning', run_id: RUN.gannet1, step_id: 'collect', message: 'retrying' } as RunEvent));
    await waitFor(() => expect(warnings()).toEqual(['slow host', 'retrying']), { timeout: 2500 });
  });

  it('frame_spaces_join_the_flow_space', async () => {
    mockApi();
    const { result } = renderHook(() => useFlowData(DETAIL, { running: false }));
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));
    expect(result.current.spaces.map((s) => s.id)).toEqual(['flow']);
    act(() => result.current.loadFrame(RUN.gannet1));
    act(() => result.current.setFrameSpace(RUN.gannet1, { id: RUN.gannet1, participants: [], messages: [] }));
    expect(result.current.spaces.map((s) => s.id)).toEqual(['flow', RUN.gannet1]);
  });

  it('a_units_agent_started_redraws_the_frame', async () => {
    const running = CHILDREN.map((c) => (c.run_id === RUN.gannet1 ? { ...c, status: 'running' as const } : c));
    const m = mockApi(running);
    m.getRunGraph.mockResolvedValue({
      ...GRAPH,
      workflow: { steps: [{ id: 'probe', kind: 'for_each', agent: 'auth-prober' }], edges: [] },
    } as unknown as RunGraphResponse);
    const { result } = renderHook(() => useFlowData(DETAIL, { running: true }));
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));
    act(() => result.current.loadFrame(RUN.gannet1));
    await waitFor(() => expect(m.readLog).toHaveBeenCalled());
    await act(async () => {});
    act(() =>
      m.emit({ type: 'unit_started', step_id: 'probe', index: 0, unit_key: '/api/login', agent: 'auth-prober', transcript_path: '/t' } as RunEvent),
    );
    const unit = () => result.current.childGraphs.get(RUN.gannet1)?.nodeById('probe')?.fanout?.units[0];
    await waitFor(() => expect(unit()?.state).toBe('running'), { timeout: 2500 });
    expect(unit()!.model).toBeUndefined();
    // The unit's agent starts on a model: same state, same codename, but
    // the card names its provider/model, so the frame is redrawn.
    act(() =>
      m.emit({
        type: 'agent_started', step_id: 'probe', unit_index: 0, agent: 'auth-prober', provider: 'anthropic',
        model: 'claude-opus-5-5', agent_run_id: 'run_01M5GPROBE0', transcript_path: '/t',
      } as RunEvent),
    );
    await waitFor(() => expect(unit()?.model).toBe('claude-opus-5-5'), { timeout: 2500 });
    expect(unit()!.provider).toBe('anthropic');
  });
});

describe('reuseChildren', () => {
  it('keeps_unchanged_children_and_the_list_when_nothing_changed', () => {
    const copy = CHILDREN.map((c) => ({ ...c }));
    expect(reuseChildren(CHILDREN, copy)).toBe(CHILDREN);
    const changed = copy.map((c, i) => (i === 1 ? { ...c, status: 'failed' as const } : c));
    const out = reuseChildren(CHILDREN, changed);
    expect(out).not.toBe(CHILDREN);
    expect(out[0]).toBe(CHILDREN[0]);
    expect(out[1]).toBe(changed[1]);
    expect(out[2]).toBe(CHILDREN[2]);
  });
});

describe('eventWindow', () => {
  it('keeps_each_line_once_for_its_whole_life', () => {
    const w = eventWindow(5);
    const line = (n: number) => ({ type: 'step_warning', run_id: 'run_1', step_id: 's', message: `w${n}` }) as RunEvent;
    // The log's replay, then the firehose's backlog of the same lines
    // (attached late, or after a reconnect), then a new one.
    for (const n of [1, 2, 3]) expect(w.add(line(n))).toBe(true);
    for (const n of [1, 2, 3]) expect(w.add(line(n))).toBe(false);
    expect(w.add(line(4))).toBe(true);
    expect(w.events.map((e) => (e as { message: string }).message)).toEqual(['w1', 'w2', 'w3', 'w4']);
    // Past its size it keeps the latest.
    for (const n of [5, 6, 7]) w.add(line(n));
    expect(w.events.map((e) => (e as { message: string }).message)).toEqual(['w3', 'w4', 'w5', 'w6', 'w7']);
    expect(w.add(line(7))).toBe(false);
  });
});
