// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { api, type MessageStreamFrame, type SpaceMessage } from './api';
import { CAUGHT_UP_QUIET_MS, useMessageSpace } from './useMessageSpace';

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
