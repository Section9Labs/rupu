// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { mergeRunUsage, useRunUsage } from './runUsage';
import { api, ApiError } from './api';

const pt = (turn: number) => ({ turn, label: 's', tokens_in: turn, tokens_out: 1, tokens_cached: 0 });
const summary = (total: number) => ({
  input_tokens: total,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: total,
  cost_usd: null,
  priced: true,
  runs: 1,
});
const resp = (over: Partial<Parameters<typeof mergeRunUsage>[1]>) => ({
  summary: summary(0),
  steps: {},
  turns: 0,
  partial: false,
  epoch: '7',
  points_from: 0,
  points: [],
  ...over,
});

describe('mergeRunUsage', () => {
  it('appends points when epoch matches and points_from continues the series', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2)] }));
    const b = mergeRunUsage(a, resp({ points_from: 2, points: [pt(3)], summary: summary(9) }));
    expect(b.points.map((p) => p.turn)).toEqual([1, 2, 3]);
    expect(b.summary.total_tokens).toBe(9);
  });
  it('replaces the series on epoch change or points_from 0', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2)] }));
    const b = mergeRunUsage(a, resp({ epoch: '8', points: [pt(1)] }));
    expect(b.points).toHaveLength(1);
    expect(b.epoch).toBe('8');
    const c = mergeRunUsage(b, resp({ epoch: '8', points_from: 0, points: [pt(1), pt(2)] }));
    expect(c.points.map((p) => p.turn)).toEqual([1, 2]);
  });
  it('truncates to points_from before appending (server resent a tail)', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2), pt(3)] }));
    const b = mergeRunUsage(a, resp({ points_from: 2, points: [pt(3), pt(4)] }));
    expect(b.points.map((p) => p.turn)).toEqual([1, 2, 3, 4]);
  });
  it('replaces the series when points_from points past what the client holds', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1)] }));
    const b = mergeRunUsage(a, resp({ points_from: 5, points: [pt(6)] }));
    expect(b.points.map((p) => p.turn)).toEqual([6]);
  });
  it('keeps the series (no new points) on an empty same-epoch tail', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2)] }));
    const b = mergeRunUsage(a, resp({ points_from: 2, points: [] }));
    expect(b.points.map((p) => p.turn)).toEqual([1, 2]);
  });
});

describe('useRunUsage', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('polls while live, sends since/epoch, stops when not live', async () => {
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockResolvedValueOnce(resp({ points: [pt(1)] }))
      .mockResolvedValue(resp({ points_from: 1, points: [pt(2)] }));
    const { result, rerender } = renderHook(
      ({ live }) => useRunUsage('run_1', undefined, live, { intervalMs: 50 }),
      { initialProps: { live: true } },
    );
    await waitFor(() => expect(result.current.usage?.points).toHaveLength(1));
    await act(async () => {
      vi.advanceTimersByTime(60);
    });
    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith('run_1', { host: undefined, since: 1, epoch: '7' }),
    );
    const calls = spy.mock.calls.length;
    rerender({ live: false });
    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    // exactly one final fetch after going terminal, then silence
    expect(spy.mock.calls.length).toBeLessThanOrEqual(calls + 1);
    const settled = spy.mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    expect(spy.mock.calls.length).toBe(settled);
  });

  it('does a single fetch (no polling) for a run that is not live', async () => {
    const spy = vi.spyOn(api, 'getRunUsage').mockResolvedValue(resp({ points: [pt(1)] }));
    const { result } = renderHook(() => useRunUsage('run_1', undefined, false, { intervalMs: 50 }));
    await waitFor(() => expect(result.current.usage?.points).toHaveLength(1));
    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('still does the final fetch when the run turns terminal mid-poll', async () => {
    let release: (() => void) | undefined;
    const first = new Promise<void>((r) => {
      release = r;
    });
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockImplementationOnce(async () => {
        await first;
        return resp({ points: [pt(1)], summary: summary(1) });
      })
      .mockResolvedValue(resp({ points_from: 1, points: [pt(2)], summary: summary(2) }));
    const { result, rerender } = renderHook(
      ({ live }) => useRunUsage('run_1', undefined, live, { intervalMs: 50 }),
      { initialProps: { live: true } },
    );
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    // Run goes terminal while the first poll is still in flight.
    rerender({ live: false });
    await act(async () => {
      release?.();
    });
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(2));
  });

  it('resets and refetches when the run id changes', async () => {
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockImplementation(async (id) => resp({ points: [pt(id === 'run_1' ? 1 : 9)], epoch: id }));
    const { result, rerender } = renderHook(
      ({ id }) => useRunUsage(id, undefined, false, { intervalMs: 50 }),
      { initialProps: { id: 'run_1' } },
    );
    await waitFor(() => expect(result.current.usage?.epoch).toBe('run_1'));
    rerender({ id: 'run_2' });
    await waitFor(() => expect(result.current.usage?.epoch).toBe('run_2'));
    // The new run's first request carries no since/epoch from the old one.
    expect(spy).toHaveBeenLastCalledWith('run_2', { host: undefined });
  });

  it('reports unavailable (no throw) when the endpoint 404s, and stops polling', async () => {
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockRejectedValue(new ApiError(404, 'not found'));
    const { result } = renderHook(() => useRunUsage('run_1', 'old-host', true, { intervalMs: 50 }));
    await waitFor(() => expect(result.current.unavailable).toBe(true));
    expect(result.current.usage).toBeNull();
    const calls = spy.mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    expect(spy.mock.calls.length).toBe(calls);
  });

  it('keeps the last good usage across a transient poll error', async () => {
    vi.spyOn(api, 'getRunUsage')
      .mockResolvedValueOnce(resp({ points: [pt(1)], summary: summary(5) }))
      .mockRejectedValue(new ApiError(502, 'bad gateway'));
    const { result } = renderHook(() => useRunUsage('run_1', undefined, true, { intervalMs: 50 }));
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(5));
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(result.current.usage?.summary.total_tokens).toBe(5);
    expect(result.current.unavailable).toBe(false);
  });
});
