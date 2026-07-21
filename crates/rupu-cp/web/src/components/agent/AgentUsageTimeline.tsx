// AgentUsageTimeline — the agent-scoped mount of the shared spend-over-time
// graph, for the Agent detail page (route `/agents/:name`). Shaped like
// `ProjectUsageTimeline`: owns its own range/window/pivot/metric/exclusion
// state, renders the same inline 7/30/All + custom-drag chip + PivotPicker
// row (`UsageRangeControls`) and a breakdown table below the chart.
//
// UNLIKE `ProjectUsageTimeline`, this component does NOT delegate to
// `UsageTimeline` — there is no `workspace_id`-style query param scoping
// `GET /api/usage/runs` to one agent (Task U1 only ever added
// `workspace_id`). `UsageTimeline` always builds its own graph from its own
// unfiltered fetch; handing it `onRunsLoaded` only gets you the rows AFTER
// its internal (agent-unaware) `buildTimeline` already ran on all of them,
// which would render every agent's spend, not just this one's. So this
// component fetches `getUsageRuns(usageWindow)` itself (no `workspaceId`),
// filters the returned rows to `row.agent === agent` BEFORE calling
// `buildTimeline`/`aggregateRuns`, and renders `UsageTimelineStacked`
// directly — mirroring `UsageTimeline`'s own internal wiring (see that
// component) rather than reusing it as a black box.
//
// `UsageRunRow.agent` (`rupu-cp/src/api/usage.rs`) is the agent NAME as
// recorded in the run's transcript (`rupu_transcript::aggregate`'s `row.agent`)
// — the same identifier `/api/agents/:name` and every other per-agent surface
// (`AgentSummary`, `AgentRunRow.agent_name`) key on, so filtering rows by
// `row.agent === agent` (the route's `:name` param) is a same-identity
// comparison, not a heuristic match.

import { useCallback, useEffect, useMemo, useState, useTransition } from 'react';
import {
  api,
  presetWindow,
  windowFromDayRange,
  type DashboardRange,
  type UsageRunRow,
  type UsageWindow,
} from '../../lib/api';
import { formatCost, formatTokens } from '../../lib/usage';
import { aggregateRuns, buildTimeline, type TimelineFilter } from '../../lib/usage/buildTimeline';
import { PIVOT_LABEL, type Pivot } from '../usage/PivotPicker';
import UsageRangeControls from '../usage/UsageRangeControls';
import UsageTimelineStacked, { type UsageMetric } from '../dashboard/UsageTimelineStacked';
import ModelBreakdownTable from '../dashboard/ModelBreakdownTable';
import { Spinner } from '../ui/Spinner';

const METRICS: UsageMetric[] = ['cost', 'tokens'];

function toggleInSet(set: Set<string>, key: string): Set<string> {
  const next = new Set(set);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

export default function AgentUsageTimeline({ agent }: { agent: string }) {
  const [range, setRange] = useState<DashboardRange>('30d');
  const [usageWindow, setUsageWindow] = useState<UsageWindow>(() => presetWindow('30d'));
  const [isCustomWindow, setIsCustomWindow] = useState(false);
  const [pivot, setPivot] = useState<Pivot>('model');
  const [metric, setMetric] = useState<UsageMetric>('cost');
  const [isPending, startTransition] = useTransition();

  const handleRangeChange = useCallback((r: DashboardRange) => {
    setRange(r);
    setUsageWindow(presetWindow(r));
    setIsCustomWindow(false);
  }, []);

  const handleSelectRange = useCallback((startDay: string, endDay: string) => {
    setUsageWindow(windowFromDayRange(startDay, endDay));
    setIsCustomWindow(true);
  }, []);

  const clearCustomWindow = useCallback(() => {
    setUsageWindow(presetWindow(range));
    setIsCustomWindow(false);
  }, [range]);

  // `null` = "haven't heard back from the fetch yet" (distinct from `[]`,
  // "fetched, genuinely zero rows for this agent") — same convention
  // `UsageTimeline` uses, so the initial load gets a real skeleton instead
  // of a premature "no usage" state.
  const [allRuns, setAllRuns] = useState<UsageRunRow[] | null>(null);
  const [isFetching, setIsFetching] = useState(false);

  // `GET /api/usage/runs` — unscoped (no `workspaceId`), because there is no
  // agent-scoped query param. Depends on the window's primitive fields, not
  // the `usageWindow` object itself, same as `UsageTimeline`'s effect — a
  // fresh `presetWindow(...)` object every render must not spuriously
  // refetch.
  useEffect(() => {
    let cancelled = false;
    setIsFetching(true);
    api
      .getUsageRuns(usageWindow)
      .then((rows) => {
        if (cancelled) return;
        setAllRuns(rows);
      })
      .catch(() => {
        if (!cancelled) setAllRuns([]);
      })
      .finally(() => {
        if (!cancelled) setIsFetching(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed off usageWindow's primitive fields, not the object itself; see comment above.
  }, [usageWindow.since, usageWindow.until]);

  // The client-side agent filter — every downstream computation (graph +
  // breakdown table) reads from `runs`, never `allRuns`.
  const runs = useMemo(() => (allRuns ?? []).filter((r) => r.agent === agent), [allRuns, agent]);
  const isInitialLoad = allRuns === null;

  const [excludedKeys, setExcludedKeys] = useState<Set<string>>(new Set());
  const [excludedRunIds, setExcludedRunIds] = useState<Set<string>>(new Set());

  useEffect(() => {
    setExcludedKeys(new Set());
  }, [pivot]);

  const filter = useMemo<TimelineFilter>(
    () => ({ excludedKeys, excludedRunIds }),
    [excludedKeys, excludedRunIds],
  );

  const toggleKey = useCallback((key: string) => {
    startTransition(() => {
      setExcludedKeys((prev) => toggleInSet(prev, key));
    });
  }, []);
  const resetExclusions = useCallback(() => {
    startTransition(() => {
      setExcludedKeys(new Set());
      setExcludedRunIds(new Set());
    });
  }, []);

  const excludedCount = excludedKeys.size + excludedRunIds.size;

  const timeline = useMemo(() => buildTimeline(runs, pivot, filter, 'day'), [runs, pivot, filter]);

  // Unfiltered breakdown for the table — every pivot key must stay
  // clickable even while excluded, or there'd be no way to re-include it.
  const breakdown = useMemo(() => aggregateRuns(runs, pivot), [runs, pivot]);

  const sawPriced = breakdown.some((r) => r.cost_usd !== null);
  const totalCost = breakdown.reduce((acc, r) => acc + (r.cost_usd ?? 0), 0);
  const totalTokens = breakdown.reduce((acc, r) => acc + r.total_tokens, 0);
  const totalRuns = new Set(runs.map((r) => r.run_id)).size;
  const anyUnpriced = breakdown.some((r) => !r.priced);

  const isUpdating = !isInitialLoad && (isFetching || isPending);

  return (
    <div className="space-y-4">
      <UsageRangeControls
        range={range}
        isCustomWindow={isCustomWindow}
        onRangeChange={handleRangeChange}
        onClearCustom={clearCustomWindow}
        pivot={pivot}
        onPivotChange={(p) => startTransition(() => setPivot(p))}
      />

      <section className="bg-panel border border-border rounded-xl shadow-card px-4 py-3">
        <div className="mb-2 flex flex-wrap items-start justify-between gap-2">
          <div>
            <h3 className="flex items-center gap-1.5 text-meta font-semibold uppercase tracking-widest text-ink-mute">
              Spend over time
              {isUpdating && <Spinner size="sm" label="updating" />}
            </h3>
            <p className="mt-0.5 text-2xl font-semibold tabular-nums text-ink">
              {formatCost(sawPriced ? totalCost : null)}
            </p>
            <p className="text-xs text-ink-mute">
              {formatTokens(totalTokens)} tokens · {totalRuns} runs{anyUnpriced ? ' · partial' : ''}
            </p>
          </div>
          <div className="flex items-center gap-2">
            {excludedCount > 0 && (
              <button
                type="button"
                onClick={resetExclusions}
                className="rounded-full border border-border px-2 py-0.5 text-[10px] text-ink-mute hover:bg-surface"
              >
                Excluded ({excludedCount}) · reset
              </button>
            )}
            <div className="flex rounded-md border border-border">
              {METRICS.map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => setMetric(m)}
                  className={`px-2 py-1 text-xs capitalize ${
                    metric === m ? 'bg-surface text-ink' : 'text-ink-mute'
                  }`}
                >
                  {m}
                </button>
              ))}
            </div>
          </div>
        </div>
        {/* Same local-only caveat as `UsageTimeline` — `/api/usage/runs` has
            no host fan-out. */}
        <p className="mb-2 text-[10px] uppercase tracking-wide text-ink-mute">local host only</p>
        <div className={isUpdating ? 'opacity-70 transition-opacity duration-200' : 'transition-opacity duration-200'}>
          <UsageTimelineStacked
            buckets={timeline}
            metric={metric}
            pivot={pivot}
            onSelectRange={handleSelectRange}
            loading={isInitialLoad}
          />
        </div>
      </section>

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
