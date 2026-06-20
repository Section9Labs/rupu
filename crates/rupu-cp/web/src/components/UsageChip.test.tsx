// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen } from '@testing-library/react';
import UsageChip from './UsageChip';
import type { UsageSummary } from '../lib/usage';

const base: UsageSummary = {
  input_tokens: 0, output_tokens: 0, cached_tokens: 0,
  total_tokens: 0, cost_usd: null, priced: true, runs: 0,
};

describe('UsageChip', () => {
  it('shows tokens and cost when priced', () => {
    render(<UsageChip usage={{ ...base, total_tokens: 4210, cost_usd: 0.03, priced: true }} />);
    expect(screen.getByText(/4,210 tok/)).toBeInTheDocument();
    expect(screen.getByText(/\$0\.0300/)).toBeInTheDocument();
  });
  it('renders an em-dash for unpriced cost', () => {
    render(<UsageChip usage={{ ...base, total_tokens: 100, cost_usd: null, priced: false }} />);
    expect(screen.getByText('—')).toBeInTheDocument();
  });
  it('marks a partial cost', () => {
    render(<UsageChip usage={{ ...base, total_tokens: 100, cost_usd: 3, priced: false }} />);
    expect(screen.getByText(/\*/)).toBeInTheDocument();
  });
});
