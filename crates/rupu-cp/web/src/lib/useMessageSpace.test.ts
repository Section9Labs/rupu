// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, ApiError, type MessageStreamFrame, type SpaceMessage } from './api';
import { CAUGHT_UP_QUIET_MS, REMOTE_PAGE_SIZE, useMessageSpace } from './useMessageSpace';

afterEach(() => vi.restoreAllMocks());

const msg = (n: number): SpaceMessage => ({
  id: `msg_${n}`,
  ts: new Date(Date.UTC(2026, 9, 20, 14, n)).toISOString(),
  kind: 'direct',
  from: 'a',
  to: 'b',
  body: `m${n}`,
  delivery: { resolved: ['b'] },
  seen: [],
});

describe('useMessageSpace caughtUp', () => {
  it('a_paged_read_has_caught_up_after_its_last_page', async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    vi.spyOn(api, 'getMessages').mockImplementation(async (_route, _id, opts) => {
      if (opts?.limit === 1) return { kind: 'ok', view: { participants: [], messages: [], legacy: false, space: 'present' } };
      if (opts?.after === 'p2') {
        await gate;
        return { kind: 'ok', view: { participants: [], messages: [msg(2)], legacy: false, space: 'present' } };
      }
      return { kind: 'ok', view: { participants: [], messages: [msg(1)], legacy: false, space: 'present', next: 'p2' } };
    });
    const { result } = renderHook(() => useMessageSpace({ route: 'runs', id: 'run_1' }, { live: false, load: true, reading: false }));
    await waitFor(() => expect(result.current.log.messages).toHaveLength(1));
    expect(result.current.caughtUp).toBe(false);
    await act(async () => release());
    await waitFor(() => expect(result.current.caughtUp).toBe(true));
    expect(result.current.log.messages).toHaveLength(2);
  });

  it('a_stream_has_caught_up_once_its_replay_goes_quiet', async () => {
    vi.spyOn(api, 'getMessages').mockResolvedValue({
      kind: 'ok',
      view: { participants: [], messages: [], legacy: false, space: 'present' },
    });
    let handlers!: { onFrame: (f: MessageStreamFrame) => void; onOpen?: () => void };
    vi.spyOn(api, 'subscribeMessages').mockImplementation((_r, _i, h) => {
      handlers = h;
      return () => {};
    });
    const { result } = renderHook(() => useMessageSpace({ route: 'runs', id: 'run_1' }, { live: true, load: true, reading: false }));
    await waitFor(() => expect(handlers).toBeDefined());
    act(() => {
      handlers.onOpen?.();
      handlers.onFrame({ t: 'msg', ...msg(1) });
    });
    // The first batch lands (loaded) before the replay is over.
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.caughtUp).toBe(false);
    act(() => handlers.onFrame({ t: 'msg', ...msg(2) }));
    await new Promise((r) => setTimeout(r, CAUGHT_UP_QUIET_MS / 2));
    expect(result.current.caughtUp).toBe(false);
    await waitFor(() => expect(result.current.caughtUp).toBe(true), { timeout: CAUGHT_UP_QUIET_MS * 3 });
    expect(result.current.log.messages).toHaveLength(2);
  });
});

describe('useMessageSpace reads', () => {
  it('a_tick_reads_what_was_logged_since_without_a_stream', async () => {
    const log = [msg(1), msg(2)];
    const get = vi.spyOn(api, 'getMessages').mockImplementation(async (_route, _id, opts) => {
      const start = opts?.after ? log.findIndex((m) => m.id === opts.after) + 1 : 0;
      const messages = opts?.limit === 1 ? [] : log.slice(start);
      return { kind: 'ok', view: { participants: [], messages, legacy: false, space: 'present' } };
    });
    const stream = vi.spyOn(api, 'subscribeMessages');
    const { result, rerender } = renderHook(
      ({ tick }) => useMessageSpace({ route: 'runs', id: 'run_1' }, { live: false, load: true, reading: false, tick }),
      { initialProps: { tick: 1 as unknown } },
    );
    await waitFor(() => expect(result.current.caughtUp).toBe(true));
    expect(result.current.log.messages.map((m) => m.id)).toEqual(['msg_1', 'msg_2']);
    // Two messages more, then the next tick: only what follows msg_2 is read.
    log.push(msg(3), msg(4));
    get.mockClear();
    rerender({ tick: 2 });
    await waitFor(() => expect(result.current.log.messages).toHaveLength(4));
    expect(get).toHaveBeenCalledTimes(1);
    expect(get.mock.calls[0][2]).toMatchObject({ after: 'msg_2' });
    // The same tick reads nothing.
    rerender({ tick: 2 });
    await new Promise((r) => setTimeout(r, 20));
    expect(get).toHaveBeenCalledTimes(1);
    expect(stream).not.toHaveBeenCalled();
  });

  it('a_remote_log_is_read_in_large_pages', async () => {
    const get = vi.spyOn(api, 'getMessages').mockResolvedValue({
      kind: 'ok',
      view: { participants: [], messages: [msg(1)], legacy: false, space: 'present' },
    });
    const { result } = renderHook(() =>
      useMessageSpace({ route: 'runs', id: 'run_1', host: 'mini' }, { live: true, load: true, reading: false }),
    );
    await waitFor(() => expect(result.current.caughtUp).toBe(true));
    const pages = get.mock.calls.filter((c) => c[2]?.limit !== 1);
    expect(pages).toHaveLength(1);
    expect(pages[0][2]).toMatchObject({ host: 'mini', limit: REMOTE_PAGE_SIZE });
  });
});

describe('useMessageSpace by tick', () => {
  it('a_space_that_appears_later_is_found_on_a_tick', async () => {
    // The run isn't there yet (a workflow child still starting), then its
    // space is empty-absent, then present with a message.
    let state: 'missing' | 'absent' | 'present' = 'missing';
    vi.spyOn(api, 'getMessages').mockImplementation(async () => {
      if (state === 'missing') throw new ApiError(404, 'no run', '{"error":"no run `run_1`"}');
      return {
        kind: 'ok',
        view: { participants: [], messages: state === 'present' ? [msg(1)] : [], legacy: false, space: state },
      };
    });
    const { result, rerender } = renderHook(
      ({ tick }) => useMessageSpace({ route: 'runs', id: 'run_1' }, { live: false, load: true, reading: false, tick }),
      { initialProps: { tick: 0 as unknown } },
    );
    await waitFor(() => expect(result.current.status).toBe('error'));
    state = 'absent';
    rerender({ tick: 1 });
    await waitFor(() => expect(result.current.space).toBe('absent'));
    state = 'present';
    rerender({ tick: 2 });
    await waitFor(() => expect(result.current.space).toBe('present'));
    await waitFor(() => expect(result.current.log.messages.map((m) => m.id)).toEqual(['msg_1']));
    expect(result.current.status).toBe('ready');
  });
});
