// Derives per-step live status by merging the static persisted
// `StepResultRecord` with overrides accumulated from the SSE event stream.
//
// Single source of truth so the RunGraph (nodes) and RunDetail (any future
// consumers) agree on what state each step is in.

import { isKnownRunEvent, type RunEvent, type StepResultRecord } from './api';
import type { StepState } from '../components/StatusPill';

export interface AwaitingInfo {
  stepId: string;
  reason: string;
}

/** Live-override map: stepId → StepState, plus a single awaiting pointer. */
export interface RunStatusState {
  byStep: Record<string, StepState>;
  awaiting?: AwaitingInfo;
}

export function emptyRunStatus(): RunStatusState {
  return { byStep: {} };
}

/** Fold a single SSE event into the running status map (immutably). */
export function reduceRunStatus(prev: RunStatusState, ev: RunEvent): RunStatusState {
  // Only the fully-typed event variants carry per-step status transitions.
  if (!isKnownRunEvent(ev)) return prev;
  // Events without a step_id (run_started / run_completed / run_failed) do not
  // touch per-step status here.
  const stepId =
    'step_id' in ev && typeof ev.step_id === 'string' ? ev.step_id : undefined;
  if (!stepId) return prev;

  const byStep = { ...prev.byStep };
  let awaiting = prev.awaiting;

  switch (ev.type) {
    case 'step_started':
    case 'step_working':
    case 'unit_started':
      byStep[stepId] = 'running';
      break;
    case 'step_awaiting_approval':
      byStep[stepId] = 'awaiting_approval';
      awaiting = { stepId, reason: ev.reason };
      break;
    case 'step_completed':
      byStep[stepId] = ev.success ? 'completed' : 'failed';
      if (awaiting?.stepId === stepId) awaiting = undefined;
      break;
    case 'step_failed':
      byStep[stepId] = 'failed';
      if (awaiting?.stepId === stepId) awaiting = undefined;
      break;
    case 'step_skipped':
      byStep[stepId] = 'skipped';
      if (awaiting?.stepId === stepId) awaiting = undefined;
      break;
    default:
      break;
  }
  return { byStep, awaiting };
}

/**
 * Resolve a step's display state: live SSE override wins, otherwise fall back
 * to the static record (skipped / success flags), otherwise pending.
 */
export function resolveStepState(
  step: StepResultRecord,
  live: RunStatusState,
): StepState {
  const override = live.byStep[step.step_id];
  if (override) return override;
  if (step.skipped) return 'skipped';
  if (step.success === true) return 'completed';
  if (step.success === false) return 'failed';
  return 'pending';
}
