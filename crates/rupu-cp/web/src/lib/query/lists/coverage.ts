// The Coverage list's query fields (one row per assessed target).
import { CircleCheck, Crosshair, FolderGit2 } from 'lucide-react';
import type { CoverageSummary } from '../../api';
import { rowField, type RowField } from '../rows';

function coverageFlags(t: CoverageSummary): string[] {
  const out: string[] = [];
  if (t.findings > 0) out.push('findings');
  if (t.has_catalog) out.push('catalog');
  if (t.assertion_lines > 0) out.push('assertions');
  return out;
}

export const COVERAGE_FIELDS: RowField<CoverageSummary>[] = [
  rowField({ key: 'target', match: 'contains', get: (t) => t.target_id, text: true, label: 'Target', description: 'target:api (contains)', icon: Crosshair }),
  rowField({ key: 'project', match: 'exact', get: (t) => t.project || t.ws_id, facet: true, text: true, label: 'Project', description: 'project:shop-web', icon: FolderGit2 }),
  rowField({ key: 'has', values: ['findings', 'catalog', 'assertions'], match: 'exact', get: coverageFlags, facet: true, label: 'Has', description: 'has:findings · -has:catalog', icon: CircleCheck }),
];
