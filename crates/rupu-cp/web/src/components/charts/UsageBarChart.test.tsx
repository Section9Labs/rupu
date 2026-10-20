// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import UsageBarChart from './UsageBarChart';

afterEach(cleanup);

describe('UsageBarChart (usage footprint strip)', () => {
  it('shows the empty state when there is no usage', () => {
    render(<UsageBarChart bars={[]} />);
    expect(screen.getByText(/No token usage/)).toBeInTheDocument();
  });

  it('sums the list into labelled spend + token figures', () => {
    const bars = [
      { id: 'a', label: 'a', input_tokens: 100, output_tokens: 20, cached_tokens: 5, cost_usd: 0.01 },
      { id: 'b', label: 'b', input_tokens: 300, output_tokens: 80, cached_tokens: 15, cost_usd: 0.03 },
    ];
    render(<UsageBarChart bars={bars} />);
    // Every figure is labelled (a strip, not a run-on sentence or a chart).
    for (const label of ['Spend', 'Tokens', 'In', 'Out', 'Cached']) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
    // In = 100 + 300 = 400.
    expect(screen.getByText('400')).toBeInTheDocument();
  });

  it('shows a dash for spend only when nothing in the list priced', () => {
    const bars = [
      { id: 'a', label: 'a', input_tokens: 100, output_tokens: 20, cached_tokens: 0, cost_usd: null },
    ];
    render(<UsageBarChart bars={bars} />);
    const spend = screen.getByText('Spend').nextElementSibling;
    expect(spend).toHaveTextContent('—');
  });
});
