import { describe, expect, it } from 'vitest';
import { FINDING_FIELDS } from './fields';
import { soleValues, withTerm } from './shortcut';

describe('soleValues', () => {
  it("gives the values of the key's only positive `:` term", () => {
    expect(soleValues('severity:high tag:x', FINDING_FIELDS, 'severity')).toEqual(['high']);
    expect(soleValues('sev:high,low', FINDING_FIELDS, 'severity')).toEqual(['high', 'low']);
  });

  it('is null for no term, two terms, a negated or compared term, or a parse error', () => {
    expect(soleValues('tag:x', FINDING_FIELDS, 'severity')).toBeNull();
    expect(soleValues('severity:high severity:low', FINDING_FIELDS, 'severity')).toBeNull();
    expect(soleValues('-severity:high', FINDING_FIELDS, 'severity')).toBeNull();
    expect(soleValues('severity>=high', FINDING_FIELDS, 'severity')).toBeNull();
    expect(soleValues('severity:high nope:1', FINDING_FIELDS, 'severity')).toBeNull();
  });
});

describe('withTerm', () => {
  it('replaces every token for the key and keeps the rest in order', () => {
    expect(withTerm('severity:low tag:needs-poc -sev:info', FINDING_FIELDS, 'severity', ['high'])).toBe('tag:needs-poc severity:high');
    expect(withTerm('', FINDING_FIELDS, 'severity', ['high'])).toBe('severity:high');
  });

  it('removes the key for null or no values', () => {
    expect(withTerm('severity:high tag:x', FINDING_FIELDS, 'severity', null)).toBe('tag:x');
    expect(withTerm('severity:high tag:x', FINDING_FIELDS, 'severity', [])).toBe('tag:x');
  });

  it('joins several values with commas, quoting the ones that need it', () => {
    expect(withTerm('xss', FINDING_FIELDS, 'owner', ['Payments Team', 'web'])).toBe('xss owner:"Payments Team",web');
  });

  it('keeps a token that does not parse', () => {
    expect(withTerm('nope:1 severity:low', FINDING_FIELDS, 'severity', ['high'])).toBe('nope:1 severity:high');
  });
});
