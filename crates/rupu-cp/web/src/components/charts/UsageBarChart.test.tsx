// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import UsageBarChart from './UsageBarChart';

describe('UsageBarChart', () => {
  it('shows empty state when no usage', () => {
    render(<MemoryRouter><UsageBarChart bars={[]} /></MemoryRouter>);
    expect(screen.getByText(/No token usage/)).toBeInTheDocument();
  });
  it('renders without crashing for non-empty', () => {
    const bars = [{ id: 'a', label: 'a', input_tokens: 100, output_tokens: 20, cached_tokens: 0, cost_usd: 0.01 }];
    render(<MemoryRouter><UsageBarChart bars={bars} /></MemoryRouter>);
  });
});
