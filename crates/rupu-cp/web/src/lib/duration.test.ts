import { describe, it, expect } from 'vitest';
import { formatDuration } from './duration';

describe('formatDuration', () => {
  it('formats sub-second and seconds', () => {
    expect(formatDuration(450)).toBe('450ms');
    expect(formatDuration(1500)).toBe('1.5s');
    expect(formatDuration(38000)).toBe('38s');
  });

  it('formats minutes and seconds', () => {
    expect(formatDuration(125000)).toBe('2m 5s');
    expect(formatDuration(59 * 60 * 1000)).toBe('59m 0s');
  });

  it('formats hours (previously capped at minutes)', () => {
    expect(formatDuration(2 * 60 * 60 * 1000)).toBe('2h 0m');
    expect(formatDuration(90 * 60 * 1000)).toBe('1h 30m');
    expect(formatDuration(3 * 60 * 60 * 1000 + 5 * 60 * 1000)).toBe('3h 5m');
  });

  it('returns an em-dash for null/undefined', () => {
    expect(formatDuration(null)).toBe('—');
    expect(formatDuration(undefined)).toBe('—');
  });
});
