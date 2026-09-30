// @vitest-environment jsdom
//
// Usage page live refresh: a preset window ends at "now", so every 30s the
// page re-derives it and refetches usage / outliers / run rows. A drag-selected
// custom window is a fixed historical span — it never ticks. Hidden tabs skip
// the tick, and unmount clears the timer.
//
// Only `setInterval`/`clearInterval`/`Date` are faked (not promises), so the
// mocked API replies resolve on the real microtask queue. The chart is mocked
// with a stub exposing a button that calls `onSelectRange(...)` — a real
// Recharts pixel-drag isn't reproducible in jsdom (same approach as
// `Usage.selectRange.test.tsx`).
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, presetWindow, windowFromDayRange, type UsageResponse } from '../lib/api';

vi.mock('../components/dashboard/UsageTimelineStacked', () => ({
  default: (props: { onSelectRange?: (startDay: string, endDay: string) => void }) => (
    <button onClick={() => props.onSelectRange?.('2026-07-10', '2026-07-12')}>trigger-select</button>
  ),
}));

import Usage from './Usage';

const FIXED_NOW = new Date('2026-07-16T12:00:00.000Z').getTime();
const TICK = 30_000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
  vi.setSystemTime(FIXED_NOW);
  vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
  vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
  vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function usageResponse(): UsageResponse {
  return {
    summary: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: 0, priced: true, runs: 0 },
    breakdown: [],
    unpriced: { models: [], rows: 0 },
    hosts: [],
  };
}

function renderUsage() {
  return render(
    <MemoryRouter>
      <Usage />
    </MemoryRouter>,
  );
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe('Usage page — 30s live refresh', () => {
  it('refetches usage, outliers and run rows every 30s with a fresh "now" window when the window ends now', async () => {
    renderUsage();
    await flush();
    expect(api.getUsage).toHaveBeenCalledTimes(1);
    expect(api.getUsage).toHaveBeenLastCalledWith(presetWindow('30d', FIXED_NOW), 'model');

    await advance(TICK);
    await flush();

    // The window advanced to the tick's "now" — a stale `until` would refetch
    // the same span and never see anything newer.
    const next = presetWindow('30d', FIXED_NOW + TICK);
    expect(api.getUsage).toHaveBeenCalledTimes(2);
    expect(api.getUsage).toHaveBeenLastCalledWith(next, 'model');
    expect(api.getUsageOutliers).toHaveBeenLastCalledWith(next);
    expect(api.getUsageRuns).toHaveBeenLastCalledWith(next);

    await advance(TICK);
    await flush();
    expect(api.getUsage).toHaveBeenCalledTimes(3);
  });

  it('does not refetch when the window is a fixed (drag-selected) span', async () => {
    renderUsage();
    await flush();

    fireEvent.click(screen.getByText('trigger-select'));
    await flush();
    const custom = windowFromDayRange('2026-07-10', '2026-07-12');
    expect(api.getUsage).toHaveBeenLastCalledWith(custom, 'model');
    const calls = vi.mocked(api.getUsage).mock.calls.length;

    await advance(TICK * 3);
    await flush();

    expect(api.getUsage).toHaveBeenCalledTimes(calls);
    expect(api.getUsageOutliers).toHaveBeenLastCalledWith(custom);
  });

  it('resumes ticking once the custom window is cleared', async () => {
    renderUsage();
    await flush();
    fireEvent.click(screen.getByText('trigger-select'));
    await flush();

    fireEvent.click(screen.getByRole('button', { name: /clear custom|×/i }));
    await flush();
    const calls = vi.mocked(api.getUsage).mock.calls.length;

    await advance(TICK);
    await flush();
    expect(vi.mocked(api.getUsage).mock.calls.length).toBeGreaterThan(calls);
  });

  it('skips the tick while the tab is hidden', async () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    renderUsage();
    await flush();
    const calls = vi.mocked(api.getUsage).mock.calls.length;

    await advance(TICK * 2);
    await flush();
    expect(api.getUsage).toHaveBeenCalledTimes(calls);

    visibility.mockReturnValue('visible');
    await advance(TICK);
    await flush();
    expect(vi.mocked(api.getUsage).mock.calls.length).toBeGreaterThan(calls);
  });

  it('clears the refresh timer on unmount', async () => {
    const { unmount } = renderUsage();
    await flush();
    expect(vi.getTimerCount()).toBeGreaterThan(0);

    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
