import { describe, expect, it } from 'vitest';
import { formatSpendShort } from './agentiflow';

describe('formatSpendShort', () => {
  it('two_decimals_and_under_a_cent_says_so', () => {
    expect(formatSpendShort(0)).toBe('$0.00');
    expect(formatSpendShort(0.0042)).toBe('<$0.01');
    expect(formatSpendShort(0.01)).toBe('$0.01');
    expect(formatSpendShort(162.894)).toBe('$162.89');
  });
});
