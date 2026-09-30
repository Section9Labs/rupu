import { cn } from '../../lib/cn';
import { memberLabel, parseCodename } from '../../lib/codename';
import { CrewChip, DERIVED_TITLE } from './CrewChip';
import { RoleBadge } from './RoleBadge';

export interface AgentNameProps {
  codename: string;
  agent?: string;
  provider?: string;
  model?: string;
  showCrew?: boolean;
  derived?: boolean;
}

export function AgentName({ codename, agent, provider, model, showCrew, derived }: AgentNameProps) {
  const { crew, role } = parseCodename(codename);
  return (
    <span className="inline-flex items-center gap-1.5">
      {showCrew && <CrewChip crew={crew} derived={derived} />}
      <span
        className={cn('inline-flex items-center gap-1', derived && 'opacity-60')}
        title={derived ? `${codename} (${DERIVED_TITLE})` : codename}
      >
        {role && <RoleBadge role={role} />}
        <span className="font-mono">{memberLabel(codename, agent, provider, model)}</span>
      </span>
    </span>
  );
}
