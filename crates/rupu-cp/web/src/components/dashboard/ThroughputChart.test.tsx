// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ThroughputChart } from './ThroughputChart';
import type { ThroughputBucket } from '../../lib/api';

afterEach(() => {
  cleanup();
});

describe('ThroughputChart', () => {
  it('shows an empty state when there are no buckets', () => {
    render(<ThroughputChart buckets={[]} />);
    expect(screen.getByText(/no runs/i)).toBeInTheDocument();
  });

  it('renders without crashing for a populated series (no empty-state text)', () => {
    const buckets: ThroughputBucket[] = [
      { ts: '2026-07-14T00:00:00Z', manual: 2, cron: 5, event: 1 },
      { ts: '2026-07-15T00:00:00Z', manual: 1, cron: 9, event: 0 },
    ];
    render(<ThroughputChart buckets={buckets} />);
    // Recharts' ResponsiveContainer does not measure >0 in jsdom (no layout
    // engine), so it renders nothing further to assert on here — the
    // meaningful assertion is that the empty-state branch does NOT fire.
    expect(screen.queryByText(/no runs/i)).not.toBeInTheDocument();
  });
});
