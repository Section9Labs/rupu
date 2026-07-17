// Dashboard — operations-first.
//
// Was spend-forward: the largest element on the page was cost and tokens. But a
// dashboard you leave open in a tab is an ops monitor; spend is something you
// review deliberately, on a cadence. Spend now lives at /usage with room to
// answer attribution and anomaly questions (plan 3).
//
// Composition (spec §5.1): freshness strip → attention row → swimlane hero →
// split status → activity feed.

import { useMemo, useState } from 'react';
import { useDashboardData } from '../lib/dashboard/useDashboardData';
import { HostFreshnessStrip } from '../components/dashboard/HostFreshnessStrip';
import { AttentionRow } from '../components/dashboard/AttentionRow';
import { ActiveStatusTiles } from '../components/dashboard/ActiveStatusTiles';
import { TerminalTrend } from '../components/dashboard/TerminalTrend';
import { Swimlane } from '../components/dashboard/Swimlane';
import { ActivityFeed } from '../components/dashboard/ActivityFeed';
import type { DashboardRange } from '../lib/api';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))]">
      <h2 className="border-b border-[rgb(var(--c-border))] px-3 py-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
        {title}
      </h2>
      <div className="p-3">{children}</div>
    </section>
  );
}

export default function Dashboard() {
  const [range, setRange] = useState<DashboardRange>('30d');
  const { data, error, loading } = useDashboardData(range);

  const failedInWindow = useMemo(
    () => (data?.terminal_buckets ?? []).reduce((s, b) => s + b.failed, 0),
    [data],
  );

  if (loading && !data) {
    return <div className="p-6 text-sm text-[rgb(var(--c-ink-mute))]">Loading…</div>;
  }
  if (!data) {
    return (
      <div className="p-6 text-sm text-[rgb(var(--c-status-failed))]">
        Could not load dashboard: {error?.message}
      </div>
    );
  }

  return (
    <div className="space-y-4 p-4">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold text-[rgb(var(--c-ink))]">Dashboard</h1>
          <div className="mt-1">
            <HostFreshnessStrip hosts={data.hosts} />
          </div>
        </div>
        <div className="flex items-center gap-2">
          {/* Stale data is kept on a transient error rather than flashing an
              error state; surface it quietly instead. */}
          {error && (
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
          {/* NOTE: no "Spend →" link yet. `/usage` does not exist — plan 3 builds
              it, and adds this link at the same time. Shipping a link to a 404 is
              worse than shipping no link. Consequence, stated plainly: spend is
              ABSENT from the dashboard between plan 1 and plan 3. That is a
              temporary regression against the old spend-forward page, and it is
              why plan 3 should follow closely. */}
        </div>
      </header>

      <AttentionRow
        active={data.active}
        failedInWindow={failedInWindow}
        findingsOpen={data.findings_open}
        findingsPartial={data.findings_partial}
      />

      <Panel title="Live activity">
        <Swimlane bars={data.active_runs} />
      </Panel>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Panel title="Active now">
          <ActiveStatusTiles active={data.active} />
        </Panel>
        <Panel title="Outcomes over time">
          <TerminalTrend buckets={data.terminal_buckets} />
        </Panel>
      </div>

      <Panel title="Activity">
        <ActivityFeed cycles={data.cycles} recentManual={data.recent_manual} />
      </Panel>
    </div>
  );
}
