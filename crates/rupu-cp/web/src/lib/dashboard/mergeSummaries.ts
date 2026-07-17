// mergeSummaries — the client-side analogue of the server's
// `merge_dashboard_summaries` (crates/rupu-cp/src/api/dashboard.rs).
//
// WHY this exists on the client at all: the old dashboard issued ONE
// `/api/dashboard` call and the SERVER fanned out across every host,
// blocking the whole page on the slowest (or unreachable) one. The fix
// (useDashboardData.ts) fires `getDashboard(range, hostId)` independently per
// host — each call already goes through the server's per-host merge (a
// single-element `reported` vec), so what comes back per host is already a
// fully-formed, zero-filled `DashboardSummary`. This function combines those
// already-correct per-host summaries; it NEVER derives anything from raw
// events, which is what makes it safe to duplicate here instead of trusting
// the client to reimplement server aggregation from scratch.
//
// Kept pure and I/O-free (no fetch, no Date.now() by default) so it is
// testable without mocking anything — the `now` parameter exists solely as
// the deterministic fallback for `captured_at` when `byHost` is empty,
// mirroring the server's `now: DateTime<Utc>` parameter for the same reason.
//
// Split with the hook: `findings_partial` / `cycles_partial` (whether ANY
// reporting host contributed `null` for that field) are NOT returned here.
// They are response-level flags, not part of `DashboardSummary` — see
// useDashboardData.ts's doc comment for exactly how the hook derives them
// from the same per-host summaries passed in here.

import type {
  ActiveCounts,
  ActiveLongest,
  CycleCounts,
  DashboardSummary,
  TerminalBucket,
  ThroughputBucket,
} from '../api';

function mergeActive(byHost: DashboardSummary[]): ActiveCounts {
  const active: ActiveCounts = { running: 0, awaiting_approval: 0, paused: 0, pending: 0 };
  for (const s of byHost) {
    active.running += s.active.running;
    active.awaiting_approval += s.active.awaiting_approval;
    active.paused += s.active.paused;
    active.pending += s.active.pending;
  }
  return active;
}

/**
 * The single longest-running run fleet-wide: max by `age_ms` across every
 * host that has one. `null` when no host is currently running anything.
 * Mirrors the server's tie-break: the first-seen host wins a tie rather than
 * a later one with the identical age.
 */
function mergeActiveLongest(byHost: DashboardSummary[]): ActiveLongest | null {
  let longest: ActiveLongest | null = null;
  for (const s of byHost) {
    const candidate = s.active_longest;
    if (!candidate) continue;
    if (!longest || candidate.age_ms > longest.age_ms) longest = candidate;
  }
  return longest;
}

/**
 * Merge day-bucket series by exact `ts` string, then sort ascending.
 *
 * Every host's grid already arrives day-key-aligned (midnight UTC) AND
 * zero-filled for the requested range — each `getDashboard(range, hostId)`
 * call is served by the same per-host `merge_dashboard_summaries` pass the
 * fleet-wide endpoint uses, which does the zero-fill. So this function's only
 * job is the seam itself: two buckets sharing a `ts` from different hosts
 * must sum into ONE bucket, never coexist as two, and never silently drop
 * either side's contribution (the client analogue of the server's C1 bug).
 */
function mergeBuckets<T extends { ts: string }>(
  seriesByHost: T[][],
  zero: (ts: string) => T,
  sumInto: (acc: T, row: T) => void,
): T[] {
  const byTs = new Map<string, T>();
  for (const series of seriesByHost) {
    for (const row of series) {
      let acc = byTs.get(row.ts);
      if (!acc) {
        acc = zero(row.ts);
        byTs.set(row.ts, acc);
      }
      sumInto(acc, row);
    }
  }
  return [...byTs.values()].sort((a, b) => (a.ts < b.ts ? -1 : a.ts > b.ts ? 1 : 0));
}

function zeroTerminal(ts: string): TerminalBucket {
  return { ts, completed: 0, failed: 0, rejected: 0, cancelled: 0 };
}

function sumTerminal(acc: TerminalBucket, row: TerminalBucket): void {
  acc.completed += row.completed;
  acc.failed += row.failed;
  acc.rejected += row.rejected;
  acc.cancelled += row.cancelled;
}

function zeroThroughput(ts: string): ThroughputBucket {
  return { ts, manual: 0, cron: 0, event: 0 };
}

function sumThroughput(acc: ThroughputBucket, row: ThroughputBucket): void {
  acc.manual += row.manual;
  acc.cron += row.cron;
  acc.event += row.event;
}

/**
 * `total` always sums (every host that reports at all knows its own cycle
 * count). `clean` / `with_failures` sum only `Some`-shaped contributors, and
 * — this is the part that must match the server exactly, NOT the looser
 * "ignore nulls" rule `findings_open` uses below — once ANY host contributes
 * `null` for one of these two fields, that field is poisoned to `null` for
 * the rest of the merge, permanently, regardless of processing order. A
 * later host's real number must never resurrect a truncated partial sum into
 * something that LOOKS complete.
 */
function mergeCycles(byHost: DashboardSummary[]): CycleCounts {
  let total = 0;
  let clean: number | null = null;
  let withFailures: number | null = null;
  let cleanPoisoned = false;
  let withFailuresPoisoned = false;

  for (const s of byHost) {
    total += s.cycles.total;

    if (s.cycles.clean === null) {
      cleanPoisoned = true;
      clean = null;
    } else if (!cleanPoisoned) {
      clean = (clean ?? 0) + s.cycles.clean;
    }

    if (s.cycles.with_failures === null) {
      withFailuresPoisoned = true;
      withFailures = null;
    } else if (!withFailuresPoisoned) {
      withFailures = (withFailures ?? 0) + s.cycles.with_failures;
    }
  }

  return { total, clean, with_failures: withFailures };
}

/**
 * Sum only the `Some`-shaped (non-null) contributors — a host reporting
 * `null` (no findings surface, e.g. SSH) is skipped rather than poisoning the
 * whole sum to `null`. This deliberately differs from `mergeCycles` above:
 * it mirrors the server's `merge_dashboard_summaries` exactly, where
 * `findings_open` stays `Some(sum-of-reporters)` even when another host
 * contributed `None` (that host instead flips the response-level
 * `findings_partial` flag — see useDashboardData.ts). Only when EVERY host
 * contributes `null` (or there are no hosts at all) does the merged value
 * stay `null`.
 */
function mergeFindingsOpen(byHost: DashboardSummary[]): number | null {
  let sum: number | null = null;
  for (const s of byHost) {
    if (s.findings_open !== null) {
      sum = (sum ?? 0) + s.findings_open;
    }
  }
  return sum;
}

/**
 * The OLDEST `captured_at` across hosts — the honest staleness bound for the
 * merged aggregate (a fleet view is only as fresh as its slowest
 * contributor). Falls back to `now` when `byHost` is empty, matching the
 * server's `oldest_captured_at.unwrap_or(now)`.
 */
function mergeCapturedAt(byHost: DashboardSummary[], now: Date): string {
  let oldest: string | null = null;
  for (const s of byHost) {
    if (oldest === null || Date.parse(s.captured_at) < Date.parse(oldest)) {
      oldest = s.captured_at;
    }
  }
  return oldest ?? now.toISOString();
}

/**
 * Combine every host's already-correct `DashboardSummary` into one
 * fleet-wide aggregate. See the module doc comment for the full contract;
 * `now` is only consulted when `byHost` is empty (defaults to the real
 * clock, matching the server's explicit `now` parameter — callers that need
 * determinism for an empty-input case should pass it explicitly).
 */
export function mergeSummaries(byHost: DashboardSummary[], now: Date = new Date()): DashboardSummary {
  return {
    active: mergeActive(byHost),
    active_longest: mergeActiveLongest(byHost),
    terminal_buckets: mergeBuckets(byHost.map((s) => s.terminal_buckets), zeroTerminal, sumTerminal),
    throughput_buckets: mergeBuckets(byHost.map((s) => s.throughput_buckets), zeroThroughput, sumThroughput),
    cycles: mergeCycles(byHost),
    findings_open: mergeFindingsOpen(byHost),
    captured_at: mergeCapturedAt(byHost, now),
  };
}
