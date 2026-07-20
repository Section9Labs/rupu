// @vitest-environment jsdom
//
// UsageTimeline's `onSelectRange` prop (Task W3) is a pure passthrough to
// `UsageTimelineStacked` — this file mocks that child (a real Recharts drag
// isn't reproducible in jsdom; see `useDragSelection.test.ts` for the actual
// state-machine coverage) purely to prove the wiring, in a file of its own
// so the mock doesn't affect `UsageTimeline.test.tsx`'s other assertions
// about the real chart's rendered content.
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api, presetWindow, type UsageRunRow } from '../../lib/api';

vi.mock('../dashboard/UsageTimelineStacked', () => ({
  default: (props: { onSelectRange?: (startDay: string, endDay: string) => void }) => (
    <button onClick={() => props.onSelectRange?.('2026-07-10', '2026-07-12')}>trigger-select</button>
  ),
}));

// Imported AFTER the mock so it picks up the mocked child.
import UsageTimeline from './UsageTimeline';

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

const NOW = new Date('2026-07-16T12:00:00.000Z').getTime();

describe('UsageTimeline onSelectRange passthrough', () => {
  it('forwards onSelectRange to UsageTimelineStacked unchanged', async () => {
    vi.spyOn(api, 'getUsageRuns').mockResolvedValue([runRow()]);
    const onSelectRange = vi.fn();

    render(
      <UsageTimeline
        usageWindow={presetWindow('30d', NOW)}
        pivot="model"
        metric="cost"
        onMetricChange={() => {}}
        filter={noFilter()}
        excludedCount={0}
        onReset={() => {}}
        headline={{ costLabel: '$4.50', subLabel: '1.5k tokens · 1 runs' }}
        onSelectRange={onSelectRange}
      />,
    );

    fireEvent.click(await screen.findByText('trigger-select'));

    await waitFor(() => expect(onSelectRange).toHaveBeenCalledWith('2026-07-10', '2026-07-12'));
  });
});
