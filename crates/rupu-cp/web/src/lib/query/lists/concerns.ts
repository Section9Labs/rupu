// The coverage detail tabs' query fields: the Catalog tab's concerns, the
// Audit tab's per-concern coverage and the Gap tab's gap rows.
import { CircleCheck, FileCode2, FolderTree, Gauge, ListChecks, ShieldAlert, Tag } from 'lucide-react';
import type { ConcernCoverage, CoverageConcern } from '../../api';
import type { GapRow } from '../../coverageGap';
import { filterRows, rowField, type FilterResult, type RowField, type RowValue } from '../rows';

const SEVERITIES = ['info', 'low', 'medium', 'high', 'critical'];

function concernField<Row>(get: (r: Row) => RowValue): RowField<Row> {
  return rowField({ key: 'concern', match: 'contains', get, text: true, label: 'Concern', description: 'concern:sqli (id or name, contains)', icon: ListChecks });
}

function severityField<Row>(get: (r: Row) => RowValue): RowField<Row> {
  return rowField({ key: 'severity', aliases: ['sev'], values: SEVERITIES, match: 'ordered', get, facet: true, label: 'Severity', description: 'severity:high · severity>=medium', icon: ShieldAlert });
}

function fileField<Row>(get: (r: Row) => RowValue, description: string): RowField<Row> {
  return rowField({ key: 'file', match: 'contains', get, label: 'File', description, icon: FileCode2 });
}

/** A catalog concern with the template it came from. */
export interface CatalogRow extends CoverageConcern {
  source: string;
}

export const CATALOG_FIELDS: RowField<CatalogRow>[] = [
  concernField((c) => [c.id, c.name]),
  severityField((c) => c.severity),
  rowField({ key: 'tag', match: 'exact', get: (c) => c.tags, facet: true, label: 'Tag', description: 'tag:owasp-a03', icon: Tag }),
  rowField({ key: 'source', match: 'exact', get: (c) => c.source, facet: true, label: 'Source', description: 'source:owasp-top-10 · source:inline', icon: FolderTree }),
  rowField({ key: 'glob', match: 'contains', get: (c) => c.applicable_globs, label: 'Glob', description: 'glob:.rs (an applicable glob contains)', icon: FileCode2 }),
  rowField({ key: 'strength', match: 'exact', get: (c) => c.min_strength, facet: true, label: 'Min strength', description: 'strength:asserted', icon: Gauge }),
  rowField({ key: 'description', aliases: ['desc'], match: 'contains', get: (c) => c.description, text: true, label: 'Description', description: 'description:injection', icon: ListChecks }),
];

function auditFlags(c: ConcernCoverage): string[] {
  const out: string[] = [];
  if (c.gap_files.length > 0) out.push('gap');
  if (c.findings > 0) out.push('findings');
  if (c.asserted_files.length > 0) out.push('assertions');
  return out;
}

export const AUDIT_FIELDS: RowField<ConcernCoverage>[] = [
  concernField((c) => [c.concern_id, c.name]),
  severityField((c) => c.severity),
  fileField((c) => c.in_scope_files, 'file:src/api (an in-scope file contains)'),
  rowField({ key: 'has', values: ['gap', 'findings', 'assertions'], match: 'exact', get: auditFlags, facet: true, label: 'Has', description: 'has:gap · -has:findings', icon: CircleCheck }),
];

export const GAP_FIELDS: RowField<GapRow>[] = [
  concernField((r) => [r.concern_id, r.name]),
  severityField((r) => r.severity),
  fileField((r) => r.gap_files, 'file:src/api (a gap file contains; the row lists only those)'),
];

/**
 * The gap rows `q` keeps, each listing only the gap files it keeps. The
 * query is evaluated per gap file (a row standing for that one file), so
 * `file:` terms select files: each file must meet every term, a negated one
 * included. A row stays when at least one of its gap files does.
 */
export function filterGapRows(rows: readonly GapRow[], q: string): FilterResult<GapRow> {
  const origin = new Map<GapRow, GapRow>();
  for (const r of rows) for (const f of r.gap_files) origin.set({ ...r, gap_files: [f] }, r);
  const kept = filterRows([...origin.keys()], q, GAP_FIELDS);
  const files = new Map<GapRow, string[]>();
  for (const one of kept.rows) {
    const r = origin.get(one)!;
    files.set(r, [...(files.get(r) ?? []), ...one.gap_files]);
  }
  return { rows: [...files].map(([r, gap_files]) => ({ ...r, gap_files })), error: kept.error };
}
