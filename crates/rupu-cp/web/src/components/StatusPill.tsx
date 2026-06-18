// Run-status visuals shared across the Runs list and the Run detail header.
// Single source of truth so a status looks identical wherever it's rendered.
//
// Ported from Okesu's StatusPill (same visual language: rounded ring pill +
// icon + label, and a small status dot), remapped from Okesu's finding-status
// enum to rupu's run statuses + step states.

import {
  AlertCircle,
  CheckCircle2,
  Clock,
  Loader2,
  Pause,
  SkipForward,
  XCircle,
  XOctagon,
  type LucideIcon,
} from 'lucide-react';
import type { RunStatusStr } from '../lib/api';
import { cn } from '../lib/cn';

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

export const RUN_STATUS_STYLES: Record<RunStatusStr, StatusStyle> = {
  pending: {
    label: 'Pending',
    cls: 'bg-slate-100 text-slate-700 ring-slate-200',
    dot: 'bg-slate-400',
    icon: Clock,
  },
  running: {
    label: 'Running',
    cls: 'bg-blue-50 text-blue-700 ring-blue-200',
    dot: 'bg-blue-500',
    icon: Loader2,
  },
  completed: {
    label: 'Completed',
    cls: 'bg-green-50 text-green-700 ring-green-200',
    dot: 'bg-green-500',
    icon: CheckCircle2,
  },
  failed: {
    label: 'Failed',
    cls: 'bg-red-50 text-red-700 ring-red-200',
    dot: 'bg-red-500',
    icon: XCircle,
  },
  awaiting_approval: {
    label: 'Awaiting approval',
    cls: 'bg-amber-50 text-amber-800 ring-amber-200',
    dot: 'bg-amber-500',
    icon: Pause,
  },
  rejected: {
    label: 'Rejected',
    cls: 'bg-red-50 text-red-700 ring-red-200',
    dot: 'bg-red-500',
    icon: XOctagon,
  },
};

export const STEP_STATE_STYLES: Record<StepState, StatusStyle> = {
  pending: {
    label: 'Pending',
    cls: 'bg-slate-100 text-slate-600 ring-slate-200',
    dot: 'bg-slate-400',
    icon: Clock,
  },
  running: {
    label: 'Running',
    cls: 'bg-blue-50 text-blue-700 ring-blue-200',
    dot: 'bg-blue-500',
    icon: Loader2,
  },
  completed: {
    label: 'Completed',
    cls: 'bg-green-50 text-green-700 ring-green-200',
    dot: 'bg-green-500',
    icon: CheckCircle2,
  },
  failed: {
    label: 'Failed',
    cls: 'bg-red-50 text-red-700 ring-red-200',
    dot: 'bg-red-500',
    icon: XCircle,
  },
  awaiting_approval: {
    label: 'Awaiting',
    cls: 'bg-amber-50 text-amber-800 ring-amber-200',
    dot: 'bg-amber-500',
    icon: Pause,
  },
  skipped: {
    label: 'Skipped',
    cls: 'bg-slate-100 text-slate-600 ring-slate-200',
    dot: 'bg-slate-400',
    icon: SkipForward,
  },
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
