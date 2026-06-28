// Canonical status descriptor map — the SINGLE source of truth for run-status
// and step-state visuals across the whole CP UI.
//
// Before this existed there were TWO diverging palettes:
//   • pills / timeline / session dots → Tailwind named colors
//     (running blue-500, done green-500, failed red-500, awaiting amber-500)
//   • the run-graph → a bespoke set (#1860f2 / #2ac769 / #fb4e4e)
// Everything now reads from here. Tailwind tokens live in `tailwind.config.ts`
// under `colors.status.*`; `src/styles.css` carries the same literal hexes for
// the pulse-ring / edge animations (CSS can't import TS).
//
// The graph model uses the state name `done`; pills use `completed`. Both
// resolve to the SAME descriptor via the done↔completed alias.

import {
  Ban,
  CheckCircle2,
  Clock,
  Loader2,
  Pause,
  SkipForward,
  XCircle,
  XOctagon,
  type LucideIcon,
} from 'lucide-react';
import type { RunStatusStr } from './api';

/** Canonical status keys. `done` is an alias of `completed` (see below). */
export type StatusKey =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'awaiting_approval'
  | 'rejected'
  | 'cancelled'
  | 'skipped';

export interface StatusDescriptor {
  /** Capitalized human label (used by pills). */
  label: string;
  /** 500-level primary color — for xyflow inline styles (borders / glyph fills)
   *  where Tailwind class names can't reach. Matches `status.*` in tailwind. */
  hex: string;
  /** Soft light-bg tint hex — for graph cards / chip fills. */
  tint: string;
  /** Icon for the status. */
  icon: LucideIcon;
  /** Status-dot Tailwind class, e.g. `bg-status-running`. */
  dotClass: string;
  /** Pill bg/text/ring Tailwind combo. */
  pillClass: string;
}

export const STATUS: Record<StatusKey, StatusDescriptor> = {
  pending: {
    label: 'Pending',
    hex: '#94a3b8',
    tint: '#f8fafc',
    icon: Clock,
    dotClass: 'bg-status-pending',
    pillClass: 'bg-slate-100 text-slate-700 ring-slate-200',
  },
  running: {
    label: 'Running',
    hex: '#3b82f6',
    tint: '#eff6ff',
    icon: Loader2,
    dotClass: 'bg-status-running',
    pillClass: 'bg-blue-50 text-blue-700 ring-blue-200',
  },
  completed: {
    label: 'Completed',
    hex: '#22c55e',
    tint: '#f0fdf4',
    icon: CheckCircle2,
    dotClass: 'bg-status-done',
    pillClass: 'bg-green-50 text-green-700 ring-green-200',
  },
  failed: {
    label: 'Failed',
    hex: '#ef4444',
    tint: '#fef2f2',
    icon: XCircle,
    dotClass: 'bg-status-failed',
    pillClass: 'bg-red-50 text-red-700 ring-red-200',
  },
  awaiting_approval: {
    label: 'Awaiting approval',
    hex: '#f59e0b',
    tint: '#fffbeb',
    icon: Pause,
    dotClass: 'bg-status-awaiting',
    pillClass: 'bg-amber-50 text-amber-800 ring-amber-200',
  },
  rejected: {
    label: 'Rejected',
    hex: '#ef4444',
    tint: '#fef2f2',
    icon: XOctagon,
    dotClass: 'bg-status-rejected',
    pillClass: 'bg-red-50 text-red-700 ring-red-200',
  },
  cancelled: {
    label: 'Cancelled',
    hex: '#64748b',
    tint: '#f1f5f9',
    icon: Ban,
    dotClass: 'bg-status-cancelled',
    pillClass: 'bg-slate-100 text-slate-700 ring-slate-300',
  },
  skipped: {
    label: 'Skipped',
    hex: '#cbd5e1',
    tint: '#f1f5f9',
    icon: SkipForward,
    dotClass: 'bg-status-skipped',
    pillClass: 'bg-slate-100 text-slate-600 ring-slate-200',
  },
};

/**
 * Accepted step-state inputs. The run-graph model emits `done`; the pill layer
 * speaks `completed`. Both are accepted and normalize to the same descriptor.
 */
export type StepStateInput =
  | 'pending'
  | 'running'
  | 'awaiting_approval'
  | 'done'
  | 'completed'
  | 'failed'
  | 'skipped';

/** Resolve any status/step token to its canonical key (done → completed). */
export function normalizeStatusKey(s: string): StatusKey {
  return (s === 'done' ? 'completed' : s) as StatusKey;
}

/** Descriptor for a run status. */
export function runStatusStyle(s: RunStatusStr): StatusDescriptor {
  return STATUS[s];
}

/** Descriptor for a step state (graph `done` or pill `completed` both work). */
export function stepStateStyle(s: StepStateInput): StatusDescriptor {
  return STATUS[normalizeStatusKey(s)];
}
