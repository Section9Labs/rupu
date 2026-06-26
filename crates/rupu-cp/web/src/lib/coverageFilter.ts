import type { GapRow } from './coverageGap';

/** Keep rows matching `severity` ('all' keeps everything), case-insensitive. */
export function filterConcerns<T extends { severity: string }>(rows: T[], severity: string): T[] {
  if (severity === 'all') return rows;
  const want = severity.toLowerCase();
  return rows.filter((r) => r.severity.toLowerCase() === want);
}

/**
 * Apply the severity filter, then (when `fileQuery` is non-empty) narrow each
 * row's `gap_files` to case-insensitive substring matches, dropping rows that
 * end up with no matching files.
 */
export function filterGapRows(
  rows: GapRow[],
  opts: { severity: string; fileQuery: string },
): GapRow[] {
  const bySeverity = filterConcerns(rows, opts.severity);
  const q = opts.fileQuery.trim().toLowerCase();
  if (!q) return bySeverity;
  return bySeverity
    .map((r) => ({ ...r, gap_files: r.gap_files.filter((f) => f.toLowerCase().includes(q)) }))
    .filter((r) => r.gap_files.length > 0);
}
