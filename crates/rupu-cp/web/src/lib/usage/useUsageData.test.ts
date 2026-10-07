// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, getConfig, renderHook, waitFor } from '@testing-library/react';
import { ApiError, api, type UsageResponse, type UsageWindow } from '../api';
import { USAGE_REMOTE_POLL_MS, useUsageData, type UseUsageDataResult } from './useUsageData';
import { FETCH_TIMEOUT_MS } from '../perHost/engine';
import { REG_LOCAL, REG_PROD, callsFor, deferred, flush } from '../perHost/testUtils';

const WIN: UsageWindow = { since: '2026-09-01T00:00:00.000Z', until: '2026-09-30T00:00:00.000Z' };

function resp(hostId: string, runs: number, state: 'ok' | 'offline' | 'unavailable' = 'ok'): UsageResponse {
  return {
    summary: { input_tokens: runs, output_tokens: 0, cached_tokens: 0, total_tokens: runs, cost_usd: runs, priced: true, runs },
    breakdown: [],
    unpriced: { models: [], rows: 0 },
    hosts: [{ host_id: hostId, name: hostId, transport_kind: 'ssh', state, captured_at: state === 'ok' ? '2026-09-30T00:00:00Z' : null, reason: state === 'ok' ? null : 'down' }],
  };
}
/** getUsage's 3rd arg is the host — adapt callsFor's object-param shape. */
const usageCallsFor = (host: string) =>
  callsFor({ mock: { calls: vi.mocked(api.getUsage).mock.calls.map((c) => [{ host: c[2] }]) } }, host);

afterEach(() => {
  // vitest runs with `globals: false`, so RTL's auto-cleanup is off: unmount explicitly, or
  // every mounted hook keeps its intervals and listeners into the next test. Before the
  // timers are restored, so a faked clearInterval clears the faked intervals.
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

/**
 * `waitFor` re-checks on the global `setInterval`, which the tick test fakes, so under
 * fake intervals it never polls again (renderHook mutates no DOM). Same contract, polled
 * with the real `setTimeout` (only intervals are faked), inside RTL's async wrapper so
 * React's act() environment is handled exactly as `waitFor` does.
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

/** A request that never answers, but rejects with an AbortError when its signal fires (as fetch does). */
function hangUntilAborted(signal: AbortSignal | undefined): Promise<UsageResponse> {
  return new Promise<UsageResponse>((_, reject) => {
    signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
  });
}

describe('useUsageData', () => {
  it('shows local as soon as it answers, while a remote is still loading', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      host === 'local' ? Promise.resolve(resp('local', 3)) : new Promise(() => {}),
    );
    const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(3));
    expect(result.current.data?.excluded).toEqual(['prod (loading)']);
    expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'loading']);
  });

  it("treats each response's own host entry as authoritative", async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      Promise.resolve(host === 'local' ? resp('local', 3) : resp('host_prod', 99, 'offline')),
    );
    const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.hosts[1]?.state).toBe('offline'));
    expect(result.current.data?.summary.runs).toBe(3);
    expect(result.current.data?.excluded).toEqual(['prod (offline)']);
  });

  it('pins group_by to model and passes the host', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    const spy = vi.spyOn(api, 'getUsage').mockResolvedValue(resp('local', 1));
    renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(spy).toHaveBeenCalledWith(WIN, 'model', 'local', expect.any(AbortSignal)));
  });

  it('a tick refetches local only; remotes keep their 60s cadence', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => Promise.resolve(resp(host ?? 'local', 1)));
    const { rerender } = renderHook(({ w, src }) => useUsageData(w, 'preset:30d', src), {
      initialProps: { w: WIN, src: 'user' as 'user' | 'tick' },
    });
    await until(() => expect(usageCallsFor('host_prod')).toHaveLength(1));
    rerender({ w: { ...WIN, until: '2026-09-30T00:00:30.000Z' }, src: 'tick' });
    await until(() => expect(usageCallsFor('local')).toHaveLength(2));
    expect(usageCallsFor('host_prod')).toHaveLength(1);
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    await act(() => vi.advanceTimersByTimeAsync(USAGE_REMOTE_POLL_MS));
    await until(() => expect(usageCallsFor('host_prod')).toHaveLength(2));
  });

  it('a user window change refetches every host and excludes answers for the old window', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    let remoteHangs = false;
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      host !== 'local' && remoteHangs ? new Promise(() => {}) : Promise.resolve(resp(host ?? 'local', host === 'local' ? 1 : 10)),
    );
    const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
      initialProps: { key: 'preset:30d' },
    });
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));
    remoteHangs = true;
    rerender({ key: 'preset:7d' });
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(1));
    expect(result.current.data?.excluded).toEqual(['prod (loading)']);
  });

  it("a newer request for a host aborts its previous one, and the aborted request's rejection changes nothing", async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const remoteSignals: AbortSignal[] = [];
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host, signal) => {
      if (host === 'local') return Promise.resolve(resp('local', 1));
      if (signal) remoteSignals.push(signal);
      return hangUntilAborted(signal);
    });
    const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
      initialProps: { key: 'preset:30d' },
    });
    await waitFor(() => expect(remoteSignals).toHaveLength(1));
    expect(remoteSignals[0]?.aborted).toBe(false);

    // The user moves the window while the remote is still pending: the old request is dead weight.
    rerender({ key: 'preset:7d' });
    await waitFor(() => expect(remoteSignals).toHaveLength(2));
    expect(remoteSignals[0]?.aborted).toBe(true);
    expect(remoteSignals[1]?.aborted).toBe(false);

    // Let the aborted promise's rejection run: it must not mark the remote offline.
    await flush();
    await flush();
    expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'loading']);
    expect(result.current.hosts[1]?.reason).toBeNull();
    expect(result.current.error).toBeNull();
    expect(result.current.data?.excluded).toEqual(['prod (loading)']);
  });

  it('aborts the requests still in flight on unmount, and a late rejection touches nothing', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const signals: Record<string, AbortSignal | undefined> = {};
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host, signal) => {
      signals[host ?? 'local'] = signal;
      return host === 'local' ? Promise.resolve(resp('local', 1)) : hangUntilAborted(signal);
    });
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { result, unmount } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(1));
    expect(signals.host_prod?.aborted).toBe(false);
    unmount();
    expect(signals.host_prod).toBeInstanceOf(AbortSignal);
    expect(signals.host_prod?.aborted).toBe(true);
    await flush();
    expect(errorSpy).not.toHaveBeenCalled();
  });

  it("drops a superseded request's late answer even when it resolves with data", async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    // The first remote request ignores its abort signal and answers only when told to.
    const lateA = deferred<UsageResponse>();
    let remoteCalls = 0;
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => {
      if (host === 'local') return Promise.resolve(resp('local', 1));
      remoteCalls += 1;
      return remoteCalls === 1 ? lateA.promise : Promise.resolve(resp('host_prod', 20));
    });
    const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
      initialProps: { key: 'preset:30d' },
    });
    await waitFor(() => expect(remoteCalls).toBe(1));
    rerender({ key: 'preset:7d' });
    await waitFor(() => expect(result.current.data?.summary.runs).toBe(21));

    lateA.resolve(resp('host_prod', 10)); // the old window's answer, arriving after the new one
    await flush();
    await flush();
    expect(result.current.data?.summary.runs).toBe(21);
    expect(result.current.data?.excluded).toEqual([]);
    expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'ok']);
  });

  describe('a host that answered for an old window then fails for the current one', () => {
    /** Both hosts answer `preset:30d`, then the mode is switched and the window moved. */
    function setup(mode: { current: (host: string) => Promise<UsageResponse> }) {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => mode.current(host ?? 'local'));
      return renderHook(({ key }) => useUsageData(WIN, key, 'user'), { initialProps: { key: 'preset:30d' } });
    }
    const answer = (host: string) => Promise.resolve(resp(host, host === 'local' ? 1 : 10));
    const down = () => Promise.reject(new Error('down'));

    it('is failed, not loading forever, once every host has failed for the new window', async () => {
      const mode = { current: answer };
      const { result, rerender } = setup(mode);
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      mode.current = down;
      rerender({ key: 'preset:7d' });
      await waitFor(() => expect(result.current.error).not.toBeNull());
      expect(result.current.error?.message).toMatch(/Local: down · prod: down/);
      expect(result.current.hosts.map((h) => h.state)).toEqual(['offline', 'offline']);
      expect(result.current.hosts.map((h) => h.reason)).toEqual(['down', 'down']);
      expect(result.current.data).toBeNull();
    });

    it('is labelled (stale) and offline while the other hosts answer', async () => {
      const mode = { current: answer };
      const { result, rerender } = setup(mode);
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      mode.current = (host) => (host === 'local' ? answer(host) : down());
      rerender({ key: 'preset:7d' });
      await waitFor(() => expect(result.current.hosts[1]?.state).toBe('offline'));
      expect(result.current.data?.summary.runs).toBe(1);
      expect(result.current.data?.excluded).toEqual(['prod (stale)']);
      expect(result.current.error).toBeNull();
    });

    it('reads loading, not stale, for a window it has not failed for', async () => {
      const mode = { current: answer };
      const { result, rerender } = setup(mode);
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));
      mode.current = (host) => (host === 'local' ? answer(host) : down());
      rerender({ key: 'preset:7d' });
      await waitFor(() => expect(result.current.hosts[1]?.state).toBe('offline'));

      // Move again; nothing answers yet. The failure was for 7d, not 90d, so nothing reads failed.
      const localC = deferred<UsageResponse>();
      mode.current = (host) => (host === 'local' ? localC.promise : new Promise(() => {}));
      rerender({ key: 'preset:90d' });
      await flush();
      expect(result.current.hosts.map((h) => h.state)).toEqual(['loading', 'loading']);
      expect(result.current.error).toBeNull();

      localC.resolve(resp('local', 1));
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(1));
      expect(result.current.data?.excluded).toEqual(['prod (loading)']);
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'loading']);
    });
  });

  describe('a host that has failed for the current window', () => {
    const answer = (host: string) => Promise.resolve(resp(host, host === 'local' ? 1 : 10));
    const down = () => Promise.reject(new Error('down'));
    const focus = async () => {
      vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
      await act(async () => {
        document.dispatchEvent(new Event('visibilitychange'));
      });
    };

    it('stays failed while a retry for the same window is in flight, and recovers when it answers', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      const mode = { current: (host: string): Promise<UsageResponse> => answer(host) };
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => mode.current(host ?? 'local'));
      const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
        initialProps: { key: 'preset:30d' },
      });
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      mode.current = down; // both fail FOR 7d
      rerender({ key: 'preset:7d' });
      await waitFor(() => expect(result.current.error).not.toBeNull());
      expect(result.current.hosts.map((h) => h.state)).toEqual(['offline', 'offline']);

      // A focus retry for the same window is now in flight. It must not turn the error back into a spinner.
      const retry = { local: deferred<UsageResponse>(), host_prod: deferred<UsageResponse>() };
      mode.current = (host) => (host === 'local' ? retry.local.promise : retry.host_prod.promise);
      const calls = usageCallsFor('host_prod').length;
      await focus();
      await waitFor(() => expect(usageCallsFor('host_prod')).toHaveLength(calls + 1));
      await flush();
      rerender({ key: 'preset:7d' }); // any render while the retry is pending (a tick, a re-render of the page)
      expect(result.current.hosts.map((h) => h.state)).toEqual(['offline', 'offline']);
      expect(result.current.error).not.toBeNull();
      expect(result.current.data).toBeNull();

      retry.local.resolve(resp('local', 1));
      retry.host_prod.resolve(resp('host_prod', 10));
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'ok']);
      expect(result.current.error).toBeNull();
    });

    it('keeps its (stale) label while a retry for the same window is in flight', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      const mode = { current: (host: string): Promise<UsageResponse> => answer(host) };
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => mode.current(host ?? 'local'));
      const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
        initialProps: { key: 'preset:30d' },
      });
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      mode.current = (host) => (host === 'local' ? answer(host) : down());
      rerender({ key: 'preset:7d' });
      await waitFor(() => expect(result.current.hosts[1]?.state).toBe('offline'));
      expect(result.current.data?.excluded).toEqual(['prod (stale)']);

      mode.current = (host) => (host === 'local' ? answer(host) : new Promise(() => {}));
      const calls = usageCallsFor('host_prod').length;
      await focus();
      await waitFor(() => expect(usageCallsFor('host_prod')).toHaveLength(calls + 1));
      await flush();
      expect(result.current.data?.excluded).toEqual(['prod (stale)']);
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'offline']);
    });
  });

  describe('a host list that cannot be read', () => {
    it('is a notice, not an error: local still loads and shows', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('boom'));
      const local = deferred<UsageResponse>();
      vi.spyOn(api, 'getUsage').mockReturnValue(local.promise);
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
      await waitFor(() => expect(result.current.notice).toMatch(/Could not list hosts \(boom\)/));
      // Local is still loading: no error claim meanwhile.
      expect(result.current.error).toBeNull();
      expect(result.current.data).toBeNull();

      await act(async () => local.resolve(resp('local', 3)));
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(3));
      expect(result.current.error).toBeNull();
      expect(result.current.notice).not.toBeNull();
    });

    it('is re-read on the 60s poll; when it answers, the notice clears and new hosts load', async () => {
      vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
      const reg = vi.spyOn(api, 'getRegisteredHosts').mockRejectedValueOnce(new Error('boom'));
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => Promise.resolve(resp(host ?? 'local', 1)));
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
      await until(() => expect(result.current.notice).not.toBeNull());
      await until(() => expect(result.current.data?.summary.runs).toBe(1));
      expect(usageCallsFor('host_prod')).toHaveLength(0);

      reg.mockResolvedValue([REG_LOCAL, REG_PROD]);
      vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
      await act(() => vi.advanceTimersByTimeAsync(USAGE_REMOTE_POLL_MS));
      await until(() => expect(result.current.notice).toBeNull());
      await until(() => expect(result.current.data?.summary.runs).toBe(2)); // local + prod
      expect(usageCallsFor('host_prod')).toHaveLength(1);
      expect(result.current.hosts.map((h) => h.host_id)).toEqual(['local', 'host_prod']);
      expect(reg).toHaveBeenCalledTimes(2);

      // The list is known now: later polls do not re-read it.
      await act(() => vi.advanceTimersByTimeAsync(USAGE_REMOTE_POLL_MS));
      expect(reg).toHaveBeenCalledTimes(2);
    });

    it('still reads as an error once every known host has failed', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockRejectedValue(new Error('boom'));
      vi.spyOn(api, 'getUsage').mockRejectedValue(new ApiError(502, 'x', '{"error":"down"}'));
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
      await waitFor(() => expect(result.current.error?.message).toMatch(/Local: down/));
      expect(result.current.notice).not.toBeNull();
    });
  });

  it('drops a remote host the CP no longer knows (404); local is never dropped', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
      host === 'local' ? Promise.resolve(resp('local', 1)) : Promise.reject(new ApiError(404, 'x', '{"error":"no such host"}')),
    );
    const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.hosts.map((h) => h.name)).toEqual(['Local']));
    expect(result.current.data?.summary.runs).toBe(1);
    expect(result.current.data?.excluded).toEqual([]);
    expect(result.current.error).toBeNull();
  });

  it('a 404 for local shows it offline instead of dropping it', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
    vi.spyOn(api, 'getUsage').mockRejectedValue(new ApiError(404, 'x', '{"error":"gone"}'));
    const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await waitFor(() => expect(result.current.hosts.map((h) => h.state)).toEqual(['offline']));
    expect(result.current.error?.message).toMatch(/Local: gone/);
  });

  it('the 60s poll and the focus refetch leave a still-pending request alone', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    const remoteSignals: AbortSignal[] = [];
    vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host, signal) => {
      if (host === 'local') return Promise.resolve(resp('local', 1));
      if (signal) remoteSignals.push(signal);
      return hangUntilAborted(signal);
    });
    renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
    await until(() => expect(remoteSignals).toHaveLength(1));

    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    await act(() => vi.advanceTimersByTimeAsync(USAGE_REMOTE_POLL_MS));
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await new Promise((r) => setTimeout(r, 10));
    expect(usageCallsFor('host_prod')).toHaveLength(1);
    expect(remoteSignals[0]?.aborted).toBe(false);
  });

  describe('a request that never answers', () => {
    /** Let promise callbacks run under the faked setTimeout (the real one is not available to wait on). */
    const settle = () => act(() => vi.advanceTimersByTimeAsync(0));

    it('is aborted and recorded as failed at FETCH_TIMEOUT_MS; settled requests keep their signal', async () => {
      vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      const signals: Record<string, AbortSignal | undefined> = {};
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host, signal) => {
        signals[host ?? 'local'] = signal;
        // The remote never settles, not even when aborted.
        return host === 'local' ? Promise.resolve(resp('local', 1)) : new Promise(() => {});
      });
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
      await settle();
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'loading']);

      await act(() => vi.advanceTimersByTimeAsync(FETCH_TIMEOUT_MS - 1));
      expect(signals.host_prod?.aborted).toBe(false);
      expect(result.current.hosts[1]?.state).toBe('loading');

      await act(() => vi.advanceTimersByTimeAsync(1));
      expect(signals.host_prod?.aborted).toBe(true);
      expect(result.current.hosts[1]?.state).toBe('offline');
      expect(result.current.hosts[1]?.reason).toBe('no answer after 45s');
      expect(result.current.data?.excluded).toEqual(['prod (offline)']);
      expect(result.current.error).toBeNull();
      // Local answered long ago: its timer was cleared with it, so its request is never aborted.
      expect(signals.local?.aborted).toBe(false);
    });

    it('keeps the last answer on a timeout (stale-on-error) and reads failed for the current window', async () => {
      vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      let remoteHangs = false;
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
        host !== 'local' && remoteHangs ? new Promise(() => {}) : Promise.resolve(resp(host ?? 'local', host === 'local' ? 1 : 10)),
      );
      const { result, rerender } = renderHook(({ key }) => useUsageData(WIN, key, 'user'), {
        initialProps: { key: 'preset:30d' },
      });
      await settle();
      expect(result.current.data?.summary.runs).toBe(11);

      remoteHangs = true;
      rerender({ key: 'preset:7d' });
      await settle();
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'loading']);

      await act(() => vi.advanceTimersByTimeAsync(FETCH_TIMEOUT_MS));
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'offline']);
      expect(result.current.hosts[1]?.reason).toBe('no answer after 45s');
      expect(result.current.data?.summary.runs).toBe(1);
      expect(result.current.data?.excluded).toEqual(['prod (stale)']);
    });
  });

  describe('a failure is remembered against the window it failed for', () => {
    const answer = (host: string) => Promise.resolve(resp(host, host === 'local' ? 1 : 10));
    const down = () => Promise.reject(new Error('down'));

    /** Every result a render produced, in order (a transient misreading would show up in one). */
    function mount(mode: { current: (host: string) => Promise<UsageResponse> }) {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) => mode.current(host ?? 'local'));
      const renders: UseUsageDataResult[] = [];
      const hook = renderHook(
        ({ key }) => {
          const r = useUsageData(WIN, key, 'user');
          renders.push(r);
          return r;
        },
        { initialProps: { key: 'preset:30d' } },
      );
      return { ...hook, renders };
    }
    const neverFailed = (renders: UseUsageDataResult[]) => {
      for (const r of renders) {
        expect(r.error).toBeNull();
        expect(r.hosts.map((h) => h.state)).not.toContain('offline');
      }
    };

    it("a failure for the old window is not read as the new window's, not even for one render", async () => {
      const mode = { current: answer };
      const { result, rerender, renders } = mount(mode);
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      // A focus refetch fails for the CURRENT window (30d): both hosts keep their answer + a reason.
      vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
      mode.current = down;
      await act(async () => {
        document.dispatchEvent(new Event('visibilitychange'));
      });
      await waitFor(() => expect(result.current.hosts.map((h) => h.reason)).toEqual(['down', 'down']));
      expect(result.current.hosts.map((h) => h.state)).toEqual(['ok', 'ok']); // still current: stale-on-error

      // Move the window; nothing has answered yet. No render may say a host failed for it.
      mode.current = () => new Promise(() => {});
      const before = renders.length;
      rerender({ key: 'preset:7d' });
      await flush();
      expect(renders.length).toBeGreaterThan(before);
      neverFailed(renders.slice(before));
      expect(result.current.hosts.map((h) => h.state)).toEqual(['loading', 'loading']);
    });

    it('a success clears the failure, so an older failing window is not read back as current', async () => {
      const mode = { current: answer };
      const { result, rerender, renders } = mount(mode);
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      mode.current = down; // fails FOR 7d
      rerender({ key: 'preset:7d' });
      await waitFor(() => expect(result.current.error).not.toBeNull());

      mode.current = answer; // answers 90d: clears the 7d failure
      rerender({ key: 'preset:90d' });
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(11));

      mode.current = () => new Promise(() => {});
      const before = renders.length;
      rerender({ key: 'preset:7d' }); // back to the window that once failed
      await flush();
      neverFailed(renders.slice(before));
      expect(result.current.hosts.map((h) => h.state)).toEqual(['loading', 'loading']);
    });
  });
  describe('customer filter', () => {
    it('passes the customer to every host, and keeps an unfiltered call as it was', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
      const spy = vi.spyOn(api, 'getUsage').mockResolvedValue(resp('local', 3));
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user', 'acme'));
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(3));
      expect(spy).toHaveBeenCalledWith(WIN, 'model', 'local', expect.any(AbortSignal), 'acme');
      cleanup();

      const plain = vi.spyOn(api, 'getUsage').mockClear();
      const { result: r2 } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user'));
      await waitFor(() => expect(r2.current.data?.summary.runs).toBe(3));
      expect(plain.mock.calls[0]).toHaveLength(4);
    });

    it('a remote host answering 501 under the filter is unavailable and not counted', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
        host === 'local'
          ? Promise.resolve(resp('local', 3))
          : Promise.reject(new ApiError(501, 'cannot filter', '')),
      );
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user', 'acme'));
      await waitFor(() => expect(result.current.hosts[1]?.state).toBe('unavailable'));
      expect(result.current.data?.summary.runs).toBe(3);
      expect(result.current.data?.excluded).toEqual(['prod (unavailable)']);
      expect(result.current.error).toBeNull();
    });

    it('changing the customer refetches every host and never mixes the old answer in', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
      const spy = vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, _h, _s, customer) =>
        Promise.resolve(resp('local', customer === 'globex' ? 9 : 3)),
      );
      const { result, rerender } = renderHook(
        ({ c }: { c: string }) => useUsageData(WIN, 'preset:30d', 'user', c),
        { initialProps: { c: 'acme' } },
      );
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(3));
      rerender({ c: 'globex' });
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(9));
      expect(spy).toHaveBeenLastCalledWith(WIN, 'model', 'local', expect.any(AbortSignal), 'globex');
    });

    it('unions the answers\' hosts_without_customer', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
      vi.spyOn(api, 'getUsage').mockImplementation((_w, _p, host) =>
        Promise.resolve({
          ...resp(host as string, 1),
          hosts_without_customer: host === 'local' ? ['worker-2', 'worker-1'] : ['worker-1'],
        }),
      );
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user', 'acme'));
      await waitFor(() => expect(result.current.data?.summary.runs).toBe(2));
      expect(result.current.data?.hostsWithoutCustomer).toEqual(['worker-1', 'worker-2']);
    });

    it('hands a scoped request\'s failure to onScopeRejected; an unscoped one never', async () => {
      vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
      const bad = new ApiError(400, 'x', '{"error":"customer: invalid slug"}');
      vi.spyOn(api, 'getUsage').mockRejectedValue(bad);
      const onRejected = vi.fn();
      const { result } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user', 'acme', onRejected));
      await waitFor(() => expect(result.current.error).not.toBeNull());
      expect(onRejected).toHaveBeenCalledWith(bad);
      cleanup();

      const unscoped = vi.fn();
      const { result: r2 } = renderHook(() => useUsageData(WIN, 'preset:30d', 'user', undefined, unscoped));
      await waitFor(() => expect(r2.current.error).not.toBeNull());
      expect(unscoped).not.toHaveBeenCalled();
    });
  });
});
