// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { coalesce, useDashboardData } from './useDashboardData';
import { api, type DashboardResponse, type RegisteredHostView } from '../api';

// `getDashboard` resolves `DashboardResponse` on the wire (`DashboardSummary`
// flattened with `hosts` / `findings_partial` / `cycles_partial` — see
// api.ts). The hook reads BOTH: the flattened `DashboardSummary` fields, and
// (since the fetchOneHost fix) `resp.hosts` to find ITS OWN per-host entry
// and honor its authoritative `state`. So the default here seeds a matching
// `hosts: [{ host_id: hostId, state: 'ok', ... }]` entry — the healthy case
// — unless a test overrides `hosts` itself to simulate a down host.
function summary(overrides: Partial<DashboardResponse> = {}, hostId = 'local'): DashboardResponse {
  const captured_at = overrides.captured_at ?? '2026-07-15T00:00:00Z';
  return {
    active: { running: 0, awaiting_approval: 0, paused: 0, pending: 0 },
    active_longest: null,
    terminal_buckets: [],
    throughput_buckets: [],
    cycles: { total: 0, clean: 0, with_failures: 0 },
    findings_open: 0,
    captured_at,
    hosts: [{ host_id: hostId, name: hostId, transport_kind: 'local', state: 'ok', captured_at, reason: null }],
    findings_partial: false,
    cycles_partial: false,
    ...overrides,
  };
}

const LOCAL_HOST: RegisteredHostView = { id: 'local', name: 'Local', transport_kind: 'local' };
const SSH_HOST: RegisteredHostView = { id: 'ssh1', name: 'staging-box', transport_kind: 'ssh' };

describe('coalesce', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('collapses a burst into ONE call', () => {
    const fn = vi.fn();
    const { trigger } = coalesce(fn, 250);
    // An autoflow cycle firing 12 runs.
    for (let i = 0; i < 12; i++) trigger();
    vi.advanceTimersByTime(250);
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it('allows a second call after the window elapses', () => {
    const fn = vi.fn();
    const { trigger } = coalesce(fn, 250);
    trigger();
    vi.advanceTimersByTime(250);
    trigger();
    vi.advanceTimersByTime(250);
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it('does not fire before the window elapses', () => {
    const fn = vi.fn();
    const { trigger } = coalesce(fn, 250);
    trigger();
    vi.advanceTimersByTime(100);
    expect(fn).not.toHaveBeenCalled();
  });

  it('cancel prevents a pending call', () => {
    const fn = vi.fn();
    const { trigger, cancel } = coalesce(fn, 250);
    trigger();
    cancel();
    vi.advanceTimersByTime(500);
    expect(fn).not.toHaveBeenCalled();
  });
});

describe('useDashboardData', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('seeds a loading slice per registered host immediately, before any getDashboard call resolves', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    vi.spyOn(api, 'getDashboard').mockReturnValue(new Promise<DashboardResponse>(() => {})); // never resolves
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    const { result } = renderHook(() => useDashboardData('30d'));

    // The strip renders from the host list alone — `loading` must flip to
    // false as soon as `getRegisteredHosts` resolves, without waiting on any
    // `getDashboard` call.
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.hosts).toHaveLength(2);
    expect(result.current.hosts.every((h) => h.state === 'loading')).toBe(true);
    expect(result.current.data).toBeNull();
  });

  // THE critical test: a hung remote host must never delay local's paint.
  it('a hung remote host does not block local — local resolves and renders while the hung host stays loading', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    const localSummary = summary({ active: { running: 1, awaiting_approval: 0, paused: 0, pending: 0 } });
    vi.spyOn(api, 'getDashboard').mockImplementation((_range, host) => {
      if (host === 'local') return Promise.resolve(localSummary);
      // ssh1: a hung connector — a promise that NEVER settles, standing in
      // for an unreachable SSH host the real fetch would otherwise block on.
      return new Promise<DashboardResponse>(() => {});
    });
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    const { result } = renderHook(() => useDashboardData('30d'));

    await waitFor(() => expect(result.current.data).not.toBeNull());

    expect(result.current.data?.active.running).toBe(1);
    const local = result.current.hosts.find((h) => h.hostId === 'local');
    expect(local?.state).toBe('ok');
    const ssh = result.current.hosts.find((h) => h.hostId === 'ssh1');
    expect(ssh?.state).toBe('loading');
  });

  it('recomputes the merged view as each additional host resolves', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    let resolveSsh: ((s: DashboardResponse) => void) | null = null;
    const sshPromise = new Promise<DashboardResponse>((resolve) => {
      resolveSsh = resolve;
    });
    vi.spyOn(api, 'getDashboard').mockImplementation((_range, host) => {
      if (host === 'local') {
        return Promise.resolve(summary({ active: { running: 1, awaiting_approval: 0, paused: 0, pending: 0 } }));
      }
      return sshPromise;
    });
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    const { result } = renderHook(() => useDashboardData('30d'));

    await waitFor(() => expect(result.current.data?.active.running).toBe(1));

    await act(async () => {
      resolveSsh?.(summary({ active: { running: 2, awaiting_approval: 0, paused: 0, pending: 0 } }, 'ssh1'));
      await sshPromise;
    });

    await waitFor(() => expect(result.current.data?.active.running).toBe(3));
    expect(result.current.hosts.find((h) => h.hostId === 'ssh1')?.state).toBe('ok');
  });

  // THE bug this task fixes: a down remote host does NOT make `getDashboard`
  // reject — the server answers with HTTP 200 and a zeroed summary whose own
  // `resp.hosts[]` entry says `unavailable`. The old success handler ignored
  // `resp.hosts` entirely and always set `state: 'ok'`, which (a) painted a
  // powered-off host as "live" in the freshness strip (its zeroed summary's
  // `captured_at` looks fresh) and (b) let its `findings_open: null` /
  // `cycles: {clean: null, with_failures: null}` leak into the merge and trip
  // `findings_partial`/`cycles_partial` for a host that isn't even reporting.
  it('a host that RESOLVES with 200 but reports itself down in resp.hosts is marked down, not ok', async () => {
    const MINI_HOST: RegisteredHostView = { id: 'mini', name: 'mini-box', transport_kind: 'ssh' };
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, MINI_HOST]);

    // local: a genuinely healthy host with a real findings count.
    const localSummary = summary({ findings_open: 7 });

    // mini: HTTP 200, but its own `hosts[]` entry says it's down. Zeroed
    // summary, `null` findings/cycles breakdown, and a `captured_at` that is
    // the SERVER's `now` — NOT proof this host is alive.
    const miniSummary: DashboardResponse = {
      active: { running: 0, awaiting_approval: 0, paused: 0, pending: 0 },
      active_longest: null,
      terminal_buckets: [],
      throughput_buckets: [],
      cycles: { total: 0, clean: null, with_failures: null },
      findings_open: null,
      captured_at: '2026-07-16T12:00:00Z',
      hosts: [
        {
          host_id: 'mini',
          name: 'mini-box',
          transport_kind: 'ssh',
          state: 'unavailable',
          captured_at: null,
          reason: 'connection refused',
        },
      ],
      findings_partial: false,
      cycles_partial: false,
    };

    vi.spyOn(api, 'getDashboard').mockImplementation((_range, host) => {
      if (host === 'local') return Promise.resolve(localSummary);
      return Promise.resolve(miniSummary);
    });
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    const { result } = renderHook(() => useDashboardData('30d'));

    await waitFor(() => expect(result.current.data).not.toBeNull());
    await waitFor(() => {
      const mini = result.current.hosts.find((h) => h.hostId === 'mini');
      expect(mini?.state).not.toBe('loading');
    });

    const mini = result.current.hosts.find((h) => h.hostId === 'mini');
    // The server's per-host state is authoritative — NOT 'ok' just because
    // the promise resolved.
    expect(mini?.state).toBe('unavailable');
    expect(mini?.reason).toBe('connection refused');
    // Its zeroed/null summary must never be stored — it must never re-enter
    // the merge, the freshness strip, or the partial-flag derivation.
    expect(mini?.summary).toBeUndefined();

    const local = result.current.hosts.find((h) => h.hostId === 'local');
    expect(local?.state).toBe('ok');

    // The merged view reflects ONLY the genuinely-ok host: local's real
    // findings count survives, and mini's `null` contribution does not trip
    // `findings_partial`/`cycles_partial` — those flags mean "a REPORTING
    // host omitted the field," not "some host in the fleet is dead."
    expect(result.current.data?.findings_open).toBe(7);
    expect(result.current.data?.findings_partial).toBe(false);
    expect(result.current.data?.cycles_partial).toBe(false);
  });

  it('stale-on-error: a host that already has data keeps it (and its state stays ok) when a later refresh fails', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
    const getDashboard = vi
      .spyOn(api, 'getDashboard')
      .mockResolvedValueOnce(summary({ active: { running: 1, awaiting_approval: 0, paused: 0, pending: 0 } }))
      .mockRejectedValueOnce(new Error('boom'));
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});

    const { result } = renderHook(() => useDashboardData('30d'));

    await waitFor(() => expect(result.current.data?.active.running).toBe(1));

    await act(async () => {
      result.current.refresh();
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(result.current.error).not.toBeNull());
    // Stale data survives the failed refresh — state stays 'ok', the old
    // summary is untouched, and the merged view still reflects it.
    expect(result.current.hosts.find((h) => h.hostId === 'local')?.state).toBe('ok');
    expect(result.current.data?.active.running).toBe(1);
    expect(getDashboard).toHaveBeenCalledTimes(2);
  });

  it('SSE arrival triggers a debounced refetch of ?host=local only, never a remote host', async () => {
    vi.useFakeTimers();
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    const getDashboard = vi
      .spyOn(api, 'getDashboard')
      .mockImplementation((_range, host) => Promise.resolve(summary({}, host)));
    let emit: (() => void) | null = null;
    vi.spyOn(api, 'subscribeEvents').mockImplementation((onEvent) => {
      emit = () => onEvent({ type: 'run_started', run_id: 'r1' } as never);
      return () => {};
    });

    await act(async () => {
      renderHook(() => useDashboardData('30d'));
      await vi.advanceTimersByTimeAsync(0);
    });

    const callsAfterBootstrap = getDashboard.mock.calls.length;
    expect(callsAfterBootstrap).toBe(2); // local + ssh1, once each

    await act(async () => {
      emit?.();
      emit?.();
      emit?.(); // a burst — must still cost only ONE refetch
      await vi.advanceTimersByTimeAsync(250);
    });

    const localCalls = getDashboard.mock.calls.filter(([, host]) => host === 'local');
    const sshCalls = getDashboard.mock.calls.filter(([, host]) => host === 'ssh1');
    expect(localCalls).toHaveLength(2); // bootstrap + the one debounced SSE refetch
    expect(sshCalls).toHaveLength(1); // untouched — no cross-host firehose
  });

  it('the visibility-gated reconciling poll refetches every host', async () => {
    vi.useFakeTimers();
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST, SSH_HOST]);
    const getDashboard = vi
      .spyOn(api, 'getDashboard')
      .mockImplementation((_range, host) => Promise.resolve(summary({}, host)));
    vi.spyOn(api, 'subscribeEvents').mockReturnValue(() => {});
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });

    await act(async () => {
      renderHook(() => useDashboardData('30d'));
      await vi.advanceTimersByTimeAsync(0);
    });

    const callsAfterBootstrap = getDashboard.mock.calls.length;
    expect(callsAfterBootstrap).toBe(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    expect(getDashboard.mock.calls.length).toBe(callsAfterBootstrap * 2);
  });
});
