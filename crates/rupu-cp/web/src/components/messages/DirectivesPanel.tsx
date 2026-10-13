import { useState } from 'react';
import { ChevronDown, ChevronRight, Pin } from 'lucide-react';
import type { Directive } from '../../lib/messages';
import { absoluteTime, relativeTime } from '../../lib/time';
import { cn } from '../../lib/cn';
import { useThemeMode } from '../codename/useThemeMode';
import { Badge } from '../ui/Badge';
import { AddressPill, Avatar } from './Avatar';
import { renderBody } from './MessageRow';

/** Standing directives pinned above the feed; retracted ones fold under a
 *  toggle, struck through, with who lifted them. */
export function DirectivesPanel({ directives, addresses }: { directives: Directive[]; addresses: string[] }) {
  const mode = useThemeMode();
  const [showLifted, setShowLifted] = useState(false);
  if (directives.length === 0) return null;
  const standing = directives.filter((d) => !d.retractedBy);
  const lifted = directives.filter((d) => d.retractedBy);
  const shown = showLifted ? directives : standing;
  return (
    <div data-testid="directives-panel">
      <div className="mb-1.5 flex items-center gap-1.5 text-note font-medium text-ink-dim">
        <Pin size={13} className="text-amber-500" />
        Pinned directives
        <span className="text-meta tabular-nums text-ink-mute">· {standing.length} standing</span>
        {lifted.length > 0 && (
          <button type="button" onClick={() => setShowLifted((v) => !v)} className="ml-auto inline-flex items-center gap-0.5 text-meta text-ink-mute hover:text-ink">
            {showLifted ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            {lifted.length} retracted
          </button>
        )}
      </div>
      <div className="flex flex-col gap-1.5">
        {shown.map(({ message: d, retractedBy }) => (
          <div key={d.id} data-testid="directive" className={cn('rounded-lg border px-3 py-1.5', retractedBy ? 'border-border opacity-60' : 'border-amber-500/40 bg-amber-500/5')}>
            <div className="flex items-center gap-2">
              <Avatar address={d.from} size={18} />
              <span className="text-note font-medium text-ink">{d.from}</span>
              {d.to && <span className="text-meta text-ink-mute">→ <AddressPill address={d.to} /></span>}
              {retractedBy ? <Badge tone="neutral">retracted by {retractedBy.from}</Badge> : <Badge tone="amber">standing</Badge>}
              <span className="ml-auto text-meta text-ink-mute" title={absoluteTime(d.ts)}>
                {relativeTime(d.ts)}
              </span>
            </div>
            <p className={cn('mt-0.5 whitespace-pre-wrap break-words text-sm leading-relaxed text-ink-dim', retractedBy && 'line-through')}>{retractedBy ? d.body : renderBody(d.body, addresses, mode)}</p>
          </div>
        ))}
      </div>
    </div>
  );
}
