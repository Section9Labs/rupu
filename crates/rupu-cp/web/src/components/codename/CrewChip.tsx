import { cn } from '../../lib/cn';
import { crewTint } from '../../lib/codename';
import { useThemeMode } from './useThemeMode';

export const DERIVED_TITLE = 'derived for a run recorded before codenames';

/** Tooltip text for a (possibly derived) name — shared with AgentName so the
 *  two agree on how "derived" is explained. */
export function derivedTitle(name: string, derived?: boolean): string {
  return derived ? `${name} (${DERIVED_TITLE})` : name;
}

export function CrewChip({ crew, derived }: { crew: string | undefined | null; derived?: boolean }) {
  const mode = useThemeMode();
  if (!crew) return null;
  const tint = crewTint(crew, mode);
  return (
    <span
      className={cn('inline-flex items-center gap-1 font-mono text-meta', derived && 'opacity-60')}
      title={derivedTitle(crew, derived)}
    >
      <span
        aria-hidden
        className="inline-block h-2 w-2 rounded-full bg-ink-mute"
        style={tint ? { backgroundColor: tint } : undefined}
      />
      {crew}
    </span>
  );
}
