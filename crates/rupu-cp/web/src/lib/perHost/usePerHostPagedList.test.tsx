// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, getConfig, renderHook, waitFor } from '@testing-library/react';
import { api, type RegisteredHostView } from '../api';
import { LOCAL_POLL_MS, REMOTE_POLL_MS, usePerHostPagedList } from './usePerHostPagedList';
import { REG_LOCAL, REG_PROD, callsFor, deferred } from './testUtils';

interface Row { id: string; started_at: string; host_id?: string }

afterEach(() => {
  // vitest runs with `globals: false`, so RTL's auto-cleanup is off: unmount explicitly, or
  // every mounted hook keeps its intervals and listeners into the next test. Before the
  // timers are restored, so a faked clearInterval clears the faked intervals.
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

/**
 * `waitFor` re-checks on the global `setInterval`, which the timer tests below fake, so
 * under fake intervals it never polls again (renderHook mutates no DOM). Same contract,
 * polled with the real `setTimeout` (only intervals are faked), inside RTL's async
 * wrapper so React's act() environment is handled exactly as `waitFor` does.
 */
async function until(assertion: () => void, timeoutMs = 1000): Promise<void> {
  await getConfig().asyncWrapper(async () => {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      try {
        assertion();
        return;
      } catch (e) {
        if (Date.now() >= deadline) throw e;
      }
      await new Promise((r) => setTimeout(r, 10));
    }
  });
}

function useList(fetch: (p: { host: string }) => Promise<Row[]>, host: string | null = null, poll = false) {
  return usePerHostPagedList<Row>({ host, fetch, timeField: 'started_at', idField: 'id', deps: [], poll });
}

describe('usePerHostPagedList', () => {
  it('fetches every registered host independently', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const fetch = vi.fn((p: { host: string }) =>
      Promise.resolve(p.host === 'local' ? [{ id: 'a', started_at: '2026-09-30T12:00:00Z' }] : []),
    );
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.rows).toHaveLength(1));
    expect(fetch.mock.calls.map((c) => c[0].host).sort()).toEqual(['host_prod', 'local']);
    expect(result.current.rows[0].host_id).toBe('local');
    expect(result.current.slices.map((s) => s.state)).toEqual(['ok', 'ok']);
  });

  it('single-host mode skips the registered-hosts read', async () => {
    const reg = vi.spyOn(api, 'getRegisteredHosts');
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    renderHook(() => useList(fetch, 'host_prod'));
    await waitFor(() => expect(fetch).toHaveBeenCalledWith(expect.objectContaining({ host: 'host_prod' })));
    expect(reg).not.toHaveBeenCalled();
  });

  it('falls back to this host when the host list cannot be read, and says so', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('boom'));
    const fetch = vi.fn((_p: { host: string }) => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.error).toMatch(/Could not list hosts/));
    expect(fetch.mock.calls.map((c) => c[0].host)).toEqual(['local']);
  });

  it("hands fetch the engine's abort signal untouched, and aborts it on unmount", async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    // A host that never answers: the request is still open when the page goes away.
    const fetch = vi.fn((_p: { host: string; signal?: AbortSignal }) => new Promise<Row[]>(() => {}));
    const { unmount } = renderHook(() => useList(fetch));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
    const signal = fetch.mock.calls[0][0].signal;
    expect(signal).toBeInstanceOf(AbortSignal);
    expect(signal?.aborted).toBe(false);
    unmount();
    expect(signal?.aborted).toBe(true);
  });

  it('polls local every 5s and remotes every 60s, remotes only while visible', async () => {
    // Only intervals are faked: waitFor and the engine's promise chains run on real timers.
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    renderHook(() => useList(fetch, null, true));
    await until(() => expect(fetch).toHaveBeenCalledTimes(2));

    await act(() => vi.advanceTimersByTimeAsync(LOCAL_POLL_MS));
    await until(() => expect(callsFor(fetch, 'local')).toHaveLength(2));
    expect(callsFor(fetch, 'host_prod')).toHaveLength(1);

    const vis = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    await act(() => vi.advanceTimersByTimeAsync(REMOTE_POLL_MS));
    await new Promise((r) => setTimeout(r, 10));
    expect(callsFor(fetch, 'host_prod')).toHaveLength(1);

    vis.mockReturnValue('visible');
    await act(() => vi.advanceTimersByTimeAsync(REMOTE_POLL_MS));
    await until(() => expect(callsFor(fetch, 'host_prod')).toHaveLength(2));

    const before = callsFor(fetch, 'host_prod').length;
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await until(() => expect(callsFor(fetch, 'host_prod').length).toBe(before + 1));
  });

  it('does not poll when poll is false', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    renderHook(() => useList(fetch));
    await until(() => expect(fetch).toHaveBeenCalledTimes(1));
    await act(() => vi.advanceTimersByTimeAsync(REMOTE_POLL_MS));
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('a successful Refresh clears the host-list error and adds the remote hosts', async () => {
    const reg = vi
      .spyOn(api, 'getRegisteredHosts')
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValue([REG_LOCAL, REG_PROD]);
    const fetch = vi.fn((_p: { host: string }) => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.error).toMatch(/Could not list hosts/));
    expect(result.current.slices.map((s) => s.hostId)).toEqual(['local']);

    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.slices.map((s) => s.hostId)).toEqual(['local', 'host_prod']));
    expect(reg).toHaveBeenCalledTimes(2);
    expect(result.current.error).toBeNull();
  });

  it('a Refresh whose host-list read fails again keeps the error', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('boom'));
    const fetch = vi.fn((_p: { host: string }) => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.error).toMatch(/Could not list hosts/));
    act(() => result.current.refresh());
    await waitFor(() => expect(callsFor(fetch, 'local')).toHaveLength(2));
    expect(result.current.error).toMatch(/Could not list hosts/);
  });

  it('manual refresh re-reads the host list and picks up a new host', async () => {
    const reg = vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.slices).toHaveLength(1));
    reg.mockResolvedValue([REG_LOCAL, REG_PROD]);
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.slices.map((s) => s.hostId)).toEqual(['local', 'host_prod']));
  });

  it('a generation change disposes the previous engine and ignores its late host-list answer', async () => {
    const first = deferred<RegisteredHostView[]>();
    const second = deferred<RegisteredHostView[]>();
    vi.spyOn(api, 'getRegisteredHosts').mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const fetch = vi.fn((_p: { host: string }) => Promise.resolve([] as Row[]));
    const { result, rerender } = renderHook(
      ({ dep }) => usePerHostPagedList<Row>({ host: null, fetch, timeField: 'started_at', idField: 'id', deps: [dep] }),
      { initialProps: { dep: 1 } },
    );
    // Generation 1 is still waiting for its host list when the filter changes.
    rerender({ dep: 2 });

    // Generation 1's answer lands late, naming a host generation 2 never asked for.
    await act(async () => {
      first.resolve([REG_LOCAL, REG_PROD]);
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(fetch).not.toHaveBeenCalled();
    expect(result.current.slices).toEqual([]);
    expect(result.current.loading).toBe(true); // generation 2 has not learned its hosts yet

    await act(async () => {
      second.resolve([REG_LOCAL]);
    });
    await waitFor(() => expect(result.current.slices.map((s) => s.hostId)).toEqual(['local']));
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(callsFor(fetch, 'host_prod')).toHaveLength(0);
  });

  it('error is null while any host answers, and names every host once all have failed', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const oneDown = vi.fn((p: { host: string }) =>
      p.host === 'local' ? Promise.resolve([] as Row[]) : Promise.reject(new Error('prod down')),
    );
    const partial = renderHook(() => useList(oneDown));
    await waitFor(() => expect(partial.result.current.slices.map((s) => s.state)).toEqual(['ok', 'offline']));
    expect(partial.result.current.error).toBeNull();
    partial.unmount();

    const allDown = vi.fn((p: { host: string }) =>
      Promise.reject(new Error(p.host === 'local' ? 'local down' : 'prod down')),
    );
    const total = renderHook(() => useList(allDown));
    await waitFor(() => expect(total.result.current.error).not.toBeNull());
    expect(total.result.current.error).toBe('Local: local down · prod: prod down');
  });
});
