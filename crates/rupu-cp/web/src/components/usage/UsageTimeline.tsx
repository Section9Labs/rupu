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
import { api, type HostFreshness, type Pivot, type UsageRunRow, type UsageWindow } from '../../lib/api';
import { buildTimeline, type TimelineFilter } from '../../lib/usage/buildTimeline';
import UsageTimelineStacked, { type UsageMetric } from '../dashboard/UsageTimelineStacked';
import { Spinner } from '../ui/Spinner';

const METRICS: UsageMetric[] = ['cost', 'tokens'];

export default function UsageTimeline({
  workspaceId,
  usageWindow,
  pivot,
  metric,
  onMetricChange,
  filter,
  excludedCount,
  onReset,
  hosts,
  headline,
  onRunsLoaded,
  onSelectRange,
  pending,
}: {
  /** Scopes the fetch to one project's runs. Omitted on `/usage` — all
   *  local runs. */
  workspaceId?: string;
  /** The `{since, until}` window driving every fetch below — a preset
   *  (7d/30d/all) is just a window ending "now" (see `presetWindow`); a
   *  drag-selected custom window (Task W3, via `onSelectRange` below) is the
   *  same shape. Named `usageWindow` (not `window`) to avoid shadowing the
   *  global `Window`. */
  usageWindow: UsageWindow;
  pivot: Pivot;
  metric: UsageMetric;
  onMetricChange: (m: UsageMetric) => void;
  filter: TimelineFilter;
  excludedCount: number;
  onReset: () => void;
  hosts?: HostFreshness[];
  headline: { costLabel: string; subLabel: string };
  onRunsLoaded?: (rows: UsageRunRow[]) => void;
  /**
   * Marquee drag-select (Task W3) — pure passthrough to
   * `UsageTimelineStacked`'s prop of the same name. The caller (which owns
   * the `usageWindow` state) is responsible for converting the ordered
   * `(startDay, endDay)` day-bucket labels into a `UsageWindow` (see
   * `windowFromDayRange` in `../../lib/api`) and setting its state — this
   * component has no window state of its own to update.
   */
  onSelectRange?: (startDay: string, endDay: string) => void;
  /**
   * An external "this is about to change" signal — e.g. the caller's own
   * `useTransition` wrapping a pivot switch or a filter-exclusion toggle
   * (both of which land here as props, not state, so THIS component can't
   * start its own transition around them). OR'd together with the internal
   * network-refetch flag below to decide whether to show the subtle
   * "updating" affordance. Optional; omitting it just means the affordance
   * only reacts to actual refetches (window/workspace changes), which is
   * still correct — just less proactive.
   */
  pending?: boolean;
}) {
  // `null` = "haven't heard back from the fetch yet" — distinct from `[]`
  // ("fetched, genuinely zero rows"). Without this distinction the graph
  // below showed the exact same "No usage recorded yet" copy during the
  // initial load as it did for a real empty result.
  const [runs, setRuns] = useState<UsageRunRow[] | null>(null);
  // True for the whole lifetime of the `getUsageRuns` request below —
  // covers both the very first load and any later refetch triggered by a
  // window/workspace change (drag-select, preset range button, or the
  // Projects page switching project).
  const [isFetching, setIsFetching] = useState(false);

  // `GET /api/usage/runs`: flat per-`(run × model)` rows behind the graph.
  // Depends on `usageWindow.since`/`usageWindow.until` (primitives), not the
  // `usageWindow` object itself — callers are free to pass a
  // freshly-computed window on every render (a plain `presetWindow(...)`
  // call is not memoized) without spuriously refetching just because the
  // object reference changed. Pivot- and filter-independent regardless —
  // every pivot switch or exclude toggle re-runs `buildTimeline` in memory,
  // no refetch.
  useEffect(() => {
    let cancelled = false;
    setIsFetching(true);
    // Called with exactly one argument when `workspaceId` is omitted (rather
    // than an explicit `undefined` second arg) so a caller-side spy
    // assertion like `toHaveBeenCalledWith(usageWindow)` — matching how
    // `/usage` itself called `getUsageRuns` before this component existed —
    // still matches.
    (workspaceId ? api.getUsageRuns(usageWindow, workspaceId) : api.getUsageRuns(usageWindow))
      .then((rows) => {
        if (cancelled) return;
        setRuns(rows);
        onRunsLoaded?.(rows);
      })
      .catch(() => {
        // The graph is secondary to whatever summary sits above it; a
        // failure here should not blank the whole page. `[]` (not `null`)
        // so a failed fetch still counts as "loaded" — an infinite skeleton
        // would be worse than a quiet empty state.
        if (!cancelled) setRuns([]);
      })
      .finally(() => {
        if (!cancelled) setIsFetching(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- onRunsLoaded is a caller-supplied callback, not a re-fetch trigger; `usageWindow` itself is intentionally omitted in favor of its primitive fields (see doc comment above).
  }, [usageWindow.since, usageWindow.until, workspaceId]);

  const isInitialLoad = runs === null;
  const timeline = useMemo(() => buildTimeline(runs ?? [], pivot, filter, 'day'), [runs, pivot, filter]);
  // Subtle "updating" affordance — only once the graph has shown real data
  // at least once (the initial load gets the full skeleton via `loading`
  // below instead, not this dimmed-overlay treatment).
  const isUpdating = !isInitialLoad && (isFetching || !!pending);

  return (
    <section className="rounded-lg border border-border bg-panel p-3">
      <div className="mb-2 flex flex-wrap items-start justify-between gap-2">
        <div>
          <h2 className="flex items-center gap-1.5 text-xs font-medium uppercase tracking-wide text-ink-dim">
            Spend over time
            {/* Subtle "this is refreshing" cue — a ~0.3s window/filter
                refetch should read as intentional, not a stall. Only shown
                once the graph has already painted once (see `isUpdating`). */}
            {isUpdating && <Spinner size="sm" label="updating" />}
          </h2>
          <p className="mt-0.5 text-2xl font-semibold tabular-nums text-ink">
            {headline.costLabel}
          </p>
          <p className="text-xs text-ink-mute">{headline.subLabel}</p>
        </div>
        <div className="flex items-center gap-2">
          {excludedCount > 0 && (
            <button
              type="button"
              onClick={onReset}
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
                onClick={() => onMetricChange(m)}
                className={`px-2 py-1 text-xs capitalize ${
                  metric === m
                    ? 'bg-surface text-ink'
                    : 'text-ink-mute'
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
      <p className="mb-2 text-[10px] uppercase tracking-wide text-ink-mute">
        local host only
      </p>
      {/* `opacity-70` only kicks in once the graph has real data on screen
          (see `isUpdating`) — a light dim rather than a full-page block, so
          a ~0.3s window/pivot/filter update reads as intentional instead of
          a stall. The initial load never dims; it gets the skeleton via
          `loading` below instead. */}
      <div className={isUpdating ? 'opacity-70 transition-opacity duration-200' : 'transition-opacity duration-200'}>
        <UsageTimelineStacked
          buckets={timeline}
          metric={metric}
          pivot={pivot}
          hosts={hosts}
          onSelectRange={onSelectRange}
          loading={isInitialLoad}
        />
      </div>
    </section>
  );
}
