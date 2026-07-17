// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { coalesce, useDashboardData } from './useDashboardData';
import { api, type DashboardResponse, type DashboardSummary, type RegisteredHostView } from '../api';

// `getDashboard` resolves `DashboardResponse` on the wire (`DashboardSummary`
// flattened with `hosts` / `findings_partial` / `cycles_partial` — see
// api.ts). The hook only reads the `DashboardSummary` fields off it, but the
// mock's resolved type must still match the real signature.
function summary(overrides: Partial<DashboardSummary> = {}): DashboardResponse {
  return {
    active: { running: 0, awaiting_approval: 0, paused: 0, pending: 0 },
    active_longest: null,
    terminal_buckets: [],
    throughput_buckets: [],
    cycles: { total: 0, clean: 0, with_failures: 0 },
    findings_open: 0,
    captured_at: '2026-07-15T00:00:00Z',
    hosts: [],
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
      resolveSsh?.(summary({ active: { running: 2, awaiting_approval: 0, paused: 0, pending: 0 } }));
      await sshPromise;
    });

    await waitFor(() => expect(result.current.data?.active.running).toBe(3));
    expect(result.current.hosts.find((h) => h.hostId === 'ssh1')?.state).toBe('ok');
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
    const getDashboard = vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
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
    const getDashboard = vi.spyOn(api, 'getDashboard').mockResolvedValue(summary());
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
