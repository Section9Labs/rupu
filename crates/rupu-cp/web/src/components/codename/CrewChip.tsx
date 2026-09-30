import { cn } from '../../lib/cn';
import { crewTint } from '../../lib/codename';
import { useThemeMode } from './useThemeMode';

export const DERIVED_TITLE = 'derived for a run recorded before codenames';

export function CrewChip({ crew, derived }: { crew: string; derived?: boolean }) {
  const mode = useThemeMode();
  const tint = crewTint(crew, mode);
  return (
    <span
      className={cn('inline-flex items-center gap-1 font-mono text-meta', derived && 'opacity-60')}
      title={derived ? DERIVED_TITLE : crew}
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
