// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import UsageTimelineStacked, { toChartData } from './UsageTimelineStacked';
import type { UsageBreakdownRow, UsageTimelineBucket } from '../../lib/usage';

// `vite.config.ts` sets `globals: false`, so `@testing-library/react`'s
// auto-cleanup (which detects a global `afterEach`) never registers — without
// this, DOM from an earlier `render()` in this file (e.g. the "No usage
// recorded yet" empty state) leaks into later tests' assertions.
afterEach(() => {
  cleanup();
});

function brow(model: string, cost: number | null, tokens: number): UsageBreakdownRow {
  return {
    provider: 'anthropic', model, agent: '', workflow: '', host_id: '', workspace_id: '',
    input_tokens: tokens, output_tokens: 0, cached_tokens: 0, total_tokens: tokens,
    cost_usd: cost, priced: cost !== null, runs: 1,
  };
}

// 2 buckets × 2 models.
const BUCKETS: UsageTimelineBucket[] = [
  { bucket: '2026-06-01', rows: [brow('alpha', 2, 1000), brow('beta', 3, 2000)] },
  { bucket: '2026-06-02', rows: [brow('alpha', 4, 3000), brow('beta', 5, 4000)] },
];

describe('toChartData', () => {
  it('emits one datum per bucket with every model key present', () => {
    const { models, data } = toChartData(BUCKETS, 'cost');
    expect(models).toEqual(['alpha', 'beta']);
    expect(data).toHaveLength(2);
    expect(data[0]).toMatchObject({ bucket: '2026-06-01', alpha: 2, beta: 3 });
    expect(data[1]).toMatchObject({ bucket: '2026-06-02', alpha: 4, beta: 5 });
  });

  it('switches values between cost and tokens metrics', () => {
    const cost = toChartData(BUCKETS, 'cost');
    const tokens = toChartData(BUCKETS, 'tokens');
    expect(cost.data[0].alpha).toBe(2);
    expect(tokens.data[0].alpha).toBe(1000);
    expect(cost.data[1].beta).toBe(5);
    expect(tokens.data[1].beta).toBe(4000);
  });

  it('treats unpriced rows as 0 in cost mode but real in tokens mode', () => {
    const buckets: UsageTimelineBucket[] = [
      { bucket: '2026-06-01', rows: [brow('alpha', null, 1500)] },
    ];
    expect(toChartData(buckets, 'cost').data[0].alpha).toBe(0);
    expect(toChartData(buckets, 'tokens').data[0].alpha).toBe(1500);
  });

  it('renders an empty bucket as a continuous zero datum (gap-fill)', () => {
    const buckets: UsageTimelineBucket[] = [
      { bucket: '2026-06-10', rows: [brow('claude-sonnet-4-6', 0.5, 1000)] },
      { bucket: '2026-06-11', rows: [] }, // zero-spend day (gap-filled by the backend)
      { bucket: '2026-06-12', rows: [brow('claude-sonnet-4-6', 1.5, 3000)] },
    ];

    const { models, data } = toChartData(buckets, 'tokens');

    expect(models).toEqual(['claude-sonnet-4-6']);
    expect(data).toHaveLength(3);
    // The empty middle bucket is present and carries an explicit 0 for the model,
    // so the stacked area stays continuous (no gap / dropped point).
    expect(data[1].bucket).toBe('2026-06-11');
    expect(data[1]['claude-sonnet-4-6']).toBe(0);
    expect(data[0]['claude-sonnet-4-6']).toBe(1000);
    expect(data[2]['claude-sonnet-4-6']).toBe(3000);
  });

  it("pivot='workflow' stacks by the `workflow` field, not `model` (the U2 workaround is gone)", () => {
    function wrow(workflow: string, cost: number, tokens: number): UsageBreakdownRow {
      return {
        provider: '', model: '', agent: '', workflow, host_id: '', workspace_id: '',
        input_tokens: tokens, output_tokens: 0, cached_tokens: 0, total_tokens: tokens,
        cost_usd: cost, priced: true, runs: 1,
      };
    }
    const buckets: UsageTimelineBucket[] = [
      { bucket: '2026-06-01', rows: [wrow('nightly-scan', 2, 1000), wrow('pr-review', 3, 2000)] },
    ];
    const { models, data } = toChartData(buckets, 'cost', 'workflow');
    expect(models).toEqual(['nightly-scan', 'pr-review']);
    expect(data[0]).toMatchObject({ bucket: '2026-06-01', 'nightly-scan': 2, 'pr-review': 3 });
  });

  it("pivot='model' (the default) is unaffected by the pivot param", () => {
    const { models, data } = toChartData(BUCKETS, 'cost', 'model');
    expect(models).toEqual(['alpha', 'beta']);
    expect(data[0]).toMatchObject({ bucket: '2026-06-01', alpha: 2, beta: 3 });
  });
});

describe('UsageTimelineStacked', () => {
  it('shows the empty state with no buckets', () => {
    const { getByText } = render(<UsageTimelineStacked buckets={[]} metric="cost" />);
    expect(getByText(/No usage recorded yet/)).toBeInTheDocument();
  });
  it('renders without crashing for 2 buckets × 2 models', () => {
    render(<UsageTimelineStacked buckets={BUCKETS} metric="tokens" />);
  });

  it('stacks by workflow when pivot="workflow", not by the blank model field', () => {
    function wrow(workflow: string): UsageBreakdownRow {
      return {
        provider: '', model: '', agent: '', workflow, host_id: '', workspace_id: '',
        input_tokens: 100, output_tokens: 0, cached_tokens: 0, total_tokens: 100,
        cost_usd: 1, priced: true, runs: 1,
      };
    }
    const buckets: UsageTimelineBucket[] = [
      { bucket: '2026-06-01', rows: [wrow('nightly-scan'), wrow('pr-review')] },
    ];
    const { getByText, queryByText } = render(
      <UsageTimelineStacked buckets={buckets} metric="cost" pivot="workflow" />,
    );
    expect(getByText('nightly-scan')).toBeInTheDocument();
    expect(getByText('pr-review')).toBeInTheDocument();
    expect(queryByText('No usage recorded yet')).not.toBeInTheDocument();
  });

  it('maps host_id to the friendly host name in the legend when pivot="host"', () => {
    function hrow(hostId: string): UsageBreakdownRow {
      return {
        provider: '', model: '', agent: '', workflow: '', host_id: hostId, workspace_id: '',
        input_tokens: 100, output_tokens: 0, cached_tokens: 0, total_tokens: 100,
        cost_usd: 1, priced: true, runs: 1,
      };
    }
    const buckets: UsageTimelineBucket[] = [
      { bucket: '2026-06-01', rows: [hrow('host_01KWREMOTE')] },
    ];
    const { getByText, queryByText } = render(
      <UsageTimelineStacked
        buckets={buckets}
        metric="cost"
        pivot="host"
        hosts={[
          { host_id: 'host_01KWREMOTE', name: 'staging-box', transport_kind: 'http_cp', state: 'ok', captured_at: null, reason: null },
        ]}
      />,
    );
    expect(getByText('staging-box')).toBeInTheDocument();
    expect(queryByText('host_01KWREMOTE')).not.toBeInTheDocument();
  });

  // Task W3 — drag-select is inert when `onSelectRange` is not passed, so
  // the dashboard's other consumers of this component (no such prop) are
  // completely unaffected. Driving a real Recharts pixel-drag isn't
  // reproducible in jsdom (no ResizeObserver-driven layout to compute
  // `activeLabel` from), so the drag STATE MACHINE itself is covered by
  // `useDragSelection.test.ts` via `renderHook`; these tests cover only that
  // the component wires `onSelectRange` through without side effects at rest.
  describe('drag-select wiring (Task W3)', () => {
    it('renders no selection band before any drag, with or without onSelectRange', () => {
      const { container: withoutHandler } = render(<UsageTimelineStacked buckets={BUCKETS} metric="cost" />);
      expect(withoutHandler.querySelector('.recharts-reference-area')).toBeNull();

      const { container: withHandler } = render(
        <UsageTimelineStacked buckets={BUCKETS} metric="cost" onSelectRange={() => {}} />,
      );
      expect(withHandler.querySelector('.recharts-reference-area')).toBeNull();
    });

    it('does not call onSelectRange merely from mounting', () => {
      const onSelectRange = vi.fn();
      render(<UsageTimelineStacked buckets={BUCKETS} metric="cost" onSelectRange={onSelectRange} />);
      expect(onSelectRange).not.toHaveBeenCalled();
    });

    it('renders the same legend/chart content whether or not onSelectRange is passed', () => {
      const { container: withoutHandler } = render(<UsageTimelineStacked buckets={BUCKETS} metric="cost" />);
      const { container: withHandler } = render(
        <UsageTimelineStacked buckets={BUCKETS} metric="cost" onSelectRange={() => {}} />,
      );
      expect(withHandler.querySelectorAll('li').length).toBe(withoutHandler.querySelectorAll('li').length);
      expect(withHandler.textContent).toBe(withoutHandler.textContent);
    });
  });
});
