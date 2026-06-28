import { describe, it, expect } from 'vitest';
import { shortId } from './shortId';

describe('shortId', () => {
  it('truncates ids longer than the head length', () => {
    expect(shortId('01HZX8K9ABCDEF', 8)).toBe('01HZX8K9…');
  });

  it('defaults to a head of 8', () => {
    expect(shortId('0123456789')).toBe('01234567…');
  });

  it('honours a custom head length', () => {
    expect(shortId('0123456789ABCDEF', 10)).toBe('0123456789…');
  });

  it('returns short ids unchanged', () => {
    expect(shortId('abc')).toBe('abc');
    expect(shortId('12345678')).toBe('12345678');
  });
});
