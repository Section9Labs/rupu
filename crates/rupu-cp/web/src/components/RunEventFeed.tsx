// rupu-native live event timeline — a scrolling vertical feed of a run's SSE
// events. Built (not the 1370-line Okesu EventTimeline port), but reuses
// Okesu's `.timeline-*` CSS animations from styles.css and the same lucide
// icon + colored-dot visual language.
//
// The SSE subscription lives ONE level up in RunDetail (shared with RunGraph),
// so this component is a pure render of the accumulated event list plus a small
// connection indicator. Auto-scrolls to newest unless the user has scrolled up.

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
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

export type ConnectionState = 'connecting' | 'live' | 'reconnecting';

/** A RunEvent paired with a monotonically-increasing sequence id for stable React keys. */
export interface SeqEvent {
  seq: number;
  event: RunEvent;
}

interface Visual {
  icon: LucideIcon;
  ring: string; // dot ring/bg
  iconColor: string;
  spin?: boolean;
}

const UNKNOWN_VISUAL: Visual = {
  icon: Circle,
  ring: 'bg-slate-100 ring-slate-200',
  iconColor: 'text-slate-400',
};

function visualFor(ev: KnownRunEvent): Visual {
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
    case 'unit_completed':
      return { icon: Circle, ring: 'bg-slate-100 ring-slate-200', iconColor: 'text-slate-400' };
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

function labelFor(ev: RunEvent): string {
  return ev.type.replace(/_/g, ' ');
}

export default function RunEventFeed({
  events,
  connection,
}: {
  events: SeqEvent[];
  connection: ConnectionState;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Whether to follow the tail. Flips false when the user scrolls up; back to
  // true when they scroll (near) the bottom.
  const [follow, setFollow] = useState(true);

  // Stick to the bottom on new events while following.
  useLayoutEffect(() => {
    if (!follow) return;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [events.length, follow]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
      setFollow(nearBottom);
    };
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
  }, []);

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center justify-between px-1 pb-2">
        <ConnectionBadge state={connection} />
        <span className="text-[11px] text-ink-mute tabular-nums">
          {events.length} event{events.length === 1 ? '' : 's'}
        </span>
      </div>

      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-y-auto rounded-xl border border-border bg-panel shadow-card"
      >
        {events.length === 0 ? (
          <div className="p-8 text-center text-sm text-ink-dim">
            Waiting for events…
          </div>
        ) : (
          <ol className="timeline-list relative py-3 pl-5 pr-4">
            {events.map(({ seq, event: ev }, i) => {
              const known = isKnownRunEvent(ev);
              const v = known ? visualFor(ev) : UNKNOWN_VISUAL;
              const Icon = v.icon;
              const detail = known ? detailFor(ev) : undefined;
              const title = known ? titleFor(ev) : ev.type;
              const isLast = i === events.length - 1;
              return (
                <li
                  key={seq}
                  className="timeline-item relative flex gap-3 pb-3 last:pb-0"
                >
                  {/* connector line */}
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
                    )}
                  >
                    <Icon size={12} className={cn(v.iconColor, v.spin && 'animate-spin')} />
                  </span>
                  <div className="min-w-0 flex-1 pt-0.5">
                    <div className="flex items-baseline gap-2">
                      <span className="text-xs font-medium text-ink truncate">
                        {title}
                      </span>
                      <span className="text-[10px] uppercase tracking-wide text-ink-mute shrink-0">
                        {labelFor(ev)}
                      </span>
                    </div>
                    {detail && (
                      <div className="text-[11px] text-ink-dim mt-0.5 break-words">
                        {detail}
                      </div>
                    )}
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </div>
    </div>
  );
}

function ConnectionBadge({ state }: { state: ConnectionState }) {
  const map: Record<ConnectionState, { label: string; dot: string; text: string }> = {
    connecting: { label: 'Connecting', dot: 'bg-amber-500', text: 'text-amber-700' },
    live: { label: 'Live', dot: 'bg-green-500', text: 'text-green-700' },
    reconnecting: { label: 'Reconnecting', dot: 'bg-amber-500', text: 'text-amber-700' },
  };
  const m = map[state];
  return (
    <span className={cn('inline-flex items-center gap-1.5 text-[11px] font-medium', m.text)}>
      <span className={cn('inline-block w-1.5 h-1.5 rounded-full', m.dot, state !== 'live' && 'animate-pulse')} />
      {m.label}
    </span>
  );
}
