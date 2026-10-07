import { describe, expect, it } from 'vitest';
import { parseQuery, parseToken, quoteValue, tokenize } from './grammar';
import { FINDING_FIELDS } from './fields';

describe('grammar extras', () => {
  it('tokenize keeps raw token text and code-point offsets', () => {
    const r = tokenize('é tag:"a b"  -x');
    expect(r.ok && r.tokens.map((t) => [t.text, t.start, t.end])).toEqual([
      ['é', 0, 1],
      ['tag:"a b"', 2, 11],
      ['-x', 13, 15],
    ]);
  });
  it('errors carry the token span', () => {
    const r = parseQuery('é tag>=x', FINDING_FIELDS);
    expect(!r.ok && [r.error.token, r.error.start, r.error.end, r.error.code]).toEqual([1, 2, 8, 'bad_operator']);
  });
  it('parseToken parses one chip', () => {
    expect(parseToken('-tag:needs-poc', FINDING_FIELDS)).toEqual({
      ok: true,
      terms: [{ neg: true, key: 'tag', op: 'eq', values: ['needs-poc'] }],
    });
  });
  it('quoteValue quotes only when it must, and round-trips', () => {
    expect(quoteValue('needs-poc')).toBe('needs-poc');
    expect(quoteValue('Payments Team')).toBe('"Payments Team"');
    expect(quoteValue('say "hi"')).toBe('"say \\"hi\\""');
    const r = parseQuery(`owner:${quoteValue('a, "b"')}`, FINDING_FIELDS);
    expect(r.ok && r.terms[0].values).toEqual(['a, "b"']);
  });
});
