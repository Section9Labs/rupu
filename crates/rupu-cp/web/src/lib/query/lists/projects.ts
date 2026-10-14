// The Projects list's query fields.
import { Building2, CalendarClock, CircleCheck, FolderGit2, FolderOpen, GitBranch, Globe, History, Link } from 'lucide-react';
import type { CustomerRef, ProjectRow } from '../../api';
import { projectProvider } from '../../projectProvider';
import { rowField, type RowField } from '../rows';

/** A customer as the query names it: its slug or name, or `none` for "no
 *  customer". An unknown customer (absent key) matches neither. */
export function customerValues(c: CustomerRef | null | undefined): string[] {
  if (c === undefined) return [];
  return c === null ? ['none'] : [c.slug, c.name];
}

const lastActive = (p: ProjectRow) => p.last_active ?? p.last_run_at;

export const PROJECT_FIELDS: RowField<ProjectRow>[] = [
  rowField({ key: 'name', match: 'contains', get: (p) => p.name, text: true, label: 'Name', description: 'name:shop (contains)', icon: FolderGit2 }),
  rowField({ key: 'path', match: 'contains', get: (p) => p.path, text: true, label: 'Path', description: 'path:code/ (contains)', icon: FolderOpen }),
  rowField({ key: 'branch', match: 'exact', get: (p) => p.branch, facet: true, label: 'Branch', description: 'branch:main', icon: GitBranch }),
  rowField({ key: 'remote', match: 'contains', get: (p) => p.repo_remote, text: true, label: 'Remote', description: 'remote:github.com/acme (contains)', icon: Link }),
  rowField({ key: 'provider', values: ['github', 'gitlab', 'remote', 'local'], match: 'exact', get: (p) => projectProvider(p.repo_remote), facet: true, label: 'Provider', description: 'provider:github · provider:local', icon: Globe }),
  rowField({ key: 'customer', match: 'exact', get: (p) => customerValues(p.customer), facet: (p) => customerValues(p.customer)[0], label: 'Customer', description: 'customer:acme · customer:none', icon: Building2 }),
  rowField({ key: 'has', values: ['runs'], match: 'exact', get: (p) => (p.run_count > 0 ? ['runs'] : []), facet: true, label: 'Has', description: 'has:runs · -has:runs (never ran)', icon: CircleCheck }),
  rowField({ key: 'since', match: 'since', get: lastActive, label: 'Active since', description: 'since:7d · since:2026-10-01 (last active)', icon: History }),
  rowField({ key: 'until', match: 'until', get: lastActive, label: 'Active until', description: 'until:30d (last active before)', icon: CalendarClock }),
];
