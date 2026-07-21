import { describe, it, expect } from 'vitest';
import { isFindingStale } from './drift';

const lines = (arr: string[]) => arr.map((text, i) => ({ n: i + 1, text }));

describe('isFindingStale', () => {
  const file = lines(['fn a() {}', '  let x = 1;', '  let y = 2;', '}']);

  it('is not stale when the excerpt still matches the range', () => {
    expect(isFindingStale('let x = 1;\nlet y = 2;', file, [2, 3])).toBe(false);
  });

  it('tolerates leading/trailing whitespace differences', () => {
    expect(isFindingStale('   let x = 1;\n\tlet y = 2;  ', file, [2, 3])).toBe(false);
  });

  it('is stale when the code at the range changed', () => {
    expect(isFindingStale('let x = 1;\nlet y = 2;', lines(['fn a() {}', '  moved();', '}']), [2, 3])).toBe(
      true,
    );
  });

  it('is not stale when the excerpt is missing (drift unknown)', () => {
    expect(isFindingStale(undefined, file, [2, 3])).toBe(false);
    expect(isFindingStale('', file, [2, 3])).toBe(false);
  });

  it('is not stale when lineRange is missing', () => {
    expect(isFindingStale('anything', file, null)).toBe(false);
    expect(isFindingStale('anything', file, undefined)).toBe(false);
  });

  it('is stale when the range runs past the end of the file', () => {
    expect(isFindingStale('let x = 1;', lines(['only one line']), [5, 5])).toBe(true);
  });

  it('detects a real change even when blank lines rearrange', () => {
    const excerpt = 'let x = 1;\n\nlet y = 2;'; // blank interior line (line 3)
    const changed = lines(['fn a() {}', '  let x = 1;', '  let y = 2;', '  extra();']); // lines 2-4 now non-blank/changed
    expect(isFindingStale(excerpt, changed, [2, 4])).toBe(true);
  });
});
