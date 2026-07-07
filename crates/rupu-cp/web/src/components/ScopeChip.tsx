// Scope badge — global vs project-scoped definition. Shared across the Build
// pages (Agents / Workflows / Autoflows) and the workflow detail header, so a
// project-scoped definition reads consistently everywhere it's shown.
//
// `scope` is either `"global"` or a project's path basename (the workspace
// scope tag rupu-cp's `/api/agents` · `/api/workflows` · `/api/autoflows`
// endpoints tag rows with) — any non-"global" value falls through to the
// neutral style below.

import { cn } from '../lib/cn';

const SCOPE_CLS: Record<string, string> = {
  workspace: 'bg-surface text-ink ring-border',
  repository: 'bg-ok-bg text-ok ring-ok/30',
  global: 'bg-indigo-50 text-indigo-700 ring-indigo-200',
};

export function ScopeChip({ scope }: { scope: string }) {
  const cls = SCOPE_CLS[scope] ?? 'bg-surface text-ink ring-border';
  return (
    <span
      className={cn(
        'inline-flex items-center rounded ring-1 text-meta font-medium px-1.5 py-0.5',
        cls,
      )}
    >
      {scope}
    </span>
  );
}
