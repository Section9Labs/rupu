// @vitest-environment jsdom
// ProjectUsageTimeline — the project-scoped mount of the shared spend-over-
// time graph (Task U4). Replaces the per-run `UsageBarChart` in the Runs
// tab: `getUsageRuns(window, wsId)` -> `buildTimeline` -> the SAME
// `UsageTimeline` graph `/usage` uses, plus a local breakdown table (built
// from `aggregateRuns`, since there is no `/api/usage?workspace_id=` to
// source one from) with checkbox exclusion. No outlier panel — see the
// component's own doc comment for why.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { api, presetWindow, type UsageRunRow } from '../../lib/api';
import ProjectUsageTimeline from './ProjectUsageTimeline';

// Fixed clock so `presetWindow(...)` computed here (for assertions) and
// inside `ProjectUsageTimeline` (driving the actual fetch) agree exactly.
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

describe('ProjectUsageTimeline', () => {
  it('fetches getUsageRuns with the default range and the project wsId', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<ProjectUsageTimeline wsId="ws_42" />);

    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'ws_42'));
    // "claude" renders both in the graph legend and the breakdown table row.
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));
  });

  it('re-fetches with the new range when a range button is clicked', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW), 'ws_42'));

    fireEvent.click(screen.getByRole('button', { name: '7d' }));

    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('7d', FIXED_NOW), 'ws_42'));
  });

  it('changing the pivot re-stacks the graph without refetching', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([
      runRow({ run_id: 'run_a', workflow_name: 'nightly-scan' }),
      runRow({ run_id: 'run_b', workflow_name: 'pr-review' }),
    ]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

    fireEvent.click(screen.getByRole('button', { name: 'workflow' }));

    await waitFor(() => expect(screen.getAllByText('nightly-scan').length).toBeGreaterThan(0));
    expect(screen.getAllByText('pr-review').length).toBeGreaterThan(0);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('toggling a breakdown row excludes it from the graph and shows the exclude chip', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([
      runRow({ run_id: 'run_a', model: 'claude' }),
      runRow({ run_id: 'run_b', model: 'gpt' }),
    ]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

    const table = screen.getByRole('table');
    fireEvent.click(within(table).getByRole('checkbox', { name: 'claude' }));

    expect(await screen.findByText('Excluded (1) · reset')).toBeInTheDocument();
    const legend = screen.getByRole('list');
    await waitFor(() => expect(within(legend).queryByText('claude')).not.toBeInTheDocument());
    expect(within(legend).getByText('gpt')).toBeInTheDocument();
  });

  it('does not render an outlier panel (no workspace-scoped outliers endpoint)', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<ProjectUsageTimeline wsId="ws_42" />);
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

    expect(screen.queryByText(/cost outliers/i)).not.toBeInTheDocument();
  });
});
