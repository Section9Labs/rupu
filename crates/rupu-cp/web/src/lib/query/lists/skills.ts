// The Skills library's query fields. `project:` is not a row filter: it picks
// which project's layer the server lists next to the global one (the page
// reads it with `skillsProject` and strips it before filtering).
import { CircleCheck, FileText, FolderGit2, Layers, Link, ShieldCheck, Sparkles } from 'lucide-react';
import type { SkillRow } from '../../api';
import { soleValues, withTerm } from '../../findingQuery/shortcut';
import { parseQuery } from '../../findingQuery/grammar';
import { rowField, type RowField } from '../rows';

function skillFlags(s: SkillRow): string[] {
  const out: string[] = [];
  if (s.warnings.length > 0) out.push('warnings');
  if (s.source) out.push('source');
  if (s.both_layers) out.push('both_layers');
  return out;
}

export const SKILL_FIELDS: RowField<SkillRow>[] = [
  rowField({ key: 'name', match: 'contains', get: (s) => s.name, text: true, label: 'Name', description: 'name:pdf (contains)', icon: Sparkles }),
  rowField({ key: 'project', match: 'exact', get: () => undefined, label: 'Project', description: "project:acme (also list that project's skills)", icon: FolderGit2 }),
  rowField({ key: 'layer', values: ['global', 'project'], match: 'exact', get: (s) => s.layer, facet: true, label: 'Layer', description: 'layer:project', icon: Layers }),
  rowField({ key: 'status', values: ['ok', 'invalid'], match: 'exact', get: (s) => s.status, facet: true, label: 'Status', description: 'status:invalid', icon: ShieldCheck }),
  rowField({ key: 'has', values: ['warnings', 'source', 'both_layers'], match: 'exact', get: skillFlags, facet: true, label: 'Has', description: 'has:warnings · has:both_layers · -has:source (hand-placed)', icon: CircleCheck }),
  rowField({ key: 'source', match: 'contains', get: (s) => s.source, facet: true, label: 'Source', description: 'source:github:acme (contains)', icon: Link }),
  rowField({ key: 'description', aliases: ['desc'], match: 'contains', get: (s) => s.description, text: true, label: 'Description', description: 'description:pdf', icon: FileText }),
];

/** The project a query lists (`null` for the global layer alone), the query
 *  without its `project:` terms, and why its `project:` can't be used. */
export function skillsProject(q: string): { project: string | null; rest: string; error: string | null } {
  const p = parseQuery(q, SKILL_FIELDS);
  const rest = withTerm(q, SKILL_FIELDS, 'project', null);
  if (!p.ok || !p.terms.some((t) => t.key === 'project')) return { project: null, rest: p.ok ? rest : q, error: null };
  const sole = soleValues(q, SKILL_FIELDS, 'project');
  if (sole === null || sole.length !== 1) {
    return { project: null, rest, error: 'project: names one project, whose skills list next to the global ones' };
  }
  return { project: sole[0], rest, error: null };
}
