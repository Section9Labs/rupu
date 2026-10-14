// The coverage detail tabs' query fields: the Catalog tab's concerns, the
// Audit tab's per-concern coverage and the Gap tab's gap rows.
import { CircleCheck, FileCode2, FolderTree, Gauge, ListChecks, ShieldAlert, Tag } from 'lucide-react';
import type { ConcernCoverage, CoverageConcern } from '../../api';
import type { GapRow } from '../../coverageGap';
import type { QueryField } from '../../findingQuery/fields';
import { parseQuery } from '../../findingQuery/grammar';
import { rowField, type RowField, type RowValue } from '../rows';

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
 * The `files` a query's `file:` terms keep: each positive term keeps the
 * files containing one of its values, each negated term drops them. A query
 * that doesn't parse, or names no file, keeps every file.
 */
export function filesMatching(files: readonly string[], q: string, fields: readonly QueryField[]): string[] {
  const p = parseQuery(q, fields);
  if (!p.ok) return [...files];
  const terms = p.terms.filter((t) => t.key === 'file');
  return files.filter((f) =>
    terms.every((t) => t.values.some((v) => f.toLowerCase().includes(v.toLowerCase())) !== t.neg),
  );
}
