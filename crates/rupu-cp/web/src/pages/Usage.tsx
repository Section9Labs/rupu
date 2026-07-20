// Usage — the spend page (dashboard redesign plan 3, task 5; made interactive
// in Task U3).
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
// INTERACTIVE FILTERING (Task U3) is the payoff of U1 ('GET /api/usage/runs',
// flat per-`(run × model)` rows) + U2 (`buildTimeline`, the pure client
// aggregation): the graph is now fed by the flat run rows, bucketed+stacked
// by the active pivot and filtered by two `Set<string>`s of excluded pivot
// keys / run ids held in this component's state. Toggling a breakdown-table
// checkbox or an outlier's exclude toggle mutates one of those sets, which
// re-runs the memoized `buildTimeline` synchronously — no refetch, so pulling
// a real ~1000x-cost outlier out of the graph is instant and the axis
// rescales live. `getUsageRuns` itself is pivot/filter-independent (fetched
// once per `range`); `getUsage`/`getUsageOutliers` are unchanged from before
// and still drive the headline number, the breakdown table's own summary
// rollup, `UnpricedBanner`, and `HostFreshnessStrip`.
//
// The graph itself (Task U4) is now `<UsageTimeline>` — extracted so the
// Projects page's Runs tab can mount the identical component, scoped by
// `workspaceId`, instead of forking it. Pivot/metric/the exclusion filter
// stay OWNED here (not inside `UsageTimeline`) because this page shares all
// three with the breakdown table and outlier panel below.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, type DashboardRange, type OutlierRun, type UsageResponse } from '../lib/api';
import { formatCost, formatTokens } from '../lib/usage';
import { type TimelineFilter } from '../lib/usage/buildTimeline';
import { PivotPicker, PIVOT_LABEL, type Pivot } from '../components/usage/PivotPicker';
import { UnpricedBanner } from '../components/usage/UnpricedBanner';
import { OutlierPanel } from '../components/usage/OutlierPanel';
import { HostFreshnessStrip } from '../components/dashboard/HostFreshnessStrip';
import UsageTimeline from '../components/usage/UsageTimeline';
import { type UsageMetric } from '../components/dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../components/dashboard/ModelBreakdownTable';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

function toggleInSet(set: Set<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

export default function Usage() {
  const [range, setRange] = useState<DashboardRange>('30d');
  const [pivot, setPivot] = useState<Pivot>('model');
  const [metric, setMetric] = useState<UsageMetric>('cost');

  const [data, setData] = useState<UsageResponse | null>(null);
  const [outliers, setOutliers] = useState<OutlierRun[]>([]);
  const [error, setError] = useState<Error | null>(null);

  const [excludedKeys, setExcludedKeys] = useState<Set<string>>(new Set());
  const [excludedRunIds, setExcludedRunIds] = useState<Set<string>>(new Set());

  // A pivot key is only meaningful under the dimension it was excluded from
  // (a `model` value has nothing to say about a `workflow` grouping) — clear
  // stale key exclusions on pivot switch so "Excluded (N)" never counts a key
  // that can no longer match any row. Run-id exclusions are pivot-independent
  // (a run's identity doesn't change), so they persist across pivot changes.
  useEffect(() => {
    setExcludedKeys(new Set());
  }, [pivot]);

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

  const filter = useMemo<TimelineFilter>(
    () => ({ excludedKeys, excludedRunIds }),
    [excludedKeys, excludedRunIds],
  );

  const toggleKey = useCallback((key: string) => {
    setExcludedKeys((prev) => toggleInSet(prev, key));
  }, []);
  const toggleRun = useCallback((runId: string) => {
    setExcludedRunIds((prev) => toggleInSet(prev, runId));
  }, []);
  const resetExclusions = useCallback(() => {
    setExcludedKeys(new Set());
    setExcludedRunIds(new Set());
  }, []);

  const excludedCount = excludedKeys.size + excludedRunIds.size;

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

          {/* The headline here is fleet-wide (`/api/usage`, fans out across
              hosts) — deliberately NOT derived from the local-only run rows
              `UsageTimeline` fetches for the graph itself, which is why it's
              passed in rather than computed inside that component (see its
              doc comment). */}
          <UsageTimeline
            range={range}
            pivot={pivot}
            metric={metric}
            onMetricChange={setMetric}
            filter={filter}
            excludedCount={excludedCount}
            onReset={resetExclusions}
            hosts={data.hosts}
            headline={{
              costLabel: formatCost(data.summary.cost_usd),
              subLabel: `${formatTokens(data.summary.total_tokens)} tokens · ${data.summary.runs} runs${
                !data.summary.priced ? ' · partial (see banner above)' : ''
              }`,
            }}
          />

          <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
            <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
              Breakdown by {PIVOT_LABEL[pivot]}
            </h2>
            <ModelBreakdownTable
              rows={data.breakdown}
              pivot={pivot}
              hosts={data.hosts}
              selectable
              excludedKeys={excludedKeys}
              onToggleKey={toggleKey}
            />
          </section>

          <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
            <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
              Cost outliers{' '}
              <span className="font-normal normal-case text-[rgb(var(--c-ink-mute))]">
                (this host only)
              </span>
            </h2>
            <OutlierPanel outliers={outliers} excludedRunIds={excludedRunIds} onToggleRun={toggleRun} />
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
