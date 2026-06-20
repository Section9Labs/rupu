import { describe, it, expect } from 'vitest';
import { formatTokens, formatCost } from './usage';

describe('formatTokens', () => {
  it('renders small counts with thousands separators', () => {
    expect(formatTokens(0)).toBe('0');
    expect(formatTokens(4210)).toBe('4,210');
    expect(formatTokens(999999)).toBe('999,999');
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
