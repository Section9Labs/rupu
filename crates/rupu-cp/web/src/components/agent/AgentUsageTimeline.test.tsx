// @vitest-environment jsdom
// AgentUsageTimeline — the agent-scoped mount of the shared spend-over-time
// graph. `getUsageRuns` has no agent-scoping query param (unlike
// `workspace_id` for `ProjectUsageTimeline`), so this component fetches ALL
// local runs for the window and client-filters to `row.agent === agent`
// before building the graph/table. The primary risk this test guards
// against: a regression that lets another agent's rows leak into the graph
// or the breakdown table.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { api, presetWindow, type UsageRunRow } from '../../lib/api';
import AgentUsageTimeline from './AgentUsageTimeline';

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

describe('AgentUsageTimeline', () => {
  it('fetches getUsageRuns with the default range and NO workspace/agent scoping arg', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<AgentUsageTimeline agent="reviewer" />);

    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW)));
    // Called with exactly one arg — no second (workspaceId-shaped) argument.
    expect(spy.mock.calls[0]).toHaveLength(1);
  });

  it('client-filters rows to the given agent before building the graph/table', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([
      runRow({ run_id: 'run_a', agent: 'reviewer', model: 'claude' }),
      runRow({ run_id: 'run_b', agent: 'other-agent', model: 'gpt' }),
    ]);

    render(<AgentUsageTimeline agent="reviewer" />);

    // Agent A's model shows up (graph legend + breakdown table)...
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));
    // ...but agent B's rows never reach buildTimeline/aggregateRuns.
    expect(screen.queryByText('gpt')).not.toBeInTheDocument();

    const table = screen.getByRole('table');
    expect(within(table).getByText('claude')).toBeInTheDocument();
    expect(within(table).queryByText('gpt')).not.toBeInTheDocument();
  });

  it('totals in the headline only reflect the filtered agent\'s rows', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([
      runRow({ run_id: 'run_a', agent: 'reviewer', cost_usd: 4.5, total_tokens: 1500 }),
      runRow({ run_id: 'run_b', agent: 'other-agent', cost_usd: 999, total_tokens: 999000 }),
    ]);

    render(<AgentUsageTimeline agent="reviewer" />);

    // "1 runs" (only agent A's run counted) somewhere in the sub-label.
    await waitFor(() => expect(screen.getByText(/1 runs/)).toBeInTheDocument());
    expect(screen.queryByText(/999,000/)).not.toBeInTheDocument();
  });

  it('re-fetches (still unscoped) when a range button is clicked', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(<AgentUsageTimeline agent="reviewer" />);
    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', FIXED_NOW)));

    fireEvent.click(screen.getByRole('button', { name: '7d' }));

    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('7d', FIXED_NOW)));
    expect(spy.mock.calls[spy.mock.calls.length - 1]).toHaveLength(1);
  });

  it('changing the pivot re-stacks the (already agent-filtered) graph without refetching', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([
      runRow({ run_id: 'run_a', agent: 'reviewer', workflow_name: 'nightly-scan' }),
      runRow({ run_id: 'run_b', agent: 'reviewer', workflow_name: 'pr-review' }),
      runRow({ run_id: 'run_c', agent: 'other-agent', workflow_name: 'unrelated' }),
    ]);

    render(<AgentUsageTimeline agent="reviewer" />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

    fireEvent.click(screen.getByRole('button', { name: 'workflow' }));

    await waitFor(() => expect(screen.getAllByText('nightly-scan').length).toBeGreaterThan(0));
    expect(screen.getAllByText('pr-review').length).toBeGreaterThan(0);
    expect(screen.queryByText('unrelated')).not.toBeInTheDocument();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it('toggling a breakdown row excludes it from the graph and shows the exclude chip', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([
      runRow({ run_id: 'run_a', agent: 'reviewer', model: 'claude' }),
      runRow({ run_id: 'run_b', agent: 'reviewer', model: 'gpt' }),
    ]);

    render(<AgentUsageTimeline agent="reviewer" />);
    await waitFor(() => expect(screen.getAllByText('claude').length).toBeGreaterThan(0));

    const table = screen.getByRole('table');
    fireEvent.click(within(table).getByRole('checkbox', { name: 'claude' }));

    expect(await screen.findByText('Excluded (1) · reset')).toBeInTheDocument();
    const legend = screen.getByRole('list');
    await waitFor(() => expect(within(legend).queryByText('claude')).not.toBeInTheDocument());
    expect(within(legend).getByText('gpt')).toBeInTheDocument();
  });
});
