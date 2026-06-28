// Run-status visuals shared across the Runs list and the Run detail header.
// Single source of truth so a status looks identical wherever it's rendered.
//
// Ported from Okesu's StatusPill (same visual language: rounded ring pill +
// icon + label, and a small status dot), remapped from Okesu's finding-status
// enum to rupu's run statuses + step states.

import { AlertCircle, type LucideIcon } from 'lucide-react';
import type { RunStatusStr } from '../lib/api';
import { cn } from '../lib/cn';
import { STATUS, type StatusDescriptor } from '../lib/status';

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

export function StatusPill({
  status,
  size = 'sm',
}: {
  status: RunStatusStr;
  size?: 'xs' | 'sm';
}) {
  const s = RUN_STATUS_STYLES[status] ?? {
    label: status,
    cls: 'bg-slate-100 text-slate-700 ring-slate-200',
    dot: 'bg-slate-400',
    icon: AlertCircle,
  };
  const Icon = s.icon;
  const spin = status === 'running';
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded ring-1 font-medium tabular-nums',
        s.cls,
        size === 'xs'
          ? 'text-[10px] uppercase tracking-wide px-1.5 py-0.5'
          : 'text-[11px] px-2 py-0.5',
      )}
    >
      <Icon size={size === 'xs' ? 9 : 11} className={spin ? 'animate-spin' : undefined} />
      {s.label}
    </span>
  );
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
      className={cn('inline-block w-1.5 h-1.5 rounded-full', s ? s.dot : 'bg-slate-400', className)}
    />
  );
}
