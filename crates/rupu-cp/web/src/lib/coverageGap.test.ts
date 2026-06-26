import { describe, it, expect } from 'vitest';
import { gapRows } from './coverageGap';
import type { AuditReport } from './api';

function report(concerns: AuditReport['concerns']): AuditReport {
  return {
    target_id: 't',
    concerns,
    files: [],
    cross_model: [],
    serendipitous: [],
    total_concerns: concerns.length,
    complete_concerns: 0,
    total_gap_files: 0,
  };
}

const base = {
  in_scope_files: [],
  asserted_files: [],
  clean: 0,
  findings: 0,
  examined: 0,
  not_applicable: 0,
};

describe('gapRows', () => {
  it('keeps only concerns with gap files, severity-sorted', () => {
    const r = report([
      { concern_id: 'a', name: 'A', severity: 'low', gap_files: ['x.rs'], ...base },
      { concern_id: 'b', name: 'B', severity: 'critical', gap_files: ['y.rs', 'z.rs'], ...base },
      { concern_id: 'c', name: 'C', severity: 'high', gap_files: [], ...base },
    ]);
    const rows = gapRows(r);
    expect(rows.map((x) => x.concern_id)).toEqual(['b', 'a']);
    expect(rows[0].gap_files).toEqual(['y.rs', 'z.rs']);
  });

  it('returns empty when there are no gaps', () => {
    expect(gapRows(report([]))).toEqual([]);
  });
});
