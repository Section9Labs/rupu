// AgentIdentity — the two-line identity block for narrow surfaces (run-graph
// nodes). AgentName packs `leaf · agent · provider/model` onto one line, which
// a 170–220px node clips; here the name gets its own line and provider/model
// a second, smaller one, so neither truncates the other away. The block's
// title is always the full identity.

import { cn } from '../../lib/cn';
import { identityTitle, memberLabel, parseCodename } from '../../lib/codename';
import { derivedTitle } from './CrewChip';
import { RoleBadge } from './RoleBadge';

export interface AgentIdentityProps {
  codename?: string | null;
  agent?: string;
  provider?: string;
  model?: string;
  derived?: boolean;
  className?: string;
}

export function AgentIdentity({ codename, agent, provider, model, derived, className }: AgentIdentityProps) {
  const head = codename ? memberLabel(codename, agent) : (agent ?? '');
  const pm = [provider, model].filter(Boolean).join('/');
  if (!head && !pm) return null;
  const role = codename ? parseCodename(codename).role : undefined;
  return (
    <div
      data-testid="agent-identity"
      className={cn('min-w-0', derived && 'opacity-60', className)}
      title={derivedTitle(identityTitle(codename ?? undefined, agent, provider, model), derived)}
    >
      {head && (
        <div className="flex min-w-0 items-center gap-1">
          {role && <RoleBadge role={role} />}
          <span data-testid="agent-head" className="truncate font-mono">
            {head}
          </span>
        </div>
      )}
      {pm && (
        <div data-testid="agent-pm" className="truncate font-mono text-[10px] leading-tight text-ink-mute">
          {pm}
        </div>
      )}
    </div>
  );
}
