// @vitest-environment jsdom
//
// Usage's drag-select wiring (Task W3). A real Recharts pixel-drag isn't
// reproducible in jsdom (see `useDragSelection.test.ts` for the actual
// state-machine coverage) — this file mocks `UsageTimelineStacked` (the
// component that owns the real mouse handlers) with a stub exposing a
// button that calls `onSelectRange('2026-07-10', '2026-07-12')`, so the
// PAGE-level wiring (window conversion, refetch, custom/preset chip) can be
// exercised end to end. Kept in its own file so the mock doesn't affect
// `Usage.test.tsx`'s other assertions about the real chart's rendered
// content.
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, presetWindow, windowFromDayRange, type UsageResponse, type UsageRunRow } from '../lib/api';

vi.mock('../components/dashboard/UsageTimelineStacked', () => ({
  default: (props: { onSelectRange?: (startDay: string, endDay: string) => void }) => (
    <button onClick={() => props.onSelectRange?.('2026-07-10', '2026-07-12')}>trigger-select</button>
  ),
}));

// Imported AFTER the mock so it (transitively, via UsageTimeline) picks up
// the mocked chart.
import Usage from './Usage';

const FIXED_NOW = new Date('2026-07-16T12:00:00.000Z').getTime();

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(FIXED_NOW);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function usageResponse(overrides: Partial<UsageResponse> = {}): UsageResponse {
  return {
    summary: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: 0, priced: true, runs: 0 },
    breakdown: [],
    unpriced: { models: [], rows: 0 },
    hosts: [],
    ...overrides,
  };
}

function runRow(overrides: Partial<UsageRunRow> = {}): UsageRunRow {
  return {
    run_id: 'run_1',
    started_at: new Date().toISOString(),
    workflow_name: 'nightly-review',
    agent: 'reviewer',
    provider: 'anthropic',
    model: 'claude',
    workspace_id: 'ws_1',
    host_id: 'local',
    input_tokens: 1000,
    output_tokens: 500,
    cached_tokens: 0,
    total_tokens: 1500,
    cost_usd: 4.5,
    priced: true,
    ...overrides,
  };
}

function mockAll() {
  vi.spyOn(api, 'getUsage').mockResolvedValue(usageResponse());
  vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);
  vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);
}

function renderUsage() {
  return render(
    <MemoryRouter>
      <Usage />
    </MemoryRouter>,
  );
}

describe('Usage page — drag-select a custom window (Task W3)', () => {
  it('a drag-select narrows the whole page to the selected window', async () => {
    mockAll();
    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'model'));

    fireEvent.click(await screen.findByText('trigger-select'));

    const custom = windowFromDayRange('2026-07-10', '2026-07-12');
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith(custom, 'model'));
    await waitFor(() => expect(api.getUsageOutliers).toHaveBeenCalledWith(custom));
    await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledWith(custom));
  });

  it('shows a "custom" chip once a drag-select is active', async () => {
    mockAll();
    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'model'));

    expect(screen.queryByText(/custom/i)).not.toBeInTheDocument();

    fireEvent.click(await screen.findByText('trigger-select'));

    expect(await screen.findByText(/custom/i)).toBeInTheDocument();
  });

  it('clicking the custom chip\'s clear (×) restores the active preset window', async () => {
    mockAll();
    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'model'));

    fireEvent.click(await screen.findByText('trigger-select'));
    await screen.findByText(/custom/i);

    fireEvent.click(screen.getByRole('button', { name: /clear custom|×/i }));

    await waitFor(() =>
      expect(api.getUsage).toHaveBeenLastCalledWith(presetWindow('30d', FIXED_NOW), 'model'),
    );
    expect(screen.queryByText(/custom/i)).not.toBeInTheDocument();
  });

  it('clicking a preset range button also clears the custom window', async () => {
    mockAll();
    renderUsage();
    await waitFor(() => expect(api.getUsage).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'model'));

    fireEvent.click(await screen.findByText('trigger-select'));
    await screen.findByText(/custom/i);

    fireEvent.click(screen.getByRole('button', { name: '7d' }));

    await waitFor(() => expect(api.getUsage).toHaveBeenLastCalledWith(presetWindow('7d', FIXED_NOW), 'model'));
    expect(screen.queryByText(/custom/i)).not.toBeInTheDocument();
  });
});
