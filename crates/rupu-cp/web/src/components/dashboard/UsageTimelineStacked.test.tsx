// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import UsageTimelineStacked, { toChartData } from './UsageTimelineStacked';
import type { UsageBreakdownRow, UsageTimelineBucket } from '../../lib/usage';

function brow(model: string, cost: number | null, tokens: number): UsageBreakdownRow {
  return {
    provider: 'anthropic', model, agent: '',
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
