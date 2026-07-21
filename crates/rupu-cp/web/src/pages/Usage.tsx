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
// once per `usageWindow`); `getUsage`/`getUsageOutliers` are unchanged from before
// and still drive the headline number, `UnpricedBanner`, and
// `HostFreshnessStrip` — those stay fleet-wide and are labeled as such.
//
// TABLE/GRAPH SHARED SOURCE (bugfix): the breakdown table below is built
// from `aggregateRuns(runs, pivot)` over the SAME flat run rows the graph's
// `buildTimeline` consumes (handed back via `UsageTimeline`'s
// `onRunsLoaded`) — NOT from `data.breakdown` (fleet-wide, top-6 + an
// `others (N)` rollup, from `GET /api/usage`). The two datasets used to
// diverge: a table row could name a pivot key the graph had never heard of
// (toggling it did nothing), the top-6/others rollup permanently disabled
// the rollup row's checkbox, and an empty pivot value rendered as an
// inert "—". `aggregateRuns` — like `ProjectUsageTimeline`'s identical
// pattern — has no top-N slicing and every row is a real, toggleable
// group, so every table row now corresponds 1:1 to a graph series.
//
// The graph itself (Task U4) is now `<UsageTimeline>` — extracted so the
// Projects page's Runs tab can mount the identical component, scoped by
// `workspaceId`, instead of forking it. Pivot/metric/the exclusion filter
// stay OWNED here (not inside `UsageTimeline`) because this page shares all
// three with the breakdown table and outlier panel below.

import { useCallback, useEffect, useMemo, useState, useTransition } from 'react';
import {
  api,
  presetWindow,
  windowFromDayRange,
  type DashboardRange,
  type OutlierRun,
  type UsageResponse,
  type UsageRunRow,
  type UsageWindow,
} from '../lib/api';
import { formatCost, formatTokens } from '../lib/usage';
import { aggregateRuns, type TimelineFilter } from '../lib/usage/buildTimeline';
import { PivotPicker, PIVOT_LABEL, type Pivot } from '../components/usage/PivotPicker';
import { UnpricedBanner } from '../components/usage/UnpricedBanner';
import { OutlierPanel } from '../components/usage/OutlierPanel';
import { HostFreshnessStrip } from '../components/dashboard/HostFreshnessStrip';
import UsageTimeline from '../components/usage/UsageTimeline';
import { type UsageMetric } from '../components/dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../components/dashboard/ModelBreakdownTable';
import { Spinner } from '../components/ui/Spinner';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

function toggleInSet(set: Set<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

export default function Usage() {
  const [range, setRange] = useState<DashboardRange>('30d');
  // The `{since, until}` window driving every usage fetch below (Task W2) —
  // `range` is kept alongside purely for the 7/30/All button highlighting.
  // Held in state (not recomputed inline from `range` on every render) so
  // its object identity is stable across renders that don't change it.
  // Named `usageWindow` (not `window`) to avoid shadowing the global
  // `Window`. A drag-selected custom window (Task W3, `handleSelectRange`
  // below) sets this to an arbitrary window without touching `range`;
  // `isCustomWindow` tracks which of the two is currently active so the
  // "custom" chip and the preset-button highlighting agree.
  const [usageWindow, setUsageWindow] = useState<UsageWindow>(() => presetWindow('30d'));
  const [isCustomWindow, setIsCustomWindow] = useState(false);
  const [pivot, setPivot] = useState<Pivot>('model');
  const [metric, setMetric] = useState<UsageMetric>('cost');
  // Task loading-ux: pivot switches and filter-exclusion toggles trigger a
  // synchronous `buildTimeline` re-stack inside `UsageTimeline` (no
  // network refetch — see the effect below) — `isPending` marks that brief
  // recompute window so the graph can show a subtle "updating" cue instead
  // of just snapping to the new shape.
  const [isPending, startTransition] = useTransition();

  const handleRangeChange = useCallback((r: DashboardRange) => {
    setRange(r);
    setUsageWindow(presetWindow(r));
    setIsCustomWindow(false);
  }, []);

  // Task W3: a drag-select on the graph narrows the whole page to an
  // arbitrary `{since, until}` window, exactly like a preset. `startDay`/
  // `endDay` are the ordered day-bucket labels `UsageTimelineStacked`'s
  // `useDragSelection` resolves a real drag to.
  const handleSelectRange = useCallback((startDay: string, endDay: string) => {
    setUsageWindow(windowFromDayRange(startDay, endDay));
    setIsCustomWindow(true);
  }, []);

  // "custom · ×" chip's clear: return to the currently-highlighted preset's
  // window without changing which preset is highlighted.
  const clearCustomWindow = useCallback(() => {
    setUsageWindow(presetWindow(range));
    setIsCustomWindow(false);
  }, [range]);

  const [data, setData] = useState<UsageResponse | null>(null);
  const [outliers, setOutliers] = useState<OutlierRun[]>([]);
  const [error, setError] = useState<Error | null>(null);
  // The flat per-run rows `UsageTimeline` fetches for the graph (Task U1),
  // handed back via `onRunsLoaded` so the breakdown table below can be built
  // from the SAME rows instead of `data.breakdown` (fleet-wide, from
  // `GET /api/usage`) — see the file-header doc comment: two different
  // datasets meant a table checkbox could toggle a key the graph had never
  // heard of (no effect), get stuck disabled (the top-6/others rollup), or
  // render as a bare "—" for an empty pivot value. Mirrors
  // `ProjectUsageTimeline`'s `aggregateRuns(runs, pivot)` table.
  const [runs, setRuns] = useState<UsageRunRow[]>([]);

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
  // freshness. Re-fetches whenever the window OR the pivot changes. Depends
  // on `usageWindow.since`/`usageWindow.until` (primitives), not the
  // `usageWindow` object itself — `handleSelectRange` (and `presetWindow`)
  // build a fresh window object each call, and keying off the object would
  // risk a spurious refetch loop if that ever stopped being referentially
  // stable (same primitive-deps pattern as `UsageTimeline`'s own effect).
  useEffect(() => {
    let cancelled = false;
    api
      .getUsage(usageWindow, pivot)
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
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed off usageWindow's primitive fields, not the object itself; see comment above.
  }, [usageWindow.since, usageWindow.until, pivot]);

  // `/api/usage/outliers`: local-only, re-fetches on window (primitives —
  // see the comment on the effect above).
  useEffect(() => {
    let cancelled = false;
    api
      .getUsageOutliers(usageWindow)
      .then((rows) => {
        if (!cancelled) setOutliers(rows);
      })
      .catch(() => {
        if (!cancelled) setOutliers([]);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed off usageWindow's primitive fields, not the object itself; see comment on the effect above.
  }, [usageWindow.since, usageWindow.until]);

  const filter = useMemo<TimelineFilter>(
    () => ({ excludedKeys, excludedRunIds }),
    [excludedKeys, excludedRunIds],
  );

  const toggleKey = useCallback((key: string) => {
    startTransition(() => {
      setExcludedKeys((prev) => toggleInSet(prev, key));
    });
  }, []);
  const toggleRun = useCallback((runId: string) => {
    startTransition(() => {
      setExcludedRunIds((prev) => toggleInSet(prev, runId));
    });
  }, []);
  const resetExclusions = useCallback(() => {
    startTransition(() => {
      setExcludedKeys(new Set());
      setExcludedRunIds(new Set());
    });
  }, []);

  const excludedCount = excludedKeys.size + excludedRunIds.size;

  // Unfiltered breakdown for the table — every pivot key must stay
  // clickable even while excluded, or there'd be no way to re-include it.
  // Same convention as `ProjectUsageTimeline`.
  const breakdown = useMemo(() => aggregateRuns(runs, pivot), [runs, pivot]);

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
          <PivotPicker value={pivot} onChange={(p) => startTransition(() => setPivot(p))} />
          {isCustomWindow && (
            <button
              type="button"
              onClick={clearCustomWindow}
              className="rounded-full border border-[rgb(var(--c-border))] px-2 py-0.5 text-[10px] text-[rgb(var(--c-ink-mute))] hover:bg-[rgb(var(--c-surface))]"
              title="Clear the drag-selected window and return to the active preset"
            >
              custom · ×
            </button>
          )}
          <div className="flex rounded-md border border-[rgb(var(--c-border))]">
            {RANGES.map((r) => (
              <button
                key={r}
                type="button"
                onClick={() => handleRangeChange(r)}
                className={`px-2 py-1 text-xs ${
                  !isCustomWindow && range === r
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
            usageWindow={usageWindow}
            pivot={pivot}
            metric={metric}
            onMetricChange={setMetric}
            filter={filter}
            excludedCount={excludedCount}
            onReset={resetExclusions}
            onRunsLoaded={setRuns}
            onSelectRange={handleSelectRange}
            pending={isPending}
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
              rows={breakdown}
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
        <div className="flex items-center justify-center p-6">
          <Spinner size="md" label="Loading…" />
        </div>
      )}
    </div>
  );
}
