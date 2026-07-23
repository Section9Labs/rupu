// Scope badge — global vs project-scoped definition. Shared across the Build
// pages (Agents / Workflows / Autoflows) and the workflow detail header, so a
// project-scoped definition reads consistently everywhere it's shown.
//
// `scope` is either `"global"` or a project's path basename (the workspace
// scope tag rupu-cp's `/api/agents` · `/api/workflows` · `/api/autoflows`
// endpoints tag rows with) — any non-"global" value falls through to the
// neutral style below.

import { cn } from '../lib/cn';

// Token-only (dark-theme bug fix): `global` used to hardcode Tailwind's
// static `indigo-50/700/200` palette, which doesn't adapt under
// `[data-theme="dark"]` (unlike every other tone here, which resolves
// through the themed `--c-*` CSS variables). `info` is the same semantic
// "global" concept `settings/ConfigField.tsx`'s provenance badge already
// uses, so this also keeps the two "global" affordances visually aligned.
const SCOPE_CLS: Record<string, string> = {
  workspace: 'bg-surface text-ink ring-border',
  repository: 'bg-ok-bg text-ok ring-ok/30',
  global: 'bg-info-bg text-info ring-info/30',
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
