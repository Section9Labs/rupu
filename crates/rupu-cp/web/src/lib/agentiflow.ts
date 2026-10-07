// Display helpers for the Agentiflows views (list tab + detail page). Pure
// functions over the `GET /api/agentiflows` wire types in `lib/api.ts`; the
// server owns every fact, this only decides how to present it.

import type {
  AgentiflowBudgetDef,
  AgentiflowDef,
  AgentiflowGoalStatus,
  AgentiflowRecord,
  AgentiflowRow,
  AgentiflowUnit,
  RunStatusStr,
} from './api';
import { formatCost, formatTokens } from './usage';

/** Detail route for one agentiflow run. */
export function agentiflowHref(id: string): string {
  return `/agentiflows/${encodeURIComponent(id)}`;
}

/** An agentiflow's own status speaks the run-status lexicon (`running` /
 *  `completed` / `failed` are a subset of it), so `StatusPill` renders it. */
export function agentiflowPillStatus(status: AgentiflowRow['status']): RunStatusStr {
  return status;
}

/** A unit's `status.state` (`pending` | `running` | `done` | `failed`) as a
 *  `StatusPill` status. A `done` unit that reported `success: false` is a
 *  failure, not a completion. */
export function unitPillStatus(unit: Pick<AgentiflowUnit, 'status'>): RunStatusStr {
  const s = unit.status;
  switch (s.state) {
    case 'pending':
      return 'pending';
    case 'running':
      return 'running';
    case 'failed':
      return 'failed';
    case 'done':
      return s.success === false ? 'failed' : 'completed';
    default:
      // A state a newer server invented: show it as pending rather than lie.
      return 'pending';
  }
}

/** `$1.23` / `$0.0042`; an em dash while nothing has been metered. */
export function formatSpendUsd(usd: number | null | undefined): string {
  return usd == null ? '—' : formatCost(usd);
}

/** `1.2M tokens` style — the same abbreviation every other usage view uses. */
export function formatSpendTokens(tokens: number): string {
  return `${formatTokens(tokens)} tokens`;
}

/** `{usd} · {tokens} tokens` for a row / header. */
export function formatSpend(r: Pick<AgentiflowRow, 'spent_usd' | 'spent_tokens'>): string {
  return `${formatSpendUsd(r.spent_usd)} · ${formatSpendTokens(r.spent_tokens)}`;
}

const WALL_CLOCK_UNIT_MS: Record<string, number> = {
  s: 1_000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
};

/** Parse a budget `wall_clock` (`Ns` / `Nm` / `Nh` / `Nd`) to milliseconds;
 *  `null` when it does not match that grammar. */
export function parseWallClockMs(s: string | null | undefined): number | null {
  if (!s) return null;
  const m = /^\s*(\d+(?:\.\d+)?)\s*([smhd])\s*$/.exec(s);
  if (!m) return null;
  return Number(m[1]) * WALL_CLOCK_UNIT_MS[m[2]];
}

/** How long the run has been (or was) going, in ms. */
export function elapsedMs(r: Pick<AgentiflowRow, 'started_at' | 'ended_at'>, now = Date.now()): number | null {
  const start = Date.parse(r.started_at);
  if (Number.isNaN(start)) return null;
  const end = r.ended_at ? Date.parse(r.ended_at) : now;
  if (Number.isNaN(end)) return null;
  return Math.max(0, end - start);
}

/** `soft_at` when the definition leaves it unset (`BudgetEnforcer::new`). */
export const DEFAULT_SOFT_AT = 0.8;

export type BudgetDimension = 'usd' | 'tokens' | 'wall_clock' | 'rounds';

export interface BudgetUse {
  dim: BudgetDimension;
  label: string;
  /** Spent so far, in the dimension's own unit (ms for `wall_clock`). */
  used: number;
  cap: number;
  /** `used / cap`, 0 when the cap is 0 (a zero cap is exhausted — see `over`). */
  ratio: number;
  /** At or past the cap. */
  over: boolean;
  /** At or past `soft_at` (but not over). */
  soft: boolean;
}

/** One entry per cap the definition sets, in a fixed order, measured against
 *  what the record says the run has used. Dimensions with no cap are absent. */
export function budgetUses(
  budget: AgentiflowBudgetDef | null | undefined,
  record: Pick<AgentiflowRecord, 'spent_usd' | 'spent_tokens' | 'rounds' | 'started_at' | 'ended_at'>,
  now = Date.now(),
): BudgetUse[] {
  if (!budget) return [];
  const softAt = budget.soft_at ?? DEFAULT_SOFT_AT;
  const out: BudgetUse[] = [];
  const push = (dim: BudgetDimension, label: string, used: number, cap: number) => {
    const ratio = cap > 0 ? used / cap : 0;
    const over = cap > 0 ? used >= cap : true;
    out.push({ dim, label, used, cap, ratio, over, soft: !over && ratio >= softAt });
  };
  if (budget.usd != null) push('usd', 'spend', record.spent_usd ?? 0, budget.usd);
  if (budget.tokens != null) push('tokens', 'tokens', record.spent_tokens, budget.tokens);
  const wall = parseWallClockMs(budget.wall_clock);
  const elapsed = elapsedMs(record, now);
  if (wall != null && elapsed != null) push('wall_clock', 'wall clock', elapsed, wall);
  if (budget.rounds != null) push('rounds', 'rounds', record.rounds, budget.rounds);
  return out;
}

export interface GoalView {
  id: string;
  /** The definition's objective; the goal id when the snapshot is missing. */
  objective: string;
  /** `target:` in words; `null` without the definition snapshot. */
  predicate: string | null;
  required: boolean;
  met: boolean;
  current: number;
  target: number;
}

/** Join the record's goal statuses to the definition's goals by `id`. Falls
 *  back to the record alone (id as objective) when the definition snapshot is
 *  absent; a definition goal the record has no status for is shown unmet. */
export function goalViews(record: Pick<AgentiflowRecord, 'goals'>, def: AgentiflowDef | null): GoalView[] {
  const status = new Map<string, AgentiflowGoalStatus>(record.goals.map((g) => [g.id, g]));
  if (!def) {
    return record.goals.map((g) => ({
      id: g.id,
      objective: g.id,
      predicate: null,
      required: true,
      met: g.met,
      current: g.current,
      target: g.target,
    }));
  }
  const seen = new Set<string>();
  const views: GoalView[] = def.goals.map((d) => {
    seen.add(d.id);
    const s = status.get(d.id);
    return {
      id: d.id,
      objective: d.objective,
      predicate: d.predicate,
      required: d.required,
      met: s?.met ?? false,
      current: s?.current ?? 0,
      target: s?.target ?? 0,
    };
  });
  // A status for a goal the snapshot doesn't define (an edited definition).
  for (const g of record.goals) {
    if (!seen.has(g.id)) {
      views.push({ id: g.id, objective: g.id, predicate: null, required: true, met: g.met, current: g.current, target: g.target });
    }
  }
  return views;
}

/** Fraction of a goal reached, 0..1. A met goal is complete whatever its counters say. */
export function goalProgress(g: Pick<GoalView, 'met' | 'current' | 'target'>): number {
  if (g.met) return 1;
  if (g.target <= 0) return 0;
  return Math.min(1, Math.max(0, g.current / g.target));
}

/** Tone of a `stop_reason` for display: a clean finish is ok, an operator stop
 *  is neutral, anything that ran out or broke is a warning / error. */
export type StopTone = 'ok' | 'neutral' | 'warn' | 'err';

/** Text colour per tone (static class strings, so Tailwind's JIT sees them). */
export const STOP_TONE_TEXT: Record<StopTone, string> = {
  ok: 'text-ok',
  neutral: 'text-ink-dim',
  warn: 'text-warn',
  err: 'text-err',
};

/** The matching `Badge` tone. */
export const STOP_TONE_BADGE: Record<StopTone, 'green' | 'neutral' | 'amber' | 'red'> = {
  ok: 'green',
  neutral: 'neutral',
  warn: 'amber',
  err: 'red',
};

export function stopReasonTone(reason: string): StopTone {
  if (reason === 'goals_met' || reason === 'coverage_reached') return 'ok';
  if (reason.startsWith('operator_stop')) return 'neutral';
  if (reason.startsWith('budget_exhausted') || reason === 'ceiling') return 'warn';
  return 'err'; // `error: …`, `orphaned: …`, and anything unrecognized
}

/** The budget state badge's tone: `ok` | `soft` | `hard:<dimension>`. */
export function budgetStateTone(state: string): StopTone {
  if (state === 'ok') return 'ok';
  if (state === 'soft') return 'warn';
  return state.startsWith('hard') ? 'err' : 'neutral';
}
