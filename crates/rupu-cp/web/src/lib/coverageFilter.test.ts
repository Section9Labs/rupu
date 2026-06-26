import { describe, it, expect } from 'vitest';
import { filterConcerns, filterGapRows } from './coverageFilter';
import type { GapRow } from './coverageGap';

const rows: GapRow[] = [
  { concern_id: 'a', name: 'A', severity: 'high', gap_files: ['src/api/x.rs', 'src/db/y.rs'] },
  { concern_id: 'b', name: 'B', severity: 'low', gap_files: ['src/api/z.rs'] },
  { concern_id: 'c', name: 'C', severity: 'high', gap_files: ['lib/util.rs'] },
];

describe('filterConcerns', () => {
  it('keeps all when severity is "all"', () => {
    expect(filterConcerns(rows, 'all')).toHaveLength(3);
  });
  it('filters by severity case-insensitively', () => {
    expect(filterConcerns(rows, 'high').map((r) => r.concern_id)).toEqual(['a', 'c']);
  });
});

describe('filterGapRows', () => {
  it('narrows files to substring matches and drops empty rows', () => {
    const out = filterGapRows(rows, { severity: 'all', fileQuery: 'api' });
    expect(out.map((r) => r.concern_id)).toEqual(['a', 'b']);
    expect(out[0].gap_files).toEqual(['src/api/x.rs']);
  });
  it('combines severity + file query', () => {
    const out = filterGapRows(rows, { severity: 'high', fileQuery: 'api' });
    expect(out.map((r) => r.concern_id)).toEqual(['a']);
  });
  it('no query keeps all files', () => {
    const out = filterGapRows(rows, { severity: 'all', fileQuery: '' });
    expect(out).toHaveLength(3);
    expect(out[0].gap_files).toHaveLength(2);
  });
});
