// ProjectUsageTimeline — the project-scoped mount of `/usage`'s interactive
// spend-over-time graph (Task U4). Replaces the per-run `UsageBarChart` that
// used to sit in the Runs tab (one bar per run, stacked by token type) with
// the SAME `UsageTimeline` graph component the fleet-wide `/usage` page
// uses, scoped to this project's `workspace_id`.
//
// This component owns its own range/pivot/metric/exclusion-filter state and
// the `getUsageRuns(window, wsId)` fetch — there's no page-level state to
// share it with here, unlike `/usage`, where pivot/filter are shared with a
// breakdown table + outlier panel sourced from OTHER endpoints. That absence
// is also why the breakdown table below is built from `aggregateRuns` over
// the SAME rows `UsageTimeline` fetches (passed back via `onRunsLoaded`,
// so there's no second fetch) rather than `/api/usage`: that endpoint fans
// out across every registered host and has no `workspace_id` filter (Task
// U1 only added scoping to `/api/usage/runs`), so it cannot produce a
// project-scoped breakdown.
//
// NO OUTLIER PANEL: `/api/usage/outliers` is local-only like `/api/usage/runs`
// but was NOT given a `workspace_id` filter either — there is nothing to
// scope an outlier panel to, and rendering the fleet-wide list here would
// misrepresent it as project-scoped. Deferred; see the U4 report for detail.

import { useCallback, useEffect, useMemo, useState } from 'react';
import { presetWindow, type DashboardRange, type UsageRunRow, type UsageWindow } from '../../lib/api';
import { formatCost, formatTokens } from '../../lib/usage';
import { aggregateRuns, type TimelineFilter } from '../../lib/usage/buildTimeline';
import { PivotPicker, PIVOT_LABEL, type Pivot } from '../usage/PivotPicker';
import UsageTimeline from '../usage/UsageTimeline';
import { type UsageMetric } from '../dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../dashboard/ModelBreakdownTable';

const RANGES: DashboardRange[] = ['7d', '30d', 'all'];

function toggleInSet(set: Set<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

export default function ProjectUsageTimeline({ wsId }: { wsId: string }) {
  const [range, setRange] = useState<DashboardRange>('30d');
  // The `{since, until}` window driving the fetch below (Task W2) — `range`
  // is kept alongside purely for the 7/30/All button highlighting. Held in
  // state (not recomputed inline from `range` on every render) so its
  // object identity is stable across renders that don't change it.
  const [window, setWindow] = useState<UsageWindow>(() => presetWindow('30d'));
  const [pivot, setPivot] = useState<Pivot>('model');
  const [metric, setMetric] = useState<UsageMetric>('cost');
  const [runs, setRuns] = useState<UsageRunRow[]>([]);

  const handleRangeChange = useCallback((r: DashboardRange) => {
    setRange(r);
    setWindow(presetWindow(r));
  }, []);

  const [excludedKeys, setExcludedKeys] = useState<Set<string>>(new Set());
  const [excludedRunIds, setExcludedRunIds] = useState<Set<string>>(new Set());

  // Same convention as `/usage`: a pivot-key exclusion is only meaningful
  // under the dimension it was excluded from — clear it on pivot switch.
  useEffect(() => {
    setExcludedKeys(new Set());
  }, [pivot]);

  const filter = useMemo<TimelineFilter>(
    () => ({ excludedKeys, excludedRunIds }),
    [excludedKeys, excludedRunIds],
  );

  const toggleKey = useCallback((key: string) => {
    setExcludedKeys((prev) => toggleInSet(prev, key));
  }, []);
  const resetExclusions = useCallback(() => {
    setExcludedKeys(new Set());
    setExcludedRunIds(new Set());
  }, []);

  const excludedCount = excludedKeys.size + excludedRunIds.size;

  // Unfiltered breakdown for the table — every pivot key must stay
  // clickable even while excluded, or there'd be no way to re-include it.
  const breakdown = useMemo(() => aggregateRuns(runs, pivot), [runs, pivot]);

  // A local-only headline analogous to `/usage`'s fleet-wide one, computed
  // from the SAME rows (not `/api/usage`, which has no workspace scope).
  // `cost_usd` is null only when NOTHING priced — mirrors `UsageSummary`'s
  // documented meaning; a real $0 total must not render as unpriced.
  const sawPriced = breakdown.some((r) => r.cost_usd !== null);
  const totalCost = breakdown.reduce((acc, r) => acc + (r.cost_usd ?? 0), 0);
  const totalTokens = breakdown.reduce((acc, r) => acc + r.total_tokens, 0);
  const totalRuns = new Set(runs.map((r) => r.run_id)).size;
  const anyUnpriced = breakdown.some((r) => !r.priced);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <PivotPicker value={pivot} onChange={setPivot} />
        <div className="flex rounded-md border border-border">
          {RANGES.map((r) => (
            <button
              key={r}
              type="button"
              onClick={() => handleRangeChange(r)}
              className={`px-2 py-1 text-xs ${
                range === r ? 'bg-surface text-ink' : 'text-ink-mute'
              }`}
            >
              {r}
            </button>
          ))}
        </div>
      </div>

      <UsageTimeline
        workspaceId={wsId}
        window={window}
        pivot={pivot}
        metric={metric}
        onMetricChange={setMetric}
        filter={filter}
        excludedCount={excludedCount}
        onReset={resetExclusions}
        onRunsLoaded={setRuns}
        headline={{
          costLabel: formatCost(sawPriced ? totalCost : null),
          subLabel: `${formatTokens(totalTokens)} tokens · ${totalRuns} runs${
            anyUnpriced ? ' · partial' : ''
          }`,
        }}
      />

      <div className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
        <h3 className="mb-2 text-meta font-semibold uppercase tracking-widest text-ink-mute">
          Breakdown by {PIVOT_LABEL[pivot]}
        </h3>
        <ModelBreakdownTable
          rows={breakdown}
          pivot={pivot}
          selectable
          excludedKeys={excludedKeys}
          onToggleKey={toggleKey}
        />
      </div>
    </div>
  );
}
