// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import RunUsageTimeline, { formatTokenTick, toChartPoint } from './RunUsageTimeline';
import type { UsageTimelinePoint } from '../../lib/api';

describe('formatTokenTick', () => {
  it('k-abbreviates thousands, trimming a trailing .0', () => {
    expect(formatTokenTick(2000)).toBe('2k');
    expect(formatTokenTick(1500)).toBe('1.5k');
    expect(formatTokenTick(500)).toBe('500');
    expect(formatTokenTick(0)).toBe('0');
  });
  it('abbreviates millions and billions', () => {
    expect(formatTokenTick(1_200_000)).toBe('1.2M');
    expect(formatTokenTick(3_000_000_000)).toBe('3B');
  });
});

describe('toChartPoint', () => {
  const p: UsageTimelinePoint = { turn: 1, label: 'step', tokens_in: 800, tokens_out: 120, tokens_cached: 40 };
  it('keeps all three series positive (axis split, not negation)', () => {
    const d = toChartPoint(p);
    expect(d.in).toBe(p.tokens_in);
    expect(d.out).toBe(p.tokens_out);
    expect(d.cached).toBe(p.tokens_cached);
  });
  it('preserves the original values and metadata', () => {
    const d = toChartPoint(p);
    expect(d.tokens_out).toBe(120);
    expect(d.tokens_in).toBe(800);
    expect(d.tokens_cached).toBe(40);
    expect(d.label).toBe('step');
    expect(d.turn).toBe(1);
  });
});

describe('RunUsageTimeline', () => {
  it('shows empty state when no usage', () => {
    const { getByText } = render(<RunUsageTimeline series={[]} />);
    expect(getByText(/No per-turn usage yet/)).toBeInTheDocument();
  });
  it('renders without crashing for a 3-kind series', () => {
    const series: UsageTimelinePoint[] = [
      { turn: 1, label: 'a', tokens_in: 800, tokens_out: 120, tokens_cached: 0 },
      { turn: 2, label: 'a', tokens_in: 600, tokens_out: 90, tokens_cached: 50 },
      { turn: 3, label: 'b', tokens_in: 400, tokens_out: 200, tokens_cached: 30 },
    ];
    render(<RunUsageTimeline series={series} separators />);
  });
});
