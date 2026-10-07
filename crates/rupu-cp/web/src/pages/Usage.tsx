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
// once per `usageWindow`); the headline number, `UnpricedBanner` and
// `HostFreshnessStrip` come from `useUsageData`, which loads `/api/usage` PER
// HOST (each host answers on its own and the figures are merged client-side;
// hosts that have not answered yet are named in the headline, not waited
// for) — those stay fleet-wide and are labeled as such. The hook pins
// `group_by=model`, so a pivot change never refetches the headline.
// `getUsageOutliers` is unchanged.
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
//
// Customer scope (customers Plan 2B): the page follows the global scope
// (`useScopedList`; its header shows the `ScopeChip`), or — embedded in a
// customer's Usage tab — a fixed `customer` prop, which also drops the title.
// Either scopes every fetch to that customer's work (`?customer=`). Aggregates
// are local-only under a filter, so a remote host answers 501 and shows as
// unavailable rather than being counted; `HostsWithoutCustomerBanner` names it,
// along with the hosts the headline's `hosts_without_customer` and the run-rows
// / outliers fetches' `X-Rupu-Hosts-Without-Customer` header name. A scope the
// backend rejects (400) is cleared with a notice.

import { useCallback, useEffect, useMemo, useRef, useState, useTransition } from 'react';
import {
  api,
  presetWindow,
  windowFromDayRange,
  type DashboardRange,
  type OutlierRun,
  type UsageRunRow,
  type UsageWindow,
} from '../lib/api';
import { formatCost, formatTokens } from '../lib/usage';
import { aggregateRuns, type TimelineFilter } from '../lib/usage/buildTimeline';
import { useUsageData } from '../lib/usage/useUsageData';
import { PivotPicker, PIVOT_LABEL, type Pivot } from '../components/usage/PivotPicker';
import { UnpricedBanner } from '../components/usage/UnpricedBanner';
import { OutlierPanel } from '../components/usage/OutlierPanel';
import { HostFreshnessStrip } from '../components/dashboard/HostFreshnessStrip';
import UsageTimeline from '../components/usage/UsageTimeline';
import { type UsageMetric } from '../components/dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../components/dashboard/ModelBreakdownTable';
import { Spinner } from '../components/ui/Spinner';
import { useScopedList } from '../lib/useScopedList';
import { ScopeChip } from '../components/customers/ScopeChip';
import { HostsWithoutCustomerBanner } from '../components/customers/HostsWithoutCustomerBanner';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

/** How often a preset ("ends now") window is re-derived and its data refetched. */
const USAGE_REFRESH_MS = 30_000;

function toggleInSet(set: Set<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

export default function Usage({ customer: fixedCustomer }: { customer?: string } = {}) {
  const { customer, embedded, rejectIfScopeError } = useScopedList(fixedCustomer, []);
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
  // Why `usageWindow` last changed: an operator action (`'user'`: preset
  // button, drag-select, clearing the custom chip) or the periodic live
  // refresh below (`'tick'`). A tick refetch is background work — it keeps
  // the last good data on failure and shows no "updating" cue; a user change
  // keeps both. Always set in the same batch as `setUsageWindow`.
  const [windowSource, setWindowSource] = useState<'user' | 'tick'>('user');
  // The fleet headline, loaded per host (spec 2026-10-01 §6.5). A preset's
  // identity is the preset (its `until` ticks every 30s without changing what
  // the operator is looking at); a custom window's is its exact bounds.
  const windowKey = isCustomWindow ? `${usageWindow.since}|${usageWindow.until}` : `preset:${range}`;
  const { data: current, hosts, error, notice } = useUsageData(
    usageWindow,
    windowKey,
    windowSource,
    customer,
    rejectIfScopeError,
  );
  // `current` is null from a user window change until the first host answers for
  // the NEW window (the hook never mixes an old window's figures into a new
  // one). Keep the last good headline on screen meanwhile, as the page did when
  // it held a single `/api/usage` response: the graph's "updating" cue and the
  // host strip's `loading` entries say it is refreshing, and the page does not
  // collapse to its full-page spinner (which would unmount `UsageTimeline` and
  // delay its run-rows fetch until the headline lands).
  // Never across a customer change, though: another customer's spend is not a
  // stand-in for this one's, so a new scope waits for its own first answer.
  const lastGood = useRef({ customer, data: current });
  if (current) lastGood.current = { customer, data: current };
  const data = current ?? (lastGood.current.customer === customer ? lastGood.current.data : null);
  // The last good headline is standing in for a window no host has answered yet.
  // If every host has failed for it (`error`), the "refresh failed" chip says so
  // and the cue stops: a drag-selected window never retries, so a spinner beside
  // that chip would spin forever.
  const headlineStale = current === null && data !== null && !error;
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
    setWindowSource('user');
  }, []);

  // Task W3: a drag-select on the graph narrows the whole page to an
  // arbitrary `{since, until}` window, exactly like a preset. `startDay`/
  // `endDay` are the ordered day-bucket labels `UsageTimelineStacked`'s
  // `useDragSelection` resolves a real drag to.
  const handleSelectRange = useCallback((startDay: string, endDay: string) => {
    setUsageWindow(windowFromDayRange(startDay, endDay));
    setIsCustomWindow(true);
    setWindowSource('user');
  }, []);

  // "custom · ×" chip's clear: return to the currently-highlighted preset's
  // window without changing which preset is highlighted.
  const clearCustomWindow = useCallback(() => {
    setUsageWindow(presetWindow(range));
    setIsCustomWindow(false);
    setWindowSource('user');
  }, [range]);

  const [outliers, setOutliers] = useState<OutlierRun[]>([]);
  // Customer-scoped only: the hosts the run-rows and outliers fetches named in
  // their `X-Rupu-Hosts-Without-Customer` header (each replaced by its latest
  // answer; cleared when the filter goes).
  const [runsHostsWithout, setRunsHostsWithout] = useState<string[]>([]);
  const [outlierHostsWithout, setOutlierHostsWithout] = useState<string[]>([]);
  useEffect(() => {
    setRunsHostsWithout([]);
    setOutlierHostsWithout([]);
  }, [customer]);
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

  // `/api/usage/outliers`: local-only, re-fetches on window. Depends on
  // `usageWindow.since`/`usageWindow.until` (primitives), not the
  // `usageWindow` object itself — `handleSelectRange` (and `presetWindow`)
  // build a fresh window object each call, and keying off the object would
  // risk a spurious refetch loop if that ever stopped being referentially
  // stable (same primitive-deps pattern as `UsageTimeline`'s own effect).
  useEffect(() => {
    let cancelled = false;
    (customer
      ? api.getUsageOutliers(usageWindow, customer, (ids) => {
          if (!cancelled) setOutlierHostsWithout(ids);
        })
      : api.getUsageOutliers(usageWindow)
    )
      .then((rows) => {
        if (!cancelled) setOutliers(rows);
      })
      .catch(() => {
        // A failed tick-driven refresh keeps the last good outliers.
        if (!cancelled && windowSource !== 'tick') setOutliers([]);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed off usageWindow's primitive fields, not the object itself; see comment above.
  }, [usageWindow.since, usageWindow.until, customer]);

  // Live refresh: a preset window ends at "now", so its `until` goes stale the
  // moment it is built — new runs (and a still-running run's growing usage)
  // land after it. While a preset is active, re-derive the window every 30s;
  // the new `until` re-fires the outliers fetch above, `useUsageData`'s local
  // refetch and `UsageTimeline`'s own run-rows fetch through their normal
  // primitive-keyed effects. A drag-selected custom window is a fixed
  // historical span and never ticks. Hidden tabs skip the tick (the next
  // visible one catches up), and the timer is cleared on unmount / range
  // change.
  useEffect(() => {
    if (isCustomWindow) return;
    const t = window.setInterval(() => {
      if (document.visibilityState !== 'visible') return;
      setWindowSource('tick');
      setUsageWindow(presetWindow(range));
    }, USAGE_REFRESH_MS);
    return () => window.clearInterval(t);
  }, [range, isCustomWindow]);

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
    <div className={embedded ? 'space-y-4' : 'space-y-4 p-4'}>
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          {!embedded && (
            <div className="flex flex-wrap items-center gap-2">
              <h1 className="text-lg font-semibold text-ink">Usage</h1>
              <ScopeChip />
            </div>
          )}
          {hosts.length > 0 && (
            <div className="mt-1">
              <HostFreshnessStrip hosts={hosts} />
            </div>
          )}
          {notice && <p className="mt-1 text-xs text-ink-mute">{notice}</p>}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {error && data && (
            <span className="text-xs text-status-failed" title={error.message}>
              refresh failed — showing last good data
            </span>
          )}
          <PivotPicker value={pivot} onChange={(p) => startTransition(() => setPivot(p))} />
          {isCustomWindow && (
            <button
              type="button"
              onClick={clearCustomWindow}
              className="rounded-full border border-border px-2 py-0.5 text-[10px] text-ink-mute hover:bg-surface"
              title="Clear the drag-selected window and return to the active preset"
            >
              custom · ×
            </button>
          )}
          <div className="flex rounded-md border border-border">
            {RANGES.map((r) => (
              <button
                key={r}
                type="button"
                onClick={() => handleRangeChange(r)}
                className={`px-2 py-1 text-xs ${
                  !isCustomWindow && range === r
                    ? 'bg-surface text-ink'
                    : 'text-ink-mute'
                }`}
              >
                {r}
              </button>
            ))}
          </div>
        </div>
      </header>

      {customer && (
        <HostsWithoutCustomerBanner
          hosts={hosts.map((h) => ({ id: h.host_id, name: h.name, state: h.state, reason: h.reason }))}
          without={[...(data?.hostsWithoutCustomer ?? []), ...runsHostsWithout, ...outlierHostsWithout]}
        />
      )}

      {data ? (
        <>
          <UnpricedBanner unpriced={data.unpriced} />

          {/* The headline here is fleet-wide (`/api/usage`, loaded per host by
              `useUsageData` and merged) — deliberately NOT derived from the
              local-only run rows `UsageTimeline` fetches for the graph itself,
              which is why it's passed in rather than computed inside that
              component (see its doc comment). */}
          <UsageTimeline
            customer={customer}
            usageWindow={usageWindow}
            pivot={pivot}
            metric={metric}
            onMetricChange={setMetric}
            filter={filter}
            excludedCount={excludedCount}
            onReset={resetExclusions}
            onRunsLoaded={setRuns}
            onHostsWithoutCustomer={customer ? setRunsHostsWithout : undefined}
            onSelectRange={handleSelectRange}
            pending={isPending || headlineStale}
            background={windowSource === 'tick'}
            hosts={hosts}
            headline={{
              costLabel: formatCost(data.summary.cost_usd),
              pricingError: data.summary.pricing_error,
              subLabel: `${formatTokens(data.summary.total_tokens)} tokens · ${data.summary.runs} runs${
                !data.summary.priced ? ' · partial (see banner above)' : ''
              }${data.excluded.length ? ` · excludes ${data.excluded.join(', ')}` : ''}`,
            }}
          />

          <section className="rounded-lg border border-border bg-panel p-3">
            <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-ink-dim">
              Breakdown by {PIVOT_LABEL[pivot]}
            </h2>
            <ModelBreakdownTable
              rows={breakdown}
              pivot={pivot}
              hosts={hosts}
              selectable
              excludedKeys={excludedKeys}
              onToggleKey={toggleKey}
            />
          </section>

          <section className="rounded-lg border border-border bg-panel p-3">
            <h2 className="mb-2 text-xs font-medium uppercase tracking-wide text-ink-dim">
              Cost outliers{' '}
              <span className="font-normal normal-case text-ink-mute">
                (this host only)
              </span>
            </h2>
            <OutlierPanel outliers={outliers} excludedRunIds={excludedRunIds} onToggleRun={toggleRun} />
          </section>
        </>
      ) : error ? (
        <div className="p-6 text-sm text-status-failed">
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
