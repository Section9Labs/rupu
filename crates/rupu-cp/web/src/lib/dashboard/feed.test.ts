import { describe, it, expect } from 'vitest';
import { buildFeed, isCycleInteresting, foldCleanRuns } from './feed';
import type { CycleRollup, DashboardRecentRun } from '../api';

const cycle = (over: Partial<CycleRollup> = {}): CycleRollup => ({
  cycle_id: 'cyc_1',
  worker_name: 'nightly-review',
  started_at: '2026-07-16T03:00:00Z',
  finished_at: '2026-07-16T03:12:00Z',
  ran: 12,
  skipped: 0,
  failed: 0,
  runs: [
    { run_id: 'r1', status: 'completed' },
    { run_id: 'r2', status: 'completed' },
  ],
  ...over,
});

const manual = (over: Partial<DashboardRecentRun> = {}): DashboardRecentRun => ({
  id: 'run_m1',
  workflow_name: 'adhoc',
  status: 'completed',
  started_at: '2026-07-16T09:00:00Z',
  finished_at: '2026-07-16T09:01:00Z',
  trigger: 'manual',
  ...over,
});

describe('buildFeed', () => {
  it('emits one row per cycle, not one per run', () => {
    const rows = buildFeed(
      [
        cycle({
          runs: ['r1', 'r2', 'r3', 'r4'].map((run_id) => ({ run_id, status: 'completed' as const })),
        }),
      ],
      [],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe('cycle');
  });

  it('never groups manual runs', () => {
    const rows = buildFeed([], [manual({ id: 'a' }), manual({ id: 'b' })]);
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => r.kind === 'manual')).toBe(true);
  });

  it('sorts cycles and manual runs together, newest first', () => {
    const rows = buildFeed(
      [cycle({ started_at: '2026-07-16T03:00:00Z' })],
      [manual({ started_at: '2026-07-16T09:00:00Z' })],
    );
    expect(rows[0].kind).toBe('manual'); // 09:00 is newer than 03:00
  });
});

describe('isCycleInteresting', () => {
  it('is interesting when any run failed', () => {
    expect(isCycleInteresting(cycle({ failed: 2 }))).toBe(true);
  });

  it('a fully clean cycle is not interesting', () => {
    expect(isCycleInteresting(cycle({ ran: 12, failed: 0, skipped: 0 }))).toBe(false);
  });

  it('an unfinished cycle is interesting — it is still live work', () => {
    expect(isCycleInteresting(cycle({ finished_at: null, failed: 0 }))).toBe(true);
  });
});

describe('foldCleanRuns', () => {
  it('folds clean runs away and reports the count', () => {
    const runs = [
      { run_id: 'r1', status: 'completed' as const },
      { run_id: 'r2', status: 'completed' as const },
      { run_id: 'r3', status: 'failed' as const },
    ];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown.map((r) => r.run_id)).toEqual(['r3']);
    expect(cleanCount).toBe(2);
  });

  it('folds nothing when every run failed', () => {
    const runs = [{ run_id: 'r1', status: 'failed' as const }];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown).toHaveLength(1);
    expect(cleanCount).toBe(0);
  });

  it('keeps awaiting_approval visible — it is blocked on the operator', () => {
    const runs = [{ run_id: 'r1', status: 'awaiting_approval' as const }];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown).toHaveLength(1);
    expect(cleanCount).toBe(0);
  });

  it('never folds an unresolved run — unknown is not clean', () => {
    const runs = [{ run_id: 'r1', status: 'unknown' as const }];
    const { shown, cleanCount } = foldCleanRuns(runs);
    expect(shown).toHaveLength(1);
    expect(cleanCount).toBe(0);
  });
});
