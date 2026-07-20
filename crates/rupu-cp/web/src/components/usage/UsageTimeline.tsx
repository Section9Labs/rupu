// UsageTimeline — the "Spend over time" graph section (Task U3/U4), reused
// verbatim by both `/usage` (all local runs) and the Projects page's Runs
// tab (one project's runs). This is the literal "same graph" the redesign
// asks for: it owns the `GET /api/usage/runs` fetch (scoped by the optional
// `workspaceId`) and the `buildTimeline` bucket+stack computation, and
// renders exactly the chart + its header controls U3 built for `/usage`.
//
// Pivot / metric / the exclusion filter are CONTROLLED props, not internal
// state — `/usage` shares all three with its breakdown table and outlier
// panel (toggling a breakdown row must rescale THIS graph instantly), so
// those live in the caller's state, not here. `headline` is likewise
// supplied by the caller because its correct SOURCE differs by page:
// `/usage`'s headline is fleet-wide (`GET /api/usage`, fans out across
// hosts); a project view has no such endpoint to draw from and computes its
// own local-only total. Faking one inside this component would risk the two
// silently drifting.
//
// `onRunsLoaded` (optional) hands the raw fetched rows back to the caller —
// added for the Projects page, which needs the same rows to build its own
// breakdown table (Task U4) without a second, duplicate fetch.

import { useEffect, useMemo, useState } from 'react';
import { api, type DashboardRange, type HostFreshness, type Pivot, type UsageRunRow } from '../../lib/api';
import { buildTimeline, type TimelineFilter } from '../../lib/usage/buildTimeline';
import UsageTimelineStacked, { type UsageMetric } from '../dashboard/UsageTimelineStacked';

const METRICS: UsageMetric[] = ['cost', 'tokens'];

export default function UsageTimeline({
  workspaceId,
  range,
  pivot,
  metric,
  onMetricChange,
  filter,
  excludedCount,
  onReset,
  hosts,
  headline,
  onRunsLoaded,
}: {
  /** Scopes the fetch to one project's runs. Omitted on `/usage` — all
   *  local runs. */
  workspaceId?: string;
  range: DashboardRange;
  pivot: Pivot;
  metric: UsageMetric;
  onMetricChange: (m: UsageMetric) => void;
  filter: TimelineFilter;
  excludedCount: number;
  onReset: () => void;
  hosts?: HostFreshness[];
  headline: { costLabel: string; subLabel: string };
  onRunsLoaded?: (rows: UsageRunRow[]) => void;
}) {
  const [runs, setRuns] = useState<UsageRunRow[]>([]);

  // `GET /api/usage/runs`: flat per-`(run × model)` rows behind the graph.
  // Pivot- and filter-independent — only re-fetches on range/workspaceId;
  // every pivot switch or exclude toggle re-runs `buildTimeline` in memory.
  useEffect(() => {
    let cancelled = false;
    // Called with exactly one argument when `workspaceId` is omitted (rather
    // than an explicit `undefined` second arg) so a caller-side spy
    // assertion like `toHaveBeenCalledWith(range)` — matching how `/usage`
    // itself called `getUsageRuns` before this component existed — still
    // matches.
    (workspaceId ? api.getUsageRuns(range, workspaceId) : api.getUsageRuns(range))
      .then((rows) => {
        if (cancelled) return;
        setRuns(rows);
        onRunsLoaded?.(rows);
      })
      .catch(() => {
        // The graph is secondary to whatever summary sits above it; a
        // failure here should not blank the whole page.
        if (!cancelled) setRuns([]);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- onRunsLoaded is a caller-supplied callback, not a re-fetch trigger.
  }, [range, workspaceId]);

  const timeline = useMemo(() => buildTimeline(runs, pivot, filter, 'day'), [runs, pivot, filter]);

  return (
    <section className="rounded-lg border border-[rgb(var(--c-border))] bg-[rgb(var(--c-panel))] p-3">
      <div className="mb-2 flex flex-wrap items-start justify-between gap-2">
        <div>
          <h2 className="text-xs font-medium uppercase tracking-wide text-[rgb(var(--c-ink-dim))]">
            Spend over time
          </h2>
          <p className="mt-0.5 text-2xl font-semibold tabular-nums text-[rgb(var(--c-ink))]">
            {headline.costLabel}
          </p>
          <p className="text-xs text-[rgb(var(--c-ink-mute))]">{headline.subLabel}</p>
        </div>
        <div className="flex items-center gap-2">
          {excludedCount > 0 && (
            <button
              type="button"
              onClick={onReset}
              className="rounded-full border border-[rgb(var(--c-border))] px-2 py-0.5 text-[10px] text-[rgb(var(--c-ink-mute))] hover:bg-[rgb(var(--c-surface))]"
            >
              Excluded ({excludedCount}) · reset
            </button>
          )}
          <div className="flex rounded-md border border-[rgb(var(--c-border))]">
            {METRICS.map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => onMetricChange(m)}
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
      </div>
      {/* This graph is fed by `/api/usage/runs`, which has no host fan-out —
          say so, or a multi-host operator reads it as fleet-wide too. */}
      <p className="mb-2 text-[10px] uppercase tracking-wide text-[rgb(var(--c-ink-mute))]">
        local host only
      </p>
      <UsageTimelineStacked buckets={timeline} metric={metric} pivot={pivot} hosts={hosts} />
    </section>
  );
}
