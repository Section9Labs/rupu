// Pure presentational timeline list — renders a SeqEvent[] as a vertical
// timeline (icon/color per event type, connector lines, title + detail).
// No subscription, no scroll state. Consumed by RunEventFeed (which adds
// scroll-follow + connection badge) and by the Events page (global stream).
//
// Props:
//   liveIDs — optional Set<number> of seq IDs that are "fresh" (just arrived).
//             When a row's seq is in this set, is-fresh / is-live CSS classes
//             apply the tick-in animation + purple stripe.
//
// Newest-first ordering is handled by the caller (Events.tsx prepends new
// events so index 0 is always the latest). EventTimelineList is order-agnostic
// — it renders whatever it receives, spine connecting each item to the next.

import {
  AlertCircle,
  CheckCircle2,
  Circle,
  Cog,
  Loader2,
  Pause,
  PlayCircle,
  SkipForward,
  XCircle,
  type LucideIcon,
} from 'lucide-react';
import { cn } from '../lib/cn';
import { isKnownRunEvent, type KnownRunEvent, type RunEvent } from '../lib/api';
import { formatDurationMs } from '../lib/time';
import { type SeqEvent } from './RunEventFeed';

interface Visual {
  icon: LucideIcon;
  ring: string;
  iconColor: string;
  spin?: boolean;
}

const UNKNOWN_VISUAL: Visual = {
  icon: Circle,
  ring: 'bg-slate-100 ring-slate-200',
  iconColor: 'text-slate-400',
};

export function visualFor(ev: KnownRunEvent): Visual {
  switch (ev.type) {
    case 'run_started':
      return { icon: PlayCircle, ring: 'bg-brand-50 ring-brand-200', iconColor: 'text-brand-600' };
    case 'step_started':
      return { icon: Cog, ring: 'bg-blue-50 ring-blue-200', iconColor: 'text-blue-600' };
    case 'step_working':
      return { icon: Loader2, ring: 'bg-blue-50 ring-blue-200', iconColor: 'text-blue-600', spin: true };
    case 'step_completed':
      return ev.success
        ? { icon: CheckCircle2, ring: 'bg-green-50 ring-green-200', iconColor: 'text-green-600' }
        : { icon: XCircle, ring: 'bg-red-50 ring-red-200', iconColor: 'text-red-600' };
    case 'step_failed':
      return { icon: XCircle, ring: 'bg-red-50 ring-red-200', iconColor: 'text-red-600' };
    case 'step_awaiting_approval':
      return { icon: Pause, ring: 'bg-amber-50 ring-amber-200', iconColor: 'text-amber-600' };
    case 'step_skipped':
      return { icon: SkipForward, ring: 'bg-slate-100 ring-slate-200', iconColor: 'text-slate-400' };
    case 'unit_started':
      return { icon: PlayCircle, ring: 'bg-blue-50 ring-blue-200', iconColor: 'text-blue-600' };
    case 'unit_completed':
      return ev.success
        ? { icon: CheckCircle2, ring: 'bg-green-50 ring-green-200', iconColor: 'text-green-600' }
        : { icon: XCircle, ring: 'bg-red-50 ring-red-200', iconColor: 'text-red-600' };
    case 'run_completed':
      return { icon: CheckCircle2, ring: 'bg-green-50 ring-green-200', iconColor: 'text-green-600' };
    case 'run_failed':
      return { icon: AlertCircle, ring: 'bg-red-50 ring-red-200', iconColor: 'text-red-600' };
    default:
      return { icon: Circle, ring: 'bg-slate-100 ring-slate-200', iconColor: 'text-slate-400' };
  }
}

function titleFor(ev: KnownRunEvent): string {
  switch (ev.type) {
    case 'run_started':
      return 'Run started';
    case 'run_completed':
      return 'Run completed';
    case 'run_failed':
      return 'Run failed';
    case 'unit_started':
      return `${ev.step_id} · unit ${ev.unit_key}`;
    case 'unit_completed':
      return `${ev.step_id} · unit ${ev.unit_key}`;
    default:
      return 'step_id' in ev && typeof ev.step_id === 'string' ? ev.step_id : ev.type;
  }
}

function detailFor(ev: KnownRunEvent): string | undefined {
  switch (ev.type) {
    case 'run_started':
      return ev.workflow_path;
    case 'step_started':
      return ev.agent ? `agent ${ev.agent}` : ev.kind;
    case 'step_working':
      return ev.note ?? undefined;
    case 'step_awaiting_approval':
      return ev.reason;
    case 'step_completed':
      return `${ev.success ? 'ok' : 'failed'} · ${formatDurationMs(ev.duration_ms)}`;
    case 'step_failed':
      return ev.error;
    case 'step_skipped':
      return ev.reason;
    case 'unit_started':
      return ev.agent ? `agent ${ev.agent}` : undefined;
    case 'unit_completed':
      return `${ev.success ? 'ok' : 'failed'} · ${ev.tokens_in}→${ev.tokens_out} tok`;
    case 'run_failed':
      return ev.error;
    default:
      return undefined;
  }
}

export function labelFor(ev: RunEvent): string {
  return ev.type.replace(/_/g, ' ');
}

/** Relative "time ago" string from a RunEvent's timestamp field, if present. */
function relTs(ev: RunEvent): string | undefined {
  const raw = ev as Record<string, unknown>;
  const ts = raw['ts'] ?? raw['timestamp'];
  if (typeof ts !== 'number' && typeof ts !== 'string') return undefined;
  const t = typeof ts === 'number' ? ts : Date.parse(ts);
  if (Number.isNaN(t)) return undefined;
  const sec = Math.round((Date.now() - t) / 1000);
  if (sec < 5) return 'just now';
  if (sec < 60) return `${sec}s ago`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.round(hr / 24)}d ago`;
}

/**
 * Pure presentational timeline — renders a list of SeqEvents as a vertical
 * icon + text feed. No subscription, no scroll state, no connection badge.
 *
 * @param liveIDs  Optional set of seq IDs to mark as "fresh" (tick animation).
 *                 The Events page passes this; RunEventFeed omits it.
 */
export default function EventTimelineList({
  events,
  className,
  liveIDs,
}: {
  events: SeqEvent[];
  className?: string;
  liveIDs?: ReadonlySet<number>;
}) {
  return (
    <ol className={cn('timeline-list relative py-3 pl-5 pr-4', className)}>
      {events.map(({ seq, event: ev }, i) => {
        const known = isKnownRunEvent(ev);
        const v = known ? visualFor(ev) : UNKNOWN_VISUAL;
        const Icon = v.icon;
        const detail = known ? detailFor(ev) : undefined;
        const title = known ? titleFor(ev) : ev.type;
        const rel = relTs(ev);
        const isLast = i === events.length - 1;
        const isFresh = liveIDs != null && liveIDs.has(seq);
        return (
          <li
            key={seq}
            className={cn(
              'timeline-item relative flex gap-3 pb-3 last:pb-0',
              isFresh && 'is-fresh',
            )}
          >
            {/* connector line — below the dot for oldest-first, below for newest-first too
                (the spine always runs "downward" toward the older/next item) */}
            {!isLast && (
              <span
                className="absolute left-[11px] top-6 bottom-0 w-px bg-border"
                aria-hidden
              />
            )}
            <span
              className={cn(
                'timeline-dot relative z-10 flex h-6 w-6 shrink-0 items-center justify-center rounded-full ring-1',
                v.ring,
                isFresh && 'is-live',
              )}
            >
              {isFresh && (
                <span
                  className={cn('timeline-dot-ring absolute inline-block w-6 h-6 rounded-full', v.ring)}
                  aria-hidden
                />
              )}
              <Icon size={12} className={cn(v.iconColor, v.spin && 'animate-spin')} />
            </span>
            <div className="min-w-0 flex-1 pt-0.5">
              <div className="flex items-baseline gap-2">
                <span className="text-xs font-medium text-ink truncate">
                  {title}
                </span>
                <span className="text-meta uppercase tracking-wide text-ink-mute shrink-0">
                  {labelFor(ev)}
                </span>
                {rel && (
                  <span className="ml-auto text-meta text-ink-mute tabular-nums shrink-0">
                    {rel}
                  </span>
                )}
              </div>
              {detail && (
                <div className="text-note text-ink-dim mt-0.5 break-words">
                  {detail}
                </div>
              )}
            </div>
          </li>
        );
      })}
    </ol>
  );
}
