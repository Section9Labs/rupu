// Dashboard — operations-first, key points not lists (spec §5).
//
// Was spend-forward: the largest element on the page was cost and tokens. But a
// dashboard you leave open in a tab is an ops monitor; spend is something you
// review deliberately, on a cadence. Spend now lives at /usage with room to
// answer attribution and anomaly questions (plan 3).
//
// Composition (final, per the composition-decision note in the P4 brief —
// supersedes spec §5.1's split-status layout): header (range + freshness
// strip) → KeyPointTiles → two graphs (outcomes trend + throughput) →
// CycleSummaryLine. AttentionRow and ActiveStatusTiles are DROPPED: once
// KeyPointTiles grew an awaiting/paused/failed/findings/active-now row, those
// two components duplicated it outright. Three components showing the same
// counts is the opposite of "key points, not lists."
//
// Each block renders from whatever has arrived: the freshness strip paints
// the instant the host list is known (`hosts`, seeded `loading`), and the
// KPI tiles / graphs / cycle line paint from `data` — which goes non-null as
// soon as ONE host resolves — without waiting on every host. A hung remote
// host therefore never blanks the page; it just sits in the strip reading
// "loading" until it resolves or the reconciling poll gives up on it.

import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useDashboardData } from '../lib/dashboard/useDashboardData';
import { HostFreshnessStrip, type HostFreshnessEntry } from '../components/dashboard/HostFreshnessStrip';
import { KeyPointTiles } from '../components/dashboard/KeyPointTiles';
import { TerminalTrend } from '../components/dashboard/TerminalTrend';
import { ThroughputChart } from '../components/dashboard/ThroughputChart';
import { CycleSummaryLine } from '../components/dashboard/CycleSummaryLine';
import type { DashboardRange } from '../lib/api';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

export default function Dashboard() {
  const [range, setRange] = useState<DashboardRange>('30d');
  const { data, hosts, error } = useDashboardData(range);

  // `useDashboardData`'s per-host state is keyed camelCase (`hostId` /
  // `transportKind`) and carries the raw per-host `summary`; the strip wants
  // the wire-shaped `HostFreshnessEntry` (snake_case, `captured_at` pulled
  // out of that summary). This mapping is the seam between the two.
  const freshnessHosts: HostFreshnessEntry[] = useMemo(
    () =>
      hosts.map((h) => ({
        host_id: h.hostId,
        name: h.name,
        transport_kind: h.transportKind,
        state: h.state,
        captured_at: h.summary?.captured_at ?? null,
        reason: h.reason ?? null,
      })),
    [hosts],
  );

  return (
    <div className="space-y-4 p-4">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold text-[rgb(var(--c-ink))]">Dashboard</h1>
          <div className="mt-1">
            <HostFreshnessStrip hosts={freshnessHosts} />
          </div>
        </div>
        <div className="flex items-center gap-2">
          {/* Stale data is kept on a transient error rather than flashing an
              error state; surface it quietly instead. */}
          {error && data && (
            <span className="text-xs text-[rgb(var(--c-status-failed))]" title={error.message}>
              refresh failed — showing last good data
            </span>
          )}
          <div className="flex rounded-md border border-[rgb(var(--c-border))]">
            {RANGES.map((r) => (
              <button
                key={r}
                onClick={() => setRange(r)}
                className={`px-2 py-1 text-xs ${
                  range === r
                    ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
                    : 'text-[rgb(var(--c-ink-mute))]'
                }`}
              >
                {r}
              </button>
            ))}
          </div>
          {/* `/usage` now exists (plan 3, task 5) — this closes the "spend is
              absent from the dashboard" regression the P4 rewrite deliberately
              left open until the page it points to was real. */}
          <Link
            to="/usage"
            className="rounded-md border border-[rgb(var(--c-border))] px-3 py-1 text-xs text-[rgb(var(--c-ink-dim))] hover:text-[rgb(var(--c-ink))]"
          >
            Spend →
          </Link>
        </div>
      </header>

      {data ? (
        <>
          <KeyPointTiles
            active={data.active}
            activeLongest={data.active_longest}
            terminalBuckets={data.terminal_buckets}
            findingsOpen={data.findings_open}
            findingsPartial={data.findings_partial}
          />

          <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
            <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
              <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
                Outcomes over time
              </h2>
              <TerminalTrend buckets={data.terminal_buckets} />
            </section>
            <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
              <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
                Throughput by trigger
              </h2>
              <ThroughputChart buckets={data.throughput_buckets} />
            </section>
          </div>

          <CycleSummaryLine cycles={data.cycles} cyclesPartial={data.cycles_partial} />
        </>
      ) : error ? (
        <div className="p-6 text-sm text-[rgb(var(--c-status-failed))]">
          Could not load dashboard: {error.message}
        </div>
      ) : (
        <div className="p-6 text-sm text-[rgb(var(--c-ink-mute))]">Loading…</div>
      )}
    </div>
  );
}
