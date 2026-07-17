// Usage — the spend page (dashboard redesign plan 3, task 5).
//
// The dashboard is ops-first and deliberately dropped spend into its own
// page: an ops monitor left open in a tab is not where you review spend on a
// cadence. This page answers the two questions the dashboard doesn't:
//   ATTRIBUTION — pivot by model/provider/agent/workflow/host/project.
//     "This autoflow costs $40/night" was unanswerable when the only
//     breakdown was by model.
//   ANOMALY — which runs cost far more than their workflow normally does
//     (`OutlierPanel`, per-workflow median baseline).
// The unpriced-spend gap is named and counted (`UnpricedBanner`) rather than
// a bare '*' footnote — a silent under-count is worse than no number.
//
// Trend is not a separate section: `UsageTimelineStacked`, fed by
// `/api/usage/timeline`, IS the trend view. That endpoint has no `group_by`
// of its own and always groups by model server-side (see the doc comment on
// `UsageTimelineStacked.tsx`), so the graph stays model-keyed regardless of
// the active pivot; the pivot instead drives the headline attribution and
// the breakdown table below, which is where all six dimensions genuinely
// live (`/api/usage?group_by=`).

import { useEffect, useState } from 'react';
import { api, usageRangeSince, type DashboardRange, type OutlierRun, type UsageResponse } from '../lib/api';
import type { UsageTimelineBucket } from '../lib/usage';
import { formatCost, formatTokens } from '../lib/usage';
import { PivotPicker, PIVOT_LABEL, type Pivot } from '../components/usage/PivotPicker';
import { UnpricedBanner } from '../components/usage/UnpricedBanner';
import { OutlierPanel } from '../components/usage/OutlierPanel';
import { HostFreshnessStrip } from '../components/dashboard/HostFreshnessStrip';
import UsageTimelineStacked, { type UsageMetric } from '../components/dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../components/dashboard/ModelBreakdownTable';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];
const METRICS: UsageMetric[] = ['cost', 'tokens'];

export default function Usage() {
  const [range, setRange] = useState<DashboardRange>('30d');
  const [pivot, setPivot] = useState<Pivot>('model');
  const [metric, setMetric] = useState<UsageMetric>('cost');

  const [data, setData] = useState<UsageResponse | null>(null);
  const [timeline, setTimeline] = useState<UsageTimelineBucket[]>([]);
  const [outliers, setOutliers] = useState<OutlierRun[]>([]);
  const [error, setError] = useState<Error | null>(null);

  // `/api/usage`: summary + the pivoted breakdown + the unpriced gap + host
  // freshness. Re-fetches whenever the range OR the pivot changes.
  useEffect(() => {
    let cancelled = false;
    api
      .getUsage(range, pivot)
      .then((resp) => {
        if (cancelled) return;
        setData(resp);
        setError(null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e : new Error(String(e)));
      });
    return () => {
      cancelled = true;
    };
  }, [range, pivot]);

  // `/api/usage/timeline`: the trend graph. Pivot-independent (see the module
  // doc comment) — only re-fetches on range.
  useEffect(() => {
    let cancelled = false;
    api
      .getUsageTimeline({ since: usageRangeSince(range) })
      .then((buckets) => {
        if (!cancelled) setTimeline(buckets);
      })
      .catch(() => {
        // The timeline is a secondary graph; a failure here should not blank
        // the whole page when the summary/breakdown above loaded fine.
        if (!cancelled) setTimeline([]);
      });
    return () => {
      cancelled = true;
    };
  }, [range]);

  // `/api/usage/outliers`: local-only, re-fetches on range.
  useEffect(() => {
    let cancelled = false;
    api
      .getUsageOutliers(range)
      .then((rows) => {
        if (!cancelled) setOutliers(rows);
      })
      .catch(() => {
        if (!cancelled) setOutliers([]);
      });
    return () => {
      cancelled = true;
    };
  }, [range]);

  return (
    <div className="space-y-4 p-4">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold text-[rgb(var(--c-ink))]">Usage</h1>
          {data && (
            <div className="mt-1">
              <HostFreshnessStrip hosts={data.hosts} />
            </div>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {error && data && (
            <span className="text-xs text-[rgb(var(--c-status-failed))]" title={error.message}>
              refresh failed — showing last good data
            </span>
          )}
          <PivotPicker value={pivot} onChange={setPivot} />
          <div className="flex rounded-md border border-[rgb(var(--c-border))]">
            {RANGES.map((r) => (
              <button
                key={r}
                type="button"
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
        </div>
      </header>

      {data ? (
        <>
          <UnpricedBanner unpriced={data.unpriced} />

          <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
            <div className="mb-2 flex flex-wrap items-start justify-between gap-2">
              <div>
                <h2 className="text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
                  Spend over time
                </h2>
                <p className="mt-0.5 text-2xl font-semibold tabular-nums text-[rgb(var(--c-ink))]">
                  {formatCost(data.summary.cost_usd)}
                </p>
                <p className="text-xs text-[rgb(var(--c-ink-mute))]">
                  {formatTokens(data.summary.total_tokens)} tokens · {data.summary.runs} runs
                  {!data.summary.priced && ' · partial (see banner above)'}
                </p>
              </div>
              <div className="flex rounded-md border border-[rgb(var(--c-border))]">
                {METRICS.map((m) => (
                  <button
                    key={m}
                    type="button"
                    onClick={() => setMetric(m)}
                    className={`px-2 py-1 text-xs capitalize ${
                      metric === m
                        ? 'bg-[rgb(var(--c-surface))] text-[rgb(var(--c-ink))]'
                        : 'text-[rgb(var(--c-ink-mute))]'
                    }`}
                  >
                    {m}
                  </button>
                ))}
              </div>
            </div>
            <UsageTimelineStacked buckets={timeline} metric={metric} />
          </section>

          <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
            <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
              Breakdown by {PIVOT_LABEL[pivot]}
            </h2>
            <ModelBreakdownTable rows={data.breakdown} pivot={pivot} />
          </section>

          <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
            <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
              Cost outliers
            </h2>
            <OutlierPanel outliers={outliers} />
          </section>
        </>
      ) : error ? (
        <div className="p-6 text-sm text-[rgb(var(--c-status-failed))]">
          Could not load usage: {error.message}
        </div>
      ) : (
        <div className="p-6 text-sm text-[rgb(var(--c-ink-mute))]">Loading…</div>
      )}
    </div>
  );
}
