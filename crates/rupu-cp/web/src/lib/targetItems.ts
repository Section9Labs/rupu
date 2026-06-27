import type { ProjectRow, RepoEntry, FsEntry } from './api';

export type TargetKind = 'workspace' | 'project' | 'repo' | 'directory';
export interface TargetItem {
  kind: TargetKind;
  label: string;
  sublabel?: string;
  resolved: { target?: string; working_dir?: string };
}

export const WORKSPACE_ITEM: TargetItem = {
  kind: 'workspace',
  label: 'This workspace',
  sublabel: 'run in the control-plane working directory',
  resolved: {},
};

export function projectItems(projects: ProjectRow[]): TargetItem[] {
  return projects.map((p) => ({
    kind: 'project',
    label: p.name,
    sublabel: p.path,
    resolved: { working_dir: p.path },
  }));
}

export function repoItems(repos: RepoEntry[]): TargetItem[] {
  return repos.map((r) => {
    const ref = `${r.platform}:${r.repo}`;
    return { kind: 'repo', label: ref, sublabel: r.default_branch, resolved: { target: ref } };
  });
}

export function dirItems(dirs: FsEntry[]): TargetItem[] {
  return dirs.map((d) => ({
    kind: 'directory',
    label: d.name,
    sublabel: d.path,
    resolved: { working_dir: d.path },
  }));
}

const REPO_RE = /^[a-z][a-z0-9]*:.+/;

export function looksLikePath(query: string): boolean {
  const q = query.trim();
  return q.startsWith('/') || q.startsWith('~') || q.startsWith('.') || q.includes('/');
}

/** Synthesize an item from raw text so the user can always pick what they typed. */
export function inferFreeTextItem(query: string): TargetItem | null {
  const q = query.trim();
  if (!q) return null;
  if (REPO_RE.test(q) && !q.startsWith('/') && !q.startsWith('~') && !q.startsWith('.')) {
    return { kind: 'repo', label: q, resolved: { target: q } };
  }
  if (looksLikePath(q)) {
    return { kind: 'directory', label: q, sublabel: 'directory', resolved: { working_dir: q } };
  }
  return null;
}
