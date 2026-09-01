// Timeline view — one swimlane per endpoint (grouped under ASN-org
// headers, server-ordered), heat cells per time bucket, an active-runs
// correlation strip on top, per-lane stats on the right.
//
// Contracts (v3 mockup + honesty rules):
//   - Cell shade = calls in that bucket against the GLOBAL max across all
//     lanes (`opacity = 0.12 + 0.78·calls/max`) — call-weighted, never
//     bytes. A 3px `status.failed` baseline marks buckets with errors.
//   - Click a cell (with calls) → zoom the shared window to that bucket;
//     the owner keeps a zoom stack so "Zoom out" can unwind.
//   - Click a lane's endpoint label → toggle its host filter. Lanes come
//     filtered by everything EXCEPT the host dimension (server facet
//     semantics), so a selected endpoint's siblings stay visible.
//   - `p50/p95` absent renders an em dash, never `0 ms`; a lane whose
//     least-observable contributor is coarse carries the amber tag.

import { Fragment, useMemo } from 'react';
import { absoluteTime } from '../../../lib/time';
import { useThemeColors } from '../../../lib/useThemeColors';
import type { TimelineView as TimelineViewData } from '../../../lib/netflow';
import { cn } from '../../../lib/cn';
import { Button } from '../../ui/Button';
import { EmptyState } from '../../ui/EmptyState';
import { FidelityBadge } from '../FidelityBadge';
import {
  netflowEmptyStateHint,
  netflowRangeEmptyHint,
  netflowWindowApplied,
  type NetflowScope,
  type NetflowWindowEcho,
} from '../ScopeDisclosure';

export interface TimelineViewProps {
  timeline: TimelineViewData;
  selectedHosts: string[];
  onToggleHost: (endpointKey: string) => void;
  onZoomBucket: (fromIso: string, toIso: string) => void;
  canZoomOut: boolean;
  onZoomOut: () => void;
  /** Selects the scope-limit sentence for the empty state. */
  scope: NetflowScope;
  appliedWindow?: NetflowWindowEcho;
}

const LABEL_W = 'w-[236px]';
const STATS_W = 'w-[170px]';

export function TimelineView({
  timeline,
  selectedHosts,
  onToggleHost,
  onZoomBucket,
  canZoomOut,
  onZoomOut,
  scope,
  appliedWindow,
}: TimelineViewProps) {
  const colors = useThemeColors();
  const t0 = Date.parse(timeline.from);
  const span = Math.max(1, Date.parse(timeline.to) - t0);

  // Every lane shares one set of bucket boundaries (dense, fixed count),
  // so the ISO strings + tooltip time labels are computed ONCE per
  // timeline — not two Date constructions plus an Intl format per cell
  // per render. Reduce-based maxima: a spread of lanes×84 numbers as
  // function arguments hits engine argument limits on wide fleets.
  const laneBucketCount = timeline.lanes[0]?.buckets.length ?? 0;
  const boundaries = useMemo(() => {
    const at = (i: number, count: number) =>
      new Date(t0 + (span * i) / Math.max(1, count));
    return {
      laneIso: Array.from({ length: laneBucketCount + 1 }, (_, i) =>
        at(i, laneBucketCount).toISOString(),
      ),
      laneLabel: Array.from({ length: laneBucketCount }, (_, i) =>
        absoluteTime(at(i, laneBucketCount).toISOString()),
      ),
      runLabel: Array.from({ length: timeline.runs.length }, (_, i) =>
        absoluteTime(at(i, timeline.runs.length).toISOString()),
      ),
    };
  }, [t0, span, laneBucketCount, timeline.runs.length]);

  if (timeline.lanes.length === 0) {
    // Both branches carry the scope-limit sentence — an empty timeline
    // must never imply "no network activity happened".
    return netflowWindowApplied(appliedWindow) ? (
      <EmptyState
        title="No endpoint activity in this range"
        hint={
          <>
            {netflowRangeEmptyHint()} {netflowEmptyStateHint(scope)}
          </>
        }
      />
    ) : (
      <EmptyState
        title="No endpoint activity for this scope"
        hint={netflowEmptyStateHint(scope)}
      />
    );
  }

  const globalMax = Math.max(
    1,
    timeline.lanes.reduce(
      (m, l) => l.buckets.reduce((mm, b) => Math.max(mm, b.calls), m),
      0,
    ),
  );
  const maxRuns = Math.max(1, timeline.runs.reduce((m, n) => Math.max(m, n), 0));

  let lastOrg: string | null = null;

  return (
    <div className="overflow-x-auto">
      <div className="min-w-[720px]">
        <div className="mb-0.5 flex items-center gap-3">
          <span className={cn('flex-none', LABEL_W)} />
          <div className="flex flex-1 justify-between text-meta text-ink-mute">
            <span>{absoluteTime(timeline.from)}</span>
            <span>{absoluteTime(new Date(t0 + span / 2).toISOString())}</span>
            <span>{absoluteTime(timeline.to)}</span>
          </div>
          <span className={cn('flex-none text-right', STATS_W)}>
            {canZoomOut && (
              <Button variant="secondary" size="sm" onClick={onZoomOut}>
                Zoom out
              </Button>
            )}
          </span>
        </div>
        <div className="flex items-center gap-3 border-b border-border py-1.5">
          <span
            className={cn(
              'flex-none text-meta font-semibold uppercase tracking-wider text-ink-mute',
              LABEL_W,
            )}
          >
            Active runs
          </span>
          <div className="flex h-3.5 flex-1 gap-px">
            {timeline.runs.map((n, i) => (
              <div
                key={i}
                title={`${n} active runs · ${boundaries.runLabel[i]}`}
                className="flex-1 rounded-[1px]"
                style={{
                  background: colors.alpha('inkMute', Math.min(0.75, (n / maxRuns) * 0.75)),
                }}
              />
            ))}
          </div>
          <span className={cn('flex-none text-right text-meta text-ink-mute', STATS_W)}>
            {timeline.runs_in_window} runs in window
          </span>
        </div>
        {timeline.lanes.map((lane) => {
          const endpointKey = `${lane.host}:${lane.port}`;
          const groupStart = lane.org_id !== lastOrg;
          lastOrg = lane.org_id;
          const on = selectedHosts.includes(endpointKey);
          return (
            <Fragment key={endpointKey}>
              {groupStart && (
                <p className="mb-0.5 mt-2.5 text-meta font-semibold uppercase tracking-wider text-ink-mute">
                  {lane.org}
                  {lane.asn != null ? ` · AS${lane.asn}` : ''}
                </p>
              )}
              <div
                className={cn(
                  'flex items-center gap-3 rounded-md py-1',
                  on && 'bg-brand-50',
                )}
              >
                <button
                  type="button"
                  aria-pressed={on}
                  onClick={() => onToggleHost(endpointKey)}
                  title="Filter everything to this endpoint"
                  className={cn('flex flex-none items-center gap-1.5 pl-1 text-left', LABEL_W)}
                >
                  <span className="truncate font-mono text-note text-ink">{endpointKey}</span>
                  <span className="flex-none">
                    <FidelityBadge fidelity={lane.fidelity} />
                  </span>
                </button>
                <div className="flex h-[22px] flex-1 gap-px">
                  {lane.buckets.map((b, i) => {
                    const from = boundaries.laneIso[i];
                    const to = boundaries.laneIso[i + 1];
                    return (
                      <button
                        key={i}
                        type="button"
                        tabIndex={-1}
                        title={`${b.calls} calls${b.errors ? ` · ${b.errors} errors` : ''} · ${boundaries.laneLabel[i]}`}
                        onClick={() => {
                          if (b.calls > 0) onZoomBucket(from, to);
                        }}
                        className={cn(
                          'flex flex-1 flex-col justify-end rounded-[1px] border-0 p-0',
                          b.calls > 0 && 'cursor-pointer',
                        )}
                        style={{
                          background:
                            b.calls > 0
                              ? colors.alpha('brand.500', 0.12 + 0.78 * (b.calls / globalMax))
                              : 'transparent',
                        }}
                      >
                        <span
                          className="block h-[3px] w-full rounded-[1px]"
                          style={{
                            background: b.errors > 0 ? colors.alpha('status.failed', 0.9) : 'transparent',
                          }}
                        />
                      </button>
                    );
                  })}
                </div>
                <span
                  className={cn(
                    'flex flex-none justify-end gap-2.5 text-note tabular-nums text-ink-mute',
                    STATS_W,
                  )}
                >
                  <span className="text-err">{lane.errors > 0 ? `${lane.errors} err` : ''}</span>
                  <span className="w-14 text-right">{lane.calls} calls</span>
                  <span className="w-[52px] text-right">
                    {lane.p95_ms != null ? `${lane.p95_ms} ms` : '—'}
                  </span>
                </span>
              </div>
            </Fragment>
          );
        })}
        <p className="mt-2.5 text-meta text-ink-mute">
          Cell shade = calls in that slice · red baseline = errors · click a cell to zoom the
          window to it · amber tag = coarse fidelity.
        </p>
      </div>
    </div>
  );
}

export default TimelineView;
