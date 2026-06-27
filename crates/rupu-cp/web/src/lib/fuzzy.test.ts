import { describe, it, expect } from 'vitest';
import { fuzzyScore } from './fuzzy';

describe('fuzzyScore', () => {
  it('empty query scores 0 with no matches', () => {
    expect(fuzzyScore('', 'anything')).toEqual({ score: 0, matched: [] });
  });
  it('returns null when chars are missing', () => {
    expect(fuzzyScore('xyz', 'abc')).toBeNull();
  });
  it('exact substring outranks scattered subsequence', () => {
    const a = fuzzyScore('api', 'github:acme/api')!;   // substring
    const b = fuzzyScore('api', 'a-p-i-zzzz')!;        // subsequence
    expect(a).not.toBeNull();
    expect(b).not.toBeNull();
    expect(a.score).toBeGreaterThan(b.score);
  });
  it('records matched indices for a substring', () => {
    const r = fuzzyScore('cd', 'abcde')!;
    expect(r.matched).toEqual([2, 3]);
  });
  it('rewards word-boundary starts', () => {
    const boundary = fuzzyScore('w', 'foo/web')!;       // 'w' after '/'
    const mid = fuzzyScore('w', 'crawl')!;              // 'w' mid-word
    expect(boundary.score).toBeGreaterThan(mid.score);
  });
});
