import { describe, it, expect } from 'vitest';
import { matchProjects } from './DirectoryPicker';

describe('matchProjects', () => {
  it('substring, case-insensitive; empty query returns all', () => {
    const paths = ['/Users/m/Code/api', '/Users/m/Code/web', '/tmp/scratch'];
    expect(matchProjects(paths, '')).toEqual(paths);
    expect(matchProjects(paths, 'CODE')).toEqual(['/Users/m/Code/api', '/Users/m/Code/web']);
    expect(matchProjects(paths, 'scr')).toEqual(['/tmp/scratch']);
  });
});
