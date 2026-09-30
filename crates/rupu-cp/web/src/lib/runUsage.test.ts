// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor, act, cleanup } from '@testing-library/react';
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
    const b = mergeRunUsage(a, resp({ points_from: 2, points: [], summary: summary(3) }));
    expect(b.points.map((p) => p.turn)).toEqual([1, 2]);
    expect(b.summary.total_tokens).toBe(3);
  });
  it('returns the SAME object for an unchanged response (no re-render, no chart redraw)', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1), pt(2)], summary: summary(3), steps: { s: summary(3) }, turns: 2 }));
    // A quiet poll: same epoch, tail resumes at the end, nothing new, equal (but
    // freshly deserialized) summary/steps.
    const quiet = () =>
      resp({ points_from: 2, points: [], summary: summary(3), steps: { s: summary(3) }, turns: 2 });
    expect(mergeRunUsage(a, quiet())).toBe(a);
    // An empty run polling itself is equally unchanged.
    const empty = mergeRunUsage(null, resp({}));
    expect(mergeRunUsage(empty, resp({}))).toBe(empty);
  });
  it('returns a new object as soon as anything changes', () => {
    const a = mergeRunUsage(null, resp({ points: [pt(1)], summary: summary(3), turns: 1 }));
    const base = { points_from: 1, points: [], summary: summary(3), turns: 1 };
    expect(mergeRunUsage(a, resp({ ...base, summary: summary(4) }))).not.toBe(a);
    expect(mergeRunUsage(a, resp({ ...base, turns: 2 }))).not.toBe(a);
    expect(mergeRunUsage(a, resp({ ...base, partial: true }))).not.toBe(a);
    expect(mergeRunUsage(a, resp({ ...base, steps: { s: summary(3) } }))).not.toBe(a);
    expect(mergeRunUsage(a, resp({ ...base, points: [pt(2)] }))).not.toBe(a);
    expect(mergeRunUsage(a, resp({ ...base, epoch: '8' }))).not.toBe(a);
  });
});

describe('useRunUsage', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });
  afterEach(() => {
    // Unmount first: a still-mounted hook keeps its interval + visibility
    // listener alive and would poll the NEXT test's spy.
    cleanup();
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

  it('reports unavailable (no throw) on a 404 once the run is known, and stops polling', async () => {
    const spy = vi.spyOn(api, 'getRunUsage').mockRejectedValue(new ApiError(404, 'not found'));
    const { result } = renderHook(() =>
      useRunUsage('run_1', 'old-host', true, { intervalMs: 50, runKnown: true }),
    );
    await waitFor(() => expect(result.current.unavailable).toBe(true));
    expect(result.current.usage).toBeNull();
    const calls = spy.mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(500);
    });
    expect(spy.mock.calls.length).toBe(calls);
  });

  it('treats a 404 as transient while the run is not known yet: keeps polling, then populates', async () => {
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockRejectedValueOnce(new ApiError(404, 'not found'))
      .mockRejectedValueOnce(new ApiError(404, 'not found'))
      .mockResolvedValueOnce(resp({ points: [pt(1)], summary: summary(5) }))
      .mockResolvedValue(resp({ points_from: 1, points: [pt(2)], summary: summary(9) }));
    const { result } = renderHook(() =>
      useRunUsage('run_1', undefined, true, { intervalMs: 50, runKnown: false }),
    );
    // Two 404s: no `unavailable`, no gone-state.
    await waitFor(() => expect(spy.mock.calls.length).toBeGreaterThanOrEqual(2));
    expect(result.current.unavailable).toBe(false);
    // The third poll lands and the fourth resumes from its tail: polling went on.
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(5));
    expect(result.current.unavailable).toBe(false);
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(9));
    // ...and it resumed from the tail (since/epoch), not from scratch.
    expect(spy).toHaveBeenCalledWith('run_1', { host: undefined, since: 1, epoch: '7' });
  });

  it('a 404 that was already in flight when the run became known is still transient, and retries at once', async () => {
    let reject404: (() => void) | undefined;
    const first = new Promise<never>((_, rej) => {
      reject404 = () => rej(new ApiError(404, 'not found'));
    });
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockImplementationOnce(() => first)
      .mockResolvedValue(resp({ points: [pt(1)], summary: summary(7) }));
    // intervalMs is huge: only the immediate retry can produce the second call.
    const { result, rerender } = renderHook(
      ({ known }) => useRunUsage('run_1', undefined, true, { intervalMs: 600_000, runKnown: known }),
      { initialProps: { known: false } },
    );
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    rerender({ known: true }); // e.g. the graph just loaded
    await act(async () => {
      reject404?.();
    });
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(7));
    expect(result.current.unavailable).toBe(false);
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it('a 404 after the run is known clears when the id changes (new run polls afresh)', async () => {
    const spy = vi.spyOn(api, 'getRunUsage').mockImplementation(async (id) => {
      if (id === 'run_1') throw new ApiError(404, 'not found');
      return resp({ points: [pt(1)], summary: summary(11) });
    });
    const { result, rerender } = renderHook(
      ({ id }) => useRunUsage(id, undefined, true, { intervalMs: 50 }),
      { initialProps: { id: 'run_1' } },
    );
    await waitFor(() => expect(result.current.unavailable).toBe(true));
    rerender({ id: 'run_2' });
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(11));
    expect(result.current.unavailable).toBe(false);
    expect(spy).toHaveBeenCalledWith('run_2', { host: undefined });
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

  it('does not refetch when a run that just loaded goes live (page-load dedupe)', async () => {
    const spy = vi.spyOn(api, 'getRunUsage').mockResolvedValue(resp({ points: [pt(1)] }));
    const { result, rerender } = renderHook(
      ({ live, known }) => useRunUsage('run_1', undefined, live, { intervalMs: 10_000, runKnown: known }),
      { initialProps: { live: false, known: false } },
    );
    await waitFor(() => expect(result.current.usage?.points).toHaveLength(1));
    // The graph resolves as `running`: live and runKnown flip together.
    rerender({ live: true, known: true });
    await act(async () => {
      vi.advanceTimersByTime(100);
    });
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('still fetches on going terminal even right after a poll (final numbers are newer)', async () => {
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockResolvedValueOnce(resp({ points: [pt(1)], summary: summary(1) }))
      .mockResolvedValue(resp({ points_from: 1, points: [pt(2)], summary: summary(2) }));
    const { result, rerender } = renderHook(
      ({ live }) => useRunUsage('run_1', undefined, live, { intervalMs: 10_000 }),
      { initialProps: { live: true } },
    );
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(1));
    rerender({ live: false });
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(2));
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it('a queued final fetch belongs to its run: switching runs mid-flight does not leak it', async () => {
    let release: (() => void) | undefined;
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const spy = vi
      .spyOn(api, 'getRunUsage')
      .mockImplementationOnce(async () => {
        await gate;
        return resp({ points: [pt(1)] });
      })
      .mockResolvedValue(resp({ points: [pt(9)], epoch: 'b' }));
    const { result, rerender } = renderHook(
      ({ id, live }) => useRunUsage(id, undefined, live, { intervalMs: 600_000 }),
      { initialProps: { id: 'run_a', live: true } },
    );
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    rerender({ id: 'run_a', live: false }); // queues run_a's final fetch
    rerender({ id: 'run_b', live: false }); // ...but the user moved on
    await waitFor(() => expect(result.current.usage?.epoch).toBe('b'));
    await act(async () => {
      release?.();
    });
    await act(async () => {
      vi.advanceTimersByTime(100);
    });
    // run_a once (discarded), run_b once — no phantom re-run of run_a's queue.
    expect(spy).toHaveBeenCalledTimes(2);
    expect(result.current.usage?.epoch).toBe('b');
  });

  it('does not re-render (same usage object) when a poll changes nothing', async () => {
    vi.spyOn(api, 'getRunUsage')
      .mockResolvedValueOnce(resp({ points: [pt(1)], summary: summary(3) }))
      .mockResolvedValue(resp({ points_from: 1, points: [], summary: summary(3) }));
    let renders = 0;
    const { result } = renderHook(() => {
      renders += 1;
      return useRunUsage('run_1', undefined, true, { intervalMs: 20 });
    });
    await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(3));
    const first = result.current.usage;
    const afterFirst = renders;
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(result.current.usage).toBe(first);
    expect(renders).toBe(afterFirst);
  });

  describe('hidden tab', () => {
    const setVisibility = (v: 'hidden' | 'visible') => {
      Object.defineProperty(document, 'visibilityState', { value: v, configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    };
    afterEach(() => {
      // Drop the own-property override so jsdom's prototype getter is back.
      delete (document as unknown as { visibilityState?: string }).visibilityState;
    });

    it('sends no request while hidden, then exactly one when visible again', async () => {
      const spy = vi
        .spyOn(api, 'getRunUsage')
        .mockResolvedValueOnce(resp({ points: [pt(1)] }))
        .mockResolvedValue(resp({ points_from: 1, points: [pt(2)] }));
      const { result } = renderHook(() => useRunUsage('run_1', undefined, true, { intervalMs: 50 }));
      await waitFor(() => expect(result.current.usage?.points).toHaveLength(1));
      setVisibility('hidden');
      const before = spy.mock.calls.length;
      await act(async () => {
        vi.advanceTimersByTime(500); // ten interval ticks' worth
      });
      expect(spy.mock.calls.length).toBe(before);
      setVisibility('visible');
      // The fetch goes out synchronously on the visibilitychange.
      expect(spy.mock.calls.length).toBe(before + 1);
      await waitFor(() => expect(result.current.usage?.points).toHaveLength(2));
    });

    it('refetches on becoming visible even when the run went terminal while hidden', async () => {
      const spy = vi
        .spyOn(api, 'getRunUsage')
        .mockResolvedValueOnce(resp({ points: [pt(1)], summary: summary(1) }))
        .mockResolvedValue(resp({ points_from: 1, points: [pt(2)], summary: summary(2) }));
      const { result, rerender } = renderHook(
        ({ live }) => useRunUsage('run_1', undefined, live, { intervalMs: 600_000 }),
        { initialProps: { live: true } },
      );
      await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(1));
      setVisibility('hidden');
      rerender({ live: false }); // terminal, but the final fetch is skipped: hidden
      expect(spy).toHaveBeenCalledTimes(1);
      setVisibility('visible');
      await waitFor(() => expect(result.current.usage?.summary.total_tokens).toBe(2));
      expect(spy).toHaveBeenCalledTimes(2);
    });
  });
});
