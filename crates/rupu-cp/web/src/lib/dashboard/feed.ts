// feed — cycle grouping for the activity feed.
//
// The problem this solves: a chatty autoflow emitting twelve runs consumed the
// entire Recent Runs list, and the rows could not even be told apart from
// operator-launched ones. Grouping is by CYCLE, not by outcome: a cycle failing
// *as a cycle* is a real event, and outcome-grouping scatters that across rows.
//
// Pure and I/O-free so the grouping is testable without rendering.

import type { CycleRollup, DashboardRecentRun } from '../api';

export interface CycleFeedRow {
  kind: 'cycle';
  sortKey: string;
  cycle: CycleRollup;
}

export interface ManualFeedRow {
  kind: 'manual';
  sortKey: string;
  run: DashboardRecentRun;
}

export type FeedRow = CycleFeedRow | ManualFeedRow;

/**
 * Build the activity feed: one row per autoflow cycle, one row per manual run.
 *
 * Manual runs are NEVER grouped — they are the operator's own actions and each
 * one is an event they care about individually.
 */
export function buildFeed(cycles: CycleRollup[], recentManual: DashboardRecentRun[]): FeedRow[] {
  const rows: FeedRow[] = [
    ...cycles.map(
      (cycle): CycleFeedRow => ({
        kind: 'cycle',
        sortKey: cycle.started_at,
        cycle,
      }),
    ),
    ...recentManual.map(
      (run): ManualFeedRow => ({
        kind: 'manual',
        sortKey: run.started_at,
        run,
      }),
    ),
  ];
  // RFC-3339 sorts correctly lexicographically. (This is why the Rust side
  // refuses non-RFC-3339 timestamps — see plan 2 task 1.)
  rows.sort((a, b) => (a.sortKey < b.sortKey ? 1 : a.sortKey > b.sortKey ? -1 : 0));
  return rows;
}

/**
 * Should this cycle be expanded by default?
 *
 * A clean, finished cycle is noise — that is the whole autoflow-flooding
 * problem. Anything unfinished or containing failures is signal.
 */
export function isCycleInteresting(c: CycleRollup): boolean {
  // `failed` is null when the host omits the breakdown — unknown is not "no
  // failures", but it is also not evidence of one. Fall through to the
  // finished/unfinished check rather than guessing either way.
  if (c.failed !== null && c.failed > 0) return true;
  if (c.finished_at === null) return true; // still running
  return false;
}

/**
 * The statuses that fold away as "clean". An ALLOW-list, not a deny-list:
 * anything we do not positively recognize as clean stays visible. `unknown`
 * (a run the host could not resolve) therefore never folds — hiding a run we
 * know nothing about is exactly the wrong default.
 */
const CLEAN_STATUSES: ReadonlySet<string> = new Set(['completed']);

/**
 * Fold clean runs behind a `+N clean` pill: hidden, never lost.
 *
 * `awaiting_approval` and `paused` deliberately never fold — they are blocked
 * on the operator.
 */
export function foldCleanRuns<T extends { run_id: string; status: string }>(
  runs: T[],
): { shown: T[]; cleanCount: number } {
  const shown = runs.filter((r) => !CLEAN_STATUSES.has(r.status));
  return { shown, cleanCount: runs.length - shown.length };
}
