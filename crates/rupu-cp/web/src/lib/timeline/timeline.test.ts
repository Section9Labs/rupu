// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { REDUCED_STEP_MS, reducedSteps, replayDuration, revealed, timeBounds, type TimelineItem } from './timeline';
import { useTimeline } from './useTimeline';

const items: TimelineItem[] = [
  { key: 'a', at: 1000, kind: 'round' },
  { key: 'b', at: 5000, kind: 'child' },
  { key: 'c', at: 9000, kind: 'message' },
];

describe('timeline', () => {
  it('revealed_is_everything_at_null_and_by_time_otherwise', () => {
    expect([...revealed(items, null)].sort()).toEqual(['a', 'b', 'c']);
    expect([...revealed(items, 5000)].sort()).toEqual(['a', 'b']);
    expect([...revealed(items, 999)]).toEqual([]);
  });

  it('bounds_cover_items_bands_and_now', () => {
    const bands = [{ key: 'r1', label: 'Round 1', from: 500, to: null }];
    expect(timeBounds(items, bands, 12000)).toEqual({ start: 500, end: 12000 });
    expect(timeBounds(items, [], null)).toEqual({ start: 1000, end: 9000 });
    expect(timeBounds([], [], null)).toEqual({ start: 0, end: 0 });
  });

  it('bounds_survive_a_huge_log', () => {
    const big = Array.from({ length: 100_000 }, (_, k) => ({ key: `k${k}`, at: k, kind: 'message' as const }));
    expect(timeBounds(big, [], null)).toEqual({ start: 0, end: 99_999 });
  });

  it('reduced_steps_are_the_start_then_what_happened_at_most_max', () => {
    expect(reducedSteps([1000, 5000, 9000, 5000], 1000, 9000)).toEqual([1000, 5000, 9000]);
    expect(reducedSteps([], 1000, 9000)).toEqual([1000]);
    const many = Array.from({ length: 100 }, (_, i) => 1000 + (i + 1) * 10);
    const steps = reducedSteps(many, 1000, 5000, 12);
    expect(steps).toHaveLength(12);
    expect(steps[0]).toBe(1000);
    expect(steps[steps.length - 1]).toBe(2000);
    expect([...steps].sort((a, b) => a - b)).toEqual(steps);
  });

  it('replay_duration_is_clamped', () => {
    expect(replayDuration(0, 60_000)).toBe(6000);
    expect(replayDuration(0, 5 * 3600 * 1000)).toBe(16000);
    expect(replayDuration(0, 200_000)).toBe(8000);
  });
});

describe('useTimeline', () => {
  let now = 0;
  let cbs: FrameRequestCallback[] = [];
  const frame = (advance: number) => {
    now += advance;
    const run = cbs;
    cbs = [];
    act(() => run.forEach((cb) => cb(now)));
  };
  beforeEach(() => {
    now = 0;
    cbs = [];
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => cbs.push(cb));
    vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(() => {});
    vi.spyOn(performance, 'now').mockImplementation(() => now);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    delete (window as { matchMedia?: unknown }).matchMedia;
  });

  const props = { items, bands: [], nowMs: null };

  it('replay_runs_to_the_end_and_releases', () => {
    const { result } = renderHook(() => useTimeline(props));
    expect(result.current.t).toBeNull();
    act(() => result.current.replay());
    expect(result.current.playing).toBe(true);
    expect(result.current.t).toBe(1000);
    frame(100);
    expect(result.current.t).toBeGreaterThan(1000);
    frame(100000);
    expect(result.current.t).toBeNull();
    expect(result.current.playing).toBe(false);
  });

  it('a_new_item_keeps_t', () => {
    const { result, rerender } = renderHook((p) => useTimeline(p), { initialProps: props });
    act(() => result.current.replay());
    frame(1000);
    const t = result.current.t;
    expect(t).not.toBeNull();
    rerender({ ...props, items: [...items, { key: 'd', at: 9500, kind: 'message' }] });
    expect(result.current.t).toBe(t);
    expect(result.current.playing).toBe(true);
  });

  it('unmount_mid_replay_cancels_the_frame', () => {
    const { result, unmount } = renderHook(() => useTimeline(props));
    act(() => result.current.replay());
    (window.cancelAnimationFrame as ReturnType<typeof vi.fn>).mockClear();
    unmount();
    expect(window.cancelAnimationFrame).toHaveBeenCalled();
  });

  it('a_new_item_mid_replay_still_reaches_the_end', () => {
    const { result, rerender } = renderHook((p) => useTimeline(p), { initialProps: props });
    act(() => result.current.replay());
    frame(1000);
    rerender({ ...props, items: [...items, { key: 'd', at: 9500, kind: 'message' }] });
    frame(1000);
    expect(result.current.playing).toBe(true);
    frame(100000);
    expect(result.current.t).toBeNull();
    expect(result.current.playing).toBe(false);
  });

  it('scrub_sets_t_and_release_clears_it', () => {
    const { result } = renderHook(() => useTimeline(props));
    act(() => result.current.scrub(4000));
    expect(result.current.t).toBe(4000);
    act(() => result.current.release());
    expect(result.current.t).toBeNull();
  });

  it('reduced_motion_replay_steps_through_the_run', () => {
    vi.useFakeTimers();
    try {
      window.matchMedia = ((q: string) => ({ matches: q.includes('reduce') })) as unknown as typeof window.matchMedia;
      const { result } = renderHook(() => useTimeline(props));
      act(() => result.current.replay());
      // No sweep (no animation frame), but it plays: from the start, one
      // step at a time through what happened, then back to the whole run.
      expect(cbs).toHaveLength(0);
      expect(result.current.playing).toBe(true);
      const seen = [result.current.t];
      for (let i = 0; i < 20 && result.current.t !== null; i++) {
        act(() => vi.advanceTimersByTime(REDUCED_STEP_MS));
        seen.push(result.current.t);
      }
      const { start, end } = timeBounds(items, [], null);
      expect(seen).toEqual([...reducedSteps(items.map((i) => i.at), start, end), null]);
      expect(seen.length).toBeGreaterThan(2);
      expect(result.current.playing).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });
});
