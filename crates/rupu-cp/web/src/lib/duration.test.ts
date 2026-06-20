import { describe, it, expect } from 'vitest';
import { formatDuration } from './duration';
describe('formatDuration', () => {
  it('formats ms', () => {
    expect(formatDuration(38000)).toBe('38s');
    expect(formatDuration(1500)).toBe('1.5s');
    expect(formatDuration(125000)).toBe('2m 5s');
    expect(formatDuration(null)).toBe('—');
    expect(formatDuration(450)).toBe('450ms');
  });
});
