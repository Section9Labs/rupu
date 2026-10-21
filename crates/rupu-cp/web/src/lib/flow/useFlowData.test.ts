// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, type RunEvent, type RunGraphResponse } from '../api';
import { useFlowData } from './useFlowData';
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
  const getChildren = vi.spyOn(api, 'getAgentiflowChildren').mockResolvedValue(children);
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

describe('useFlowData', () => {
  it('children_and_board_reread_on_each_poll_while_running', async () => {
    const m = mockApi();
    const { result, rerender } = renderHook(({ detail, running }) => useFlowData(detail, { running }), {
      initialProps: { detail: DETAIL, running: true },
    });
    await waitFor(() => expect(result.current.children).toHaveLength(CHILDREN.length));
    expect(result.current.boards.get('flow')).toEqual(FLOW_BOARD);
    expect(m.getChildren).toHaveBeenCalledTimes(1);
    expect(m.getBoard).toHaveBeenCalledTimes(1);

    // A poll: a new detail object.
    rerender({ detail: { ...DETAIL }, running: true });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(2));
    expect(m.getBoard).toHaveBeenCalledTimes(2);

    // Stopped: children follow the detail, the board no longer does.
    rerender({ detail: { ...DETAIL }, running: false });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(3));
    rerender({ detail: { ...DETAIL }, running: false });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(4));
    expect(m.getBoard).toHaveBeenCalledTimes(3);
  });

  it('a_frame_loads_once_and_follows_only_while_its_child_runs', async () => {
    const running = CHILDREN.map((c) => (c.run_id === RUN.gannet1 ? { ...c, status: 'running' as const } : c));
    const m = mockApi(running);
    const { result, rerender } = renderHook(({ detail }) => useFlowData(detail, { running: true }), {
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

    // The child ended: one last read of its graph, and the follow stops.
    m.getChildren.mockResolvedValue(CHILDREN);
    rerender({ detail: { ...DETAIL } });
    await waitFor(() => expect(m.unsubscribe).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(m.getRunGraph).toHaveBeenCalledTimes(2));
    expect(m.subscribe).toHaveBeenCalledTimes(1);
    const boards = m.getRunBoard.mock.calls.length;
    rerender({ detail: { ...DETAIL } });
    await waitFor(() => expect(m.getChildren).toHaveBeenCalledTimes(4));
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
