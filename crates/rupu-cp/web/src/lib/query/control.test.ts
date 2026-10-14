import { describe, expect, it } from 'vitest';
import { customersRange, customersRowQuery } from './lists/customers';
import { skillsProject } from './lists/skills';

describe('control terms', () => {
  it('keep their value while another token is broken', () => {
    expect(customersRange('range:7d has:').range).toBe('7d');
    expect(customersRange('range:7d has:findings')).toEqual({ range: '7d', rest: 'has:findings', error: null });
    expect(skillsProject('project:acme has:').project).toBe('acme');
  });
  it('fall back to the default when absent, and refuse two values', () => {
    expect(customersRange('has:findings').range).toBe('30d');
    expect(customersRange('range:7d,all').error).toMatch(/one window/);
  });
});

describe('customersRowQuery', () => {
  it('keeps archived customers out unless the query names is:, even with a broken token', () => {
    expect(customersRowQuery('has:findings')).toBe('has:findings is:active');
    expect(customersRowQuery('has:')).toBe('has: is:active');
    expect(customersRowQuery('is:archived has:')).toBe('is:archived has:');
  });
});
