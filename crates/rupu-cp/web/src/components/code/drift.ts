/** Collapse a line to its comparable core: trim ends, collapse inner runs of
 *  whitespace to a single space. Drift detection should ignore reindentation
 *  and trailing-newline noise, not real edits. */
function norm(s: string): string {
  return s.replace(/\s+/g, ' ').trim();
}

/**
 * True when a finding's recorded `code_excerpt` no longer matches the current
 * file content at its `line_range`. Absent excerpt or range → drift is
 * unknown, reported as not-stale (no note shown).
 */
export function isFindingStale(
  excerpt: string | null | undefined,
  fileLines: { n: number; text: string }[],
  lineRange: [number, number] | null | undefined,
): boolean {
  if (!excerpt || !excerpt.trim() || !lineRange) return false;
  const [start, end] = lineRange;
  const current: string[] = [];
  for (let n = start; n <= end; n++) {
    const ln = fileLines[n - 1];
    if (!ln) return true; // range runs past EOF → definitely drifted
    current.push(ln.text);
  }
  const want = excerpt
    .split('\n')
    .map(norm)
    .filter((l) => l.length > 0);
  const have = current.map(norm).filter((l) => l.length > 0);
  if (want.length === 0) return false;
  // The excerpt must appear as a contiguous, in-order match of the range.
  return want.join('\n') !== have.join('\n');
}
