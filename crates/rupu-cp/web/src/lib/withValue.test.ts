import { describe, it, expect } from 'vitest';
import { parseWithValue, formatWithValue } from './withValue';

describe('parseWithValue', () => {
  it('parses JSON literals to typed values', () => {
    expect(parseWithValue('3')).toBe(3);
    expect(parseWithValue('true')).toBe(true);
    expect(parseWithValue('["a","b"]')).toEqual(['a', 'b']);
    expect(parseWithValue('{"k":1}')).toEqual({ k: 1 });
  });
  it('keeps templates and plain strings as strings', () => {
    expect(parseWithValue('{{ inputs.x }}')).toBe('{{ inputs.x }}');
    expect(parseWithValue('hello world')).toBe('hello world');
  });
  it('empty text signals deletion (undefined)', () => {
    expect(parseWithValue('')).toBeUndefined();
    expect(parseWithValue('   ')).toBeUndefined();
  });
  it('a JSON-quoted string stays a string', () => {
    expect(parseWithValue('"true"')).toBe('true');
  });
});

describe('formatWithValue round-trips parseWithValue', () => {
  it.each(['3', 'true', '["a","b"]', '{"k":1}', '{{ inputs.x }}', 'hello world'])('%s', (text) => {
    const v = parseWithValue(text);
    if (v === undefined) return;
    expect(parseWithValue(formatWithValue(v))).toEqual(v);
  });
});
