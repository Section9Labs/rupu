import { Users, X } from 'lucide-react';
import type { SpaceParticipant } from '../../lib/api';
import { hexA, identityOf } from '../../lib/messages';
import { cn } from '../../lib/cn';
import { useThemeMode } from '../codename/useThemeMode';
import { Avatar } from './Avatar';

/** Everyone in the space, live first. Clicking one filters the feed to the
 *  messages involving it; clicking it again (or ×) clears the filter. */
export function ParticipantsBar({
  participants,
  involving,
  onInvolving,
}: {
  participants: SpaceParticipant[];
  involving: string | null;
  onInvolving: (address: string | null) => void;
}) {
  const mode = useThemeMode();
  if (participants.length === 0) return null;
  const sorted = [...participants].sort((a, b) => Number(b.live) - Number(a.live));
  return (
    <div data-testid="participants-bar" className="flex flex-wrap items-center gap-1.5">
      <span className="mr-0.5 inline-flex items-center gap-1 text-meta font-medium text-ink-mute">
        <Users size={12} />
        {participants.filter((p) => p.live).length} live · {participants.length}
      </span>
      {sorted.map((p) => {
        const { color } = identityOf(p.address, mode);
        const active = involving === p.address;
        return (
          <button
            type="button"
            key={p.address}
            data-testid="participant"
            data-live={p.live}
            aria-pressed={active}
            onClick={() => onInvolving(active ? null : p.address)}
            className={cn('inline-flex items-center gap-1.5 rounded-full py-0.5 pl-0.5 pr-2 transition-shadow', !p.live && 'opacity-55')}
            style={{ background: hexA(color, active ? 0.22 : 0.09), boxShadow: active ? `inset 0 0 0 1px ${hexA(color, 0.6)}` : undefined }}
            title={[p.codename ?? p.address, p.agent, p.step_id && `step ${p.step_id}`, p.live ? 'live' : 'left'].filter(Boolean).join(' · ')}
          >
            <Avatar address={p.address} size={18} />
            <span className="text-meta font-medium" style={{ color }}>
              {p.address}
            </span>
            <span className="text-[10px] text-ink-mute">{p.agent}</span>
            {p.live ? <span className="h-1.5 w-1.5 rounded-full bg-ok" aria-label="live" /> : <span className="text-[10px] text-ink-mute">left</span>}
          </button>
        );
      })}
      {involving && (
        <button type="button" onClick={() => onInvolving(null)} className="inline-flex items-center gap-0.5 text-meta text-ink-mute hover:text-ink">
          <X size={11} />
          involving {involving}
        </button>
      )}
    </div>
  );
}
