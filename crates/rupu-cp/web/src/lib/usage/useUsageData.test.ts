// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, getConfig, renderHook, waitFor } from '@testing-library/react';
import { api, type UsageResponse, type UsageWindow } from '../api';
import { USAGE_REMOTE_POLL_MS, useUsageData } from './useUsageData';
import { REG_LOCAL, REG_PROD, callsFor, flush } from '../perHost/testUtils';

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
});
