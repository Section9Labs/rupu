// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import UsageTimelineStacked, { toChartData } from './UsageTimelineStacked';
import type { UsageBreakdownRow, UsageTimelineBucket } from '../../lib/usage';

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
});

describe('UsageTimelineStacked', () => {
  it('shows the empty state with no buckets', () => {
    const { getByText } = render(<UsageTimelineStacked buckets={[]} metric="cost" />);
    expect(getByText(/No usage recorded yet/)).toBeInTheDocument();
  });
  it('renders without crashing for 2 buckets × 2 models', () => {
    render(<UsageTimelineStacked buckets={BUCKETS} metric="tokens" />);
  });
});
