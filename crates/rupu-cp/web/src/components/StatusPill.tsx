// Run-status visuals shared across the Runs list and the Run detail header.
// Single source of truth so a status looks identical wherever it's rendered.
//
// Ported from Okesu's StatusPill (same visual language: rounded ring pill +
// icon + label, and a small status dot), remapped from Okesu's finding-status
// enum to rupu's run statuses + step states.

import { AlertCircle, type LucideIcon } from 'lucide-react';
import type { RunStatusStr } from '../lib/api';
import { cn } from '../lib/cn';
import { sessionStatusLabel, sessionStatusTone, SESSION_STATUS_DESCRIPTOR } from '../lib/sessionStatus';
import { STATUS, statusMotionClass, type StatusDescriptor } from '../lib/status';

// Live step state derived from the static record + SSE overrides. Kept in one
// place so RunGraph and the timeline agree on the vocabulary.
export type StepState =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'awaiting_approval'
  | 'skipped';

interface StatusStyle {
  label: string;
  cls: string; // pill background/text/ring
  dot: string; // small dot color
  icon: LucideIcon;
}

// Colors / labels / icons all come from the canonical descriptor map
// (`lib/status.ts`) so pills, timeline, graph, and dots never diverge.
function toPillStyle(d: StatusDescriptor): StatusStyle {
  return { label: d.label, cls: d.pillClass, dot: d.dotClass, icon: d.icon };
}

export const RUN_STATUS_STYLES: Record<RunStatusStr, StatusStyle> = {
  pending: toPillStyle(STATUS.pending),
  running: toPillStyle(STATUS.running),
  completed: toPillStyle(STATUS.completed),
  failed: toPillStyle(STATUS.failed),
  awaiting_approval: toPillStyle(STATUS.awaiting_approval),
  paused: toPillStyle(STATUS.paused),
  rejected: toPillStyle(STATUS.rejected),
  cancelled: toPillStyle(STATUS.cancelled),
};

export const STEP_STATE_STYLES: Record<StepState, StatusStyle> = {
  pending: toPillStyle(STATUS.pending),
  running: toPillStyle(STATUS.running),
  completed: toPillStyle(STATUS.completed),
  failed: toPillStyle(STATUS.failed),
  // Step pills use the short 'Awaiting' label (the run pill uses the longer
  // 'Awaiting approval'); same color/icon, sourced from the descriptor.
  awaiting_approval: { ...toPillStyle(STATUS.awaiting_approval), label: 'Awaiting' },
  skipped: toPillStyle(STATUS.skipped),
};

// Shared rendering for every pill flavor (run-status and session-status
// alike) — one markup path so `StatusPill` and `SessionStatusPill` are
// byte-identical apart from which descriptor + motion class they're given.
// `motion` is a CSS class name (`rg-pulse-run` / `rg-pulse-await`, both
// reduced-motion guarded in `styles.css`) or `undefined` for a static pill;
// it doubles as the `data-motion` marker tests assert on.
function PillShell({
  style,
  size,
  motion,
}: {
  style: StatusStyle;
  size: 'xs' | 'sm';
  motion?: 'rg-pulse-run' | 'rg-pulse-await';
}) {
  const Icon = style.icon;
  return (
    <span
      data-motion={motion}
      className={cn(
        'inline-flex items-center gap-1 rounded ring-1 font-medium tabular-nums whitespace-nowrap',
        style.cls,
        motion,
        size === 'xs'
          ? 'text-meta uppercase tracking-wide px-1.5 py-0.5'
          : 'text-note px-2 py-0.5',
      )}
    >
      <Icon size={size === 'xs' ? 9 : 11} />
      {style.label}
    </span>
  );
}

export function StatusPill({
  status,
  size = 'sm',
}: {
  status: RunStatusStr;
  size?: 'xs' | 'sm';
}) {
  const s = RUN_STATUS_STYLES[status] ?? {
    label: status,
    cls: 'bg-surface text-ink ring-border',
    dot: 'bg-ink-mute',
    icon: AlertCircle,
  };
  return <PillShell style={s} size={size} motion={statusMotionClass(status)} />;
}

// Sessions speak a distinct 4-value vocabulary (idle/running/failed/stopped —
// see `lib/sessionStatus.ts`), never renamed into run-status words. Routing
// through the SAME `PillShell` as `StatusPill` is what makes a session
// `running` render the identical motion marker as a run `running` (Task 2).
//
// `status` is `unknown` (the wire type the session summary carries — see
// `lib/sessionStatus.ts`'s doc comment). `sessionStatusTone` does the same
// forgiving normalization every other session-status consumer already relies
// on (exact vocabulary match, then substring heuristics for looser wire
// values like `"active"`) — that part is a deliberate, tested mapping, not
// the bug. The bug was its `neutral` catch-all (truly unrecognized input:
// null, absent, garbage) being folded onto `stopped`, presenting a confident
// but fabricated state. For `neutral` this renders the RAW label with a
// neutral tone and an AlertCircle icon instead, mirroring `StatusPill`'s own
// unrecognized-status fallback below — never inventing a real state.
export function SessionStatusPill({
  status,
  size = 'sm',
}: {
  status: unknown;
  size?: 'xs' | 'sm';
}) {
  const tone = sessionStatusTone(status);
  if (tone !== 'neutral') {
    const s = toPillStyle(SESSION_STATUS_DESCRIPTOR[tone]);
    return <PillShell style={s} size={size} motion={tone === 'running' ? 'rg-pulse-run' : undefined} />;
  }
  const s: StatusStyle = {
    label: sessionStatusLabel(status),
    cls: 'bg-surface text-ink ring-border',
    dot: 'bg-ink-mute',
    icon: AlertCircle,
  };
  return <PillShell style={s} size={size} />;
}

export function StatusDot({
  status,
  className,
}: {
  status: RunStatusStr;
  className?: string;
}) {
  const s = RUN_STATUS_STYLES[status];
  return (
    <span
      className={cn('inline-block w-1.5 h-1.5 rounded-full', s ? s.dot : 'bg-ink-mute', className)}
    />
  );
}
