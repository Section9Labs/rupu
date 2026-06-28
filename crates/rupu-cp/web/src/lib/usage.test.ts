import { describe, it, expect } from 'vitest';
import { formatTokens, formatCost } from './usage';

describe('formatTokens', () => {
  it('renders counts under 10k with thousands separators', () => {
    expect(formatTokens(0)).toBe('0');
    expect(formatTokens(4210)).toBe('4,210');
    expect(formatTokens(9999)).toBe('9,999');
  });
  it('compacts thousands from 10k', () => {
    expect(formatTokens(10_000)).toBe('10k');
    expect(formatTokens(50_000)).toBe('50k');
    expect(formatTokens(950_000)).toBe('950k');
  });
  it('compacts millions and billions', () => {
    expect(formatTokens(1_200_000)).toBe('1.2M');
    expect(formatTokens(3_400_000_000)).toBe('3.4B');
  });
});

describe('formatCost', () => {
  it('renders an em-dash when unpriced (null)', () => {
    expect(formatCost(null)).toBe('—');
  });
  it('shows 4 decimals under a dollar, 2 at or above', () => {
    expect(formatCost(0.0312)).toBe('$0.0312');
    expect(formatCost(12.5)).toBe('$12.50');
    expect(formatCost(0)).toBe('$0.0000');
  });
});
