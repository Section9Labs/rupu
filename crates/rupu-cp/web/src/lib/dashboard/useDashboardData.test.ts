import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { coalesce } from './useDashboardData';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('coalesce', () => {
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
