// @vitest-environment jsdom
//
// ProjectUsageTimeline's drag-select wiring (Task W3) — same treatment as
// `Usage.selectRange.test.tsx`: mock `UsageTimelineStacked` (the component
// that owns the real, jsdom-unreproducible mouse handlers) with a stub
// exposing a button that fires `onSelectRange('2026-07-10', '2026-07-12')`,
// so the page-level wiring (window conversion, refetch, custom/preset chip)
// can be exercised end to end. Kept in its own file so the mock doesn't
// affect `ProjectUsageTimeline.test.tsx`'s other assertions about the real
// chart's rendered content.
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api, presetWindow, windowFromDayRange, type UsageRunRow } from '../../lib/api';

vi.mock('../dashboard/UsageTimelineStacked', () => ({
  default: (props: { onSelectRange?: (startDay: string, endDay: string) => void }) => (
    <button onClick={() => props.onSelectRange?.('2026-07-10', '2026-07-12')}>trigger-select</button>
  ),
}));

// Imported AFTER the mock so it (transitively, via UsageTimeline) picks up
// the mocked chart.
import ProjectUsageTimeline from './ProjectUsageTimeline';

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

function runRow(overrides: Partial<UsageRunRow> = {}): UsageRunRow {
  return {
    run_id: 'run_1',
    started_at: new Date().toISOString(),
    workflow_name: 'nightly-review',
    agent: 'reviewer',
    provider: 'anthropic',
    model: 'claude',
    workspace_id: 'ws_42',
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

describe('ProjectUsageTimeline — drag-select a custom window (Task W3)', () => {
  it('a drag-select narrows the whole graph+table to the selected window', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'ws_42'));

    fireEvent.click(await screen.findByText('trigger-select'));

    const custom = windowFromDayRange('2026-07-10', '2026-07-12');
    await waitFor(() => expect(spy).toHaveBeenCalledWith(custom, 'ws_42'));
  });

  it('shows a "custom" chip once a drag-select is active, cleared by the × or a preset click', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(api.getUsageRuns).toHaveBeenCalledTimes(1));

    expect(screen.queryByText(/custom/i)).not.toBeInTheDocument();

    fireEvent.click(await screen.findByText('trigger-select'));
    expect(await screen.findByText(/custom/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /clear custom|×/i }));
    expect(screen.queryByText(/custom/i)).not.toBeInTheDocument();
  });

  it('clicking a preset range button clears an active custom window', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'ws_42'));

    fireEvent.click(await screen.findByText('trigger-select'));
    await screen.findByText(/custom/i);

    fireEvent.click(screen.getByRole('button', { name: '7d' }));

    await waitFor(() => expect(spy).toHaveBeenLastCalledWith(presetWindow('7d', FIXED_NOW), 'ws_42'));
    expect(screen.queryByText(/custom/i)).not.toBeInTheDocument();
  });
});
