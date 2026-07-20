// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import UsageTimeline from './UsageTimeline';
import { api, presetWindow, type UsageRunRow } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

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

function noFilter() {
  return { excludedRunIds: new Set<string>(), excludedKeys: new Set<string>() };
}

// Fixed clock so two `presetWindow(...)` calls (one building the prop, one
// building the assertion) produce byte-identical windows.
const NOW = new Date('2026-07-16T12:00:00.000Z').getTime();

describe('UsageTimeline', () => {
  it('fetches getUsageRuns with the given range and no workspaceId when omitted', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);

    render(
      <UsageTimeline
        window={presetWindow('30d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={noFilter()}
        excludedCount={0}
        onReset={() => {}}
        headline={{ costLabel: '$4.50', subLabel: '1.5k tokens · 1 runs' }}
      />,
    );

    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('30d', NOW)));
    expect(await screen.findByText('claude')).toBeInTheDocument();
  });

  it('passes workspaceId through to getUsageRuns when provided', async () => {
    const spy = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow({ workspace_id: 'ws_42' })]);

    render(
      <UsageTimeline
        workspaceId="ws_42"
        window={presetWindow('7d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={noFilter()}
        excludedCount={0}
        onReset={() => {}}
        headline={{ costLabel: '$4.50', subLabel: '1.5k tokens · 1 runs' }}
      />,
    );

    await waitFor(() => expect(spy).toHaveBeenCalledWith(presetWindow('7d', NOW), 'ws_42'));
  });

  it('calls onRunsLoaded with the fetched rows once loaded', async () => {
    const rows = [runRow()];
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue(rows);
    const onRunsLoaded = vi.fn();

    render(
      <UsageTimeline
        workspaceId="ws_42"
        window={presetWindow('7d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={noFilter()}
        excludedCount={0}
        onReset={() => {}}
        headline={{ costLabel: '$4.50', subLabel: '1.5k tokens · 1 runs' }}
        onRunsLoaded={onRunsLoaded}
      />,
    );

    await waitFor(() => expect(onRunsLoaded).toHaveBeenCalledWith(rows));
  });

  it('shows the "Excluded (N) · reset" chip and calls onReset when clicked', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);
    const onReset = vi.fn();

    render(
      <UsageTimeline
        window={presetWindow('30d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={noFilter()}
        excludedCount={2}
        onReset={onReset}
        headline={{ costLabel: '$4.50', subLabel: '1.5k tokens · 1 runs' }}
      />,
    );

    const chip = await screen.findByText('Excluded (2) · reset');
    chip.click();
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it('excludes a run via the filter prop — the graph re-stacks with no refetch', async () => {
    const spy = vi
      .spyOn(api, 'getUsageRuns')
      .mockResolvedValue([runRow({ run_id: 'run_a', model: 'claude' }), runRow({ run_id: 'run_b', model: 'gpt' })]);

    const { rerender } = render(
      <UsageTimeline
        window={presetWindow('30d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={noFilter()}
        excludedCount={0}
        onReset={() => {}}
        headline={{ costLabel: '$0', subLabel: '' }}
      />,
    );

    await screen.findByText('claude');
    expect(screen.getByText('gpt')).toBeInTheDocument();

    rerender(
      <UsageTimeline
        window={presetWindow('30d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={{ excludedRunIds: new Set(['run_b']), excludedKeys: new Set() }}
        excludedCount={1}
        onReset={() => {}}
        headline={{ costLabel: '$0', subLabel: '' }}
      />,
    );

    await waitFor(() => expect(screen.queryByText('gpt')).not.toBeInTheDocument());
    expect(screen.getByText('claude')).toBeInTheDocument();
    // No refetch — the exclusion is a pure client-side re-stack.
    expect(spy).toHaveBeenCalledTimes(1);
  });
});
