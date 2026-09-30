import { cn } from '../../lib/cn';
import { identityTitle, memberLabel, parseCodename } from '../../lib/codename';
import { CrewChip, derivedTitle } from './CrewChip';
import { RoleBadge } from './RoleBadge';

export interface AgentNameProps {
  /** Server-minted codename. Falsy (legacy row, missing field) renders nothing. */
  codename: string | undefined | null;
  agent?: string;
  provider?: string;
  model?: string;
  showCrew?: boolean;
  derived?: boolean;
}

export function AgentName({ codename, agent, provider, model, showCrew, derived }: AgentNameProps) {
  if (!codename) return null;
  const { crew, role } = parseCodename(codename);
  // Hover shows the full identity (whole codename, crew included).
  const title = derivedTitle(identityTitle(codename, agent, provider, model), derived);
  // A crew-only codename (e.g. a derived legacy `jade-reef`) has no member
  // segment: its leaf IS the crew, so with showCrew the chip alone names it —
  // rendering the leaf label too would print the crew twice.
  if (showCrew && !codename.includes('/')) {
    const rest = memberLabel(undefined, agent, provider, model);
    return (
      <span className="inline-flex items-center gap-1.5" title={title}>
        <CrewChip crew={crew} derived={derived} />
        {rest && <span className={cn('font-mono', derived && 'opacity-60')}>{rest}</span>}
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5">
      {showCrew && <CrewChip crew={crew} derived={derived} />}
      <span
        className={cn('inline-flex items-center gap-1', derived && 'opacity-60')}
        title={title}
      >
        {role && <RoleBadge role={role} />}
        <span className="font-mono">{memberLabel(codename, agent, provider, model)}</span>
      </span>
    </span>
  );
}
