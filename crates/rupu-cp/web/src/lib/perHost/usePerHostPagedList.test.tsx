// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, getConfig, renderHook, waitFor } from '@testing-library/react';
import { api } from '../api';
import { LOCAL_POLL_MS, REMOTE_POLL_MS, usePerHostPagedList } from './usePerHostPagedList';
import { REG_LOCAL, REG_PROD, callsFor } from './testUtils';

interface Row { id: string; started_at: string; host_id?: string }

afterEach(() => {
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

  it('manual refresh re-reads the host list and picks up a new host', async () => {
    const reg = vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const fetch = vi.fn(() => Promise.resolve([] as Row[]));
    const { result } = renderHook(() => useList(fetch));
    await waitFor(() => expect(result.current.slices).toHaveLength(1));
    reg.mockResolvedValue([REG_LOCAL, REG_PROD]);
    act(() => result.current.refresh());
    await waitFor(() => expect(result.current.slices.map((s) => s.hostId)).toEqual(['local', 'host_prod']));
  });
});
