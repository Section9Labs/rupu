// filterOptions is a pure function — no DOM needed, runs in node env.
import { describe, it, expect } from 'vitest';
import { filterOptions } from './Combobox';

const opts = [
  { value: 'github:acme/api', label: 'acme/api' },
  { value: 'github:acme/web', label: 'acme/web' },
  { value: 'gitlab:corp/backend', label: 'corp/backend' },
];

describe('filterOptions', () => {
  it('returns all options for an empty query', () => {
    expect(filterOptions(opts, '')).toEqual(opts);
  });

  it('filters by label substring (case-insensitive)', () => {
    expect(filterOptions(opts, 'acme')).toEqual([
      { value: 'github:acme/api', label: 'acme/api' },
      { value: 'github:acme/web', label: 'acme/web' },
    ]);
  });

  it('filters by value substring so platform prefix is searchable', () => {
    expect(filterOptions(opts, 'gitlab')).toEqual([
      { value: 'gitlab:corp/backend', label: 'corp/backend' },
    ]);
  });

  it('returns empty array when no option matches', () => {
    expect(filterOptions(opts, 'zzz')).toEqual([]);
  });

  it('is case-insensitive on the query', () => {
    expect(filterOptions(opts, 'ACME')).toHaveLength(2);
  });

  it('matches partial value segments', () => {
    expect(filterOptions(opts, 'api')).toEqual([
      { value: 'github:acme/api', label: 'acme/api' },
    ]);
  });
});
