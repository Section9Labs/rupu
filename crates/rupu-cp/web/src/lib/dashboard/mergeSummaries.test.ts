import { describe, it, expect } from 'vitest';
import { mergeSummaries } from './mergeSummaries';
import type { DashboardSummary } from '../api';

function empty(capturedAt: string): DashboardSummary {
  return {
    active: { running: 0, awaiting_approval: 0, paused: 0, pending: 0 },
    active_longest: null,
    terminal_buckets: [],
    throughput_buckets: [],
    cycles: { total: 0, clean: null, with_failures: null },
    findings_open: null,
    captured_at: capturedAt,
  };
}

describe('mergeSummaries', () => {
  it('sums each active field across hosts', () => {
    const local: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      active: { running: 2, awaiting_approval: 1, paused: 0, pending: 3 },
    };
    const ssh: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      active: { running: 1, awaiting_approval: 0, paused: 2, pending: 0 },
    };

    const merged = mergeSummaries([local, ssh]);

    expect(merged.active).toEqual({ running: 3, awaiting_approval: 1, paused: 2, pending: 3 });
  });

  it('active_longest picks the max age_ms across hosts', () => {
    const shorter: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      active_longest: { run_id: 'run_short', workflow_name: 'wf', age_ms: 5_000 },
    };
    const longer: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      active_longest: { run_id: 'run_long', workflow_name: 'wf', age_ms: 500_000 },
    };

    const merged = mergeSummaries([shorter, longer]);

    expect(merged.active_longest).toEqual({ run_id: 'run_long', workflow_name: 'wf', age_ms: 500_000 });
  });

  it('active_longest is null when no host reports one', () => {
    const merged = mergeSummaries([empty('2026-07-15T00:00:00Z')]);
    expect(merged.active_longest).toBeNull();
  });

  // The seam test (mandatory): two buckets with the SAME ts from different
  // hosts must merge into ONE bucket with summed fields — the client
  // analogue of the server's C1 bug, where a same-day bucket was silently
  // dropped instead of merged.
  it('terminal_buckets: two hosts contributing the same ts merge into one summed bucket', () => {
    const day = '2026-07-15T00:00:00Z';
    const local: DashboardSummary = {
      ...empty(day),
      terminal_buckets: [{ ts: day, completed: 3, failed: 0, rejected: 0, cancelled: 1 }],
    };
    const ssh: DashboardSummary = {
      ...empty(day),
      terminal_buckets: [{ ts: day, completed: 2, failed: 1, rejected: 0, cancelled: 0 }],
    };

    const merged = mergeSummaries([local, ssh]);

    expect(merged.terminal_buckets).toEqual([
      { ts: day, completed: 5, failed: 1, rejected: 0, cancelled: 1 },
    ]);
  });

  it('terminal_buckets: distinct ts values from different hosts all survive, sorted ascending', () => {
    const dayA = '2026-07-14T00:00:00Z';
    const dayB = '2026-07-15T00:00:00Z';
    const local: DashboardSummary = {
      ...empty(dayB),
      terminal_buckets: [{ ts: dayB, completed: 1, failed: 0, rejected: 0, cancelled: 0 }],
    };
    const ssh: DashboardSummary = {
      ...empty(dayB),
      terminal_buckets: [{ ts: dayA, completed: 4, failed: 0, rejected: 0, cancelled: 0 }],
    };

    const merged = mergeSummaries([local, ssh]);

    expect(merged.terminal_buckets.map((b) => b.ts)).toEqual([dayA, dayB]);
  });

  it('throughput_buckets: two hosts contributing the same ts merge into one summed bucket', () => {
    const day = '2026-07-15T00:00:00Z';
    const local: DashboardSummary = {
      ...empty(day),
      throughput_buckets: [{ ts: day, manual: 2, cron: 1, event: 0 }],
    };
    const ssh: DashboardSummary = {
      ...empty(day),
      throughput_buckets: [{ ts: day, manual: 1, cron: 0, event: 3 }],
    };

    const merged = mergeSummaries([local, ssh]);

    expect(merged.throughput_buckets).toEqual([{ ts: day, manual: 3, cron: 1, event: 3 }]);
  });

  it('cycles: sums total/clean/with_failures when every host reports the full breakdown', () => {
    const a: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      cycles: { total: 3, clean: 2, with_failures: 1 },
    };
    const b: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      cycles: { total: 5, clean: 4, with_failures: 1 },
    };

    const merged = mergeSummaries([a, b]);

    expect(merged.cycles).toEqual({ total: 8, clean: 6, with_failures: 2 });
  });

  it('cycles: total always sums, but one host reporting null clean/with_failures makes the merged value null, never a truncated sum', () => {
    const local: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      cycles: { total: 4, clean: 3, with_failures: 1 },
    };
    const ssh: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      cycles: { total: 2, clean: null, with_failures: null },
    };

    const merged = mergeSummaries([local, ssh]);

    expect(merged.cycles.total).toBe(6);
    expect(merged.cycles.clean).toBeNull();
    expect(merged.cycles.with_failures).toBeNull();
  });

  it('cycles: a null contributor poisons the merge regardless of processing order', () => {
    const ssh: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      cycles: { total: 2, clean: null, with_failures: null },
    };
    const local: DashboardSummary = {
      ...empty('2026-07-15T00:00:00Z'),
      cycles: { total: 4, clean: 3, with_failures: 1 },
    };

    // ssh (the null contributor) processed FIRST this time.
    const merged = mergeSummaries([ssh, local]);

    expect(merged.cycles.clean).toBeNull();
    expect(merged.cycles.with_failures).toBeNull();
  });

  it('findings_open: sums only the non-null contributors (one host null → merged stays the Some sum, not null)', () => {
    const a: DashboardSummary = { ...empty('2026-07-15T00:00:00Z'), findings_open: 3 };
    const b: DashboardSummary = { ...empty('2026-07-15T00:00:00Z'), findings_open: null };

    const merged = mergeSummaries([a, b]);

    expect(merged.findings_open).toBe(3);
  });

  it('findings_open: null when every host contributes null', () => {
    const a: DashboardSummary = { ...empty('2026-07-15T00:00:00Z'), findings_open: null };
    const b: DashboardSummary = { ...empty('2026-07-15T00:00:00Z'), findings_open: null };

    const merged = mergeSummaries([a, b]);

    expect(merged.findings_open).toBeNull();
  });

  it('captured_at: the OLDEST across hosts, not the newest', () => {
    const older: DashboardSummary = { ...empty('2026-07-10T00:00:00Z') };
    const newer: DashboardSummary = { ...empty('2026-07-15T00:00:00Z') };

    const merged = mergeSummaries([newer, older]);

    expect(merged.captured_at).toBe('2026-07-10T00:00:00Z');
  });

  it('empty input: zero active, null cycles/findings, empty buckets, captured_at falls back to the injected now', () => {
    const now = new Date('2026-07-16T12:00:00Z');
    const merged = mergeSummaries([], now);

    expect(merged.active).toEqual({ running: 0, awaiting_approval: 0, paused: 0, pending: 0 });
    expect(merged.active_longest).toBeNull();
    expect(merged.terminal_buckets).toEqual([]);
    expect(merged.throughput_buckets).toEqual([]);
    expect(merged.cycles).toEqual({ total: 0, clean: null, with_failures: null });
    expect(merged.findings_open).toBeNull();
    expect(merged.captured_at).toBe(now.toISOString());
  });
});
