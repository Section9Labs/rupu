import { normFindingSeverity, sevRank, type AuditReport } from './api';

export interface GapRow {
  concern_id: string;
  name: string;
  severity: string;
  gap_files: string[];
}

/** Concerns with unassessed in-scope files, severity-sorted (critical→info). */
export function gapRows(report: AuditReport): GapRow[] {
  return report.concerns
    .filter((c) => c.gap_files.length > 0)
    .map((c) => ({
      concern_id: c.concern_id,
      name: c.name,
      severity: c.severity,
      gap_files: c.gap_files,
    }))
    .sort(
      // SEV_ORDER puts critical at index 0, so ascending rank = critical→info.
      (a, b) => sevRank(normFindingSeverity(a.severity)) - sevRank(normFindingSeverity(b.severity)),
    );
}
