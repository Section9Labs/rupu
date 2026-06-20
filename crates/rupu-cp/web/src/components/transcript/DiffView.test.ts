import { describe, it, expect } from 'vitest';
import { parseDiff } from './DiffView';

// ---------------------------------------------------------------------------
// parseDiff — unit tests
// ---------------------------------------------------------------------------

describe('parseDiff', () => {
  it('parses a minimal hunk + del + add', () => {
    const result = parseDiff('@@ -1,1 +1,1 @@\n- old\n+ new');
    expect(result).toEqual([
      { type: 'hunk', text: '@@ -1,1 +1,1 @@' },
      { type: 'del', text: '- old' },
      { type: 'add', text: '+ new' },
    ]);
  });

  it('does NOT mistype "--- a/x" as del (it is ctx)', () => {
    const result = parseDiff('--- a/src/foo.rs\n+++ b/src/foo.rs');
    expect(result[0]).toEqual({ type: 'ctx', text: '--- a/src/foo.rs' });
    expect(result[1]).toEqual({ type: 'ctx', text: '+++ b/src/foo.rs' });
  });

  it('does NOT mistype "+++ b/x" as add (it is ctx)', () => {
    const result = parseDiff('+++ b/src/bar.ts');
    expect(result[0].type).toBe('ctx');
  });

  it('treats "diff --git ..." as ctx', () => {
    const result = parseDiff('diff --git a/x b/x');
    expect(result[0]).toEqual({ type: 'ctx', text: 'diff --git a/x b/x' });
  });

  it('treats "index " lines as ctx', () => {
    const result = parseDiff('index abc123..def456 100644');
    expect(result[0]).toEqual({ type: 'ctx', text: 'index abc123..def456 100644' });
  });

  it('treats context lines (space-prefixed or unchanged) as ctx', () => {
    const result = parseDiff(' unchanged line');
    expect(result[0]).toEqual({ type: 'ctx', text: ' unchanged line' });
  });

  it('handles a full realistic diff', () => {
    const diff = [
      'diff --git a/src/lib.rs b/src/lib.rs',
      'index abc..def 100644',
      '--- a/src/lib.rs',
      '+++ b/src/lib.rs',
      '@@ -1,3 +1,3 @@',
      ' pub fn hello() {',
      '-    println!("old");',
      '+    println!("new");',
      ' }',
    ].join('\n');

    const result = parseDiff(diff);

    expect(result[0]).toEqual({ type: 'ctx', text: 'diff --git a/src/lib.rs b/src/lib.rs' });
    expect(result[1]).toEqual({ type: 'ctx', text: 'index abc..def 100644' });
    expect(result[2]).toEqual({ type: 'ctx', text: '--- a/src/lib.rs' });
    expect(result[3]).toEqual({ type: 'ctx', text: '+++ b/src/lib.rs' });
    expect(result[4]).toEqual({ type: 'hunk', text: '@@ -1,3 +1,3 @@' });
    expect(result[5]).toEqual({ type: 'ctx', text: ' pub fn hello() {' });
    expect(result[6]).toEqual({ type: 'del', text: '-    println!("old");' });
    expect(result[7]).toEqual({ type: 'add', text: '+    println!("new");' });
    expect(result[8]).toEqual({ type: 'ctx', text: ' }' });
  });

  it('returns empty array for empty string', () => {
    expect(parseDiff('')).toEqual([]);
  });

  it('skips empty trailing lines', () => {
    const result = parseDiff('@@ -1 +1 @@\n');
    expect(result).toHaveLength(1);
    expect(result[0].type).toBe('hunk');
  });
});
