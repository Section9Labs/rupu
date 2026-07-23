// KeyPointTiles — the operator's glance-questions, each a number (spec §5.2).
//
// Replaces the swimlane. The swimlane's only irreplaceable signal was
// duration-outlier detection — "is a run stuck". That is a key point, not a
// chart: surface it directly as "Active now: 3 · longest 2h14m —
// nightly-review →", linking to /runs for the bars. Everything else the
// swimlane showed (which runs, on which host) is a list, and lists live on
// /runs.
//
// Tiles that mean "the system is blocked on you" (AwaitingApproval, Paused)
// are SEPARATE tiles (unlike AttentionRow's combined "Blocked on you"
// number) and take visual weight when nonzero — they are the only states
// where nothing moves until the operator acts.
//
// The `Option`/`null` discipline is load-bearing: `findings_open` is `null`
// when no reporting host supplies it (never rendered as a fabricated `0`),
// and a `findings_partial` sum is marked, never presented as complete.

import { Link } from 'react-router-dom';
import { AreaChart, Area, ResponsiveContainer } from 'recharts';
import { useThemeColors } from '../../lib/useThemeColors';
import { formatDuration } from '../../lib/duration';
import type { ActiveCounts, ActiveLongest, TerminalBucket } from '../../lib/api';

function TileShell({
  testId,
  weighted,
  weightVar,
  className,
  children,
}: {
  testId: string;
  weighted: boolean;
  weightVar: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div
      data-testid={testId}
      className={`rounded-lg border p-3 ${
        weighted
          ? `border-[rgb(var(${weightVar}))] bg-[rgb(var(--c-surface))]`
          : 'border-border bg-panel'
      } ${className ?? ''}`}
    >
      {children}
    </div>
  );
}

function TileLabel({ children }: { children: React.ReactNode }) {
  return <div className="text-xs text-ink-dim">{children}</div>;
}

export function KeyPointTiles({
  active,
  activeLongest,
  terminalBuckets,
  findingsOpen,
  findingsPartial,
}: {
  active: ActiveCounts;
  /** The single longest currently-running run, fleet-wide. `null`/`undefined`
   *  when nothing is running — render just the running count. */
  activeLongest: ActiveLongest | null | undefined;
  /** Same series `TerminalTrend` renders, so the Failed sparkline and the
   *  Success-rate tile agree with the outcomes chart by construction. */
  terminalBuckets: TerminalBucket[];
  /** `null` = nobody reported. Render "—", never "0". */
  findingsOpen: number | null;
  /** True = the number below is a partial sum. Mark it; never imply completeness. */
  findingsPartial: boolean;
}) {
  const colors = useThemeColors();

  const failedSeries = terminalBuckets.map((b) => ({ ts: b.ts, failed: b.failed }));
  const failedTotal = terminalBuckets.reduce((s, b) => s + b.failed, 0);

  const terminalTotal = terminalBuckets.reduce(
    (s, b) => s + b.completed + b.failed + b.rejected + b.cancelled,
    0,
  );
  const completedTotal = terminalBuckets.reduce((s, b) => s + b.completed, 0);
  const successRate = terminalTotal > 0 ? Math.round((completedTotal / terminalTotal) * 100) : null;

  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
      <TileShell testId="tile-awaiting" weighted={active.awaiting_approval > 0} weightVar="--c-status-awaiting">
        <TileLabel>Awaiting you</TileLabel>
        <div
          className={`mt-1 tabular-nums text-ink ${
            active.awaiting_approval > 0 ? 'text-3xl font-semibold' : 'text-2xl'
          }`}
        >
          {active.awaiting_approval}
        </div>
      </TileShell>

      <TileShell testId="tile-paused" weighted={active.paused > 0} weightVar="--c-status-paused">
        <TileLabel>Paused</TileLabel>
        <div
          className={`mt-1 tabular-nums text-ink ${
            active.paused > 0 ? 'text-3xl font-semibold' : 'text-2xl'
          }`}
        >
          {active.paused}
        </div>
      </TileShell>

      <TileShell testId="tile-failed" weighted={failedTotal > 0} weightVar="--c-status-failed">
        <TileLabel>Failed</TileLabel>
        <div className="mt-1 text-2xl tabular-nums text-ink">{failedTotal}</div>
        {failedSeries.length > 0 && (
          <div className="mt-1 h-8 w-full">
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={failedSeries} margin={{ top: 0, right: 0, bottom: 0, left: 0 }}>
                <Area
                  type="monotone"
                  dataKey="failed"
                  stroke={colors.get('status.failed')}
                  fill={colors.get('status.failed')}
                  fillOpacity={0.25}
                  // Same reasoning as TerminalTrend: no animation, liveness is
                  // per-transport and an animating sparkline implies a
                  // smoothness the SSH hosts do not have.
                  isAnimationActive={false}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        )}
      </TileShell>

      <TileShell testId="tile-success-rate" weighted={false} weightVar="--c-border">
        <TileLabel>Success rate</TileLabel>
        <div className="mt-1 text-2xl tabular-nums text-ink">
          {successRate === null ? '—' : `${successRate}%`}
        </div>
      </TileShell>

      <Link
        to="/runs"
        data-testid="tile-active-now"
        className="rounded-lg border border-border bg-panel p-3 hover:bg-surface-hover"
      >
        <TileLabel>Active now</TileLabel>
        <div className="mt-1 tabular-nums text-ink">
          <span className="text-2xl">{active.running}</span>
          {activeLongest && (
            <span className="ml-1 text-sm font-normal text-ink-dim">
              · longest {formatDuration(activeLongest.age_ms)} — {activeLongest.workflow_name}
            </span>
          )}
        </div>
      </Link>

      <Link
        to="/findings"
        data-testid="tile-findings"
        className="rounded-lg border border-border bg-panel p-3 hover:bg-surface-hover"
      >
        <TileLabel>
          Open findings
          {findingsPartial && (
            <span title="Some reporting hosts do not supply a findings count — this is a partial sum, not a fleet total.">
              {' '}
              (partial)
            </span>
          )}
        </TileLabel>
        <div className="mt-1 text-2xl tabular-nums text-ink">
          {/* `null` means nobody reported. "—" not "0": unknown is not none. */}
          {findingsOpen === null ? '—' : `${findingsOpen}${findingsPartial ? '+' : ''}`}
        </div>
      </Link>
    </div>
  );
}

export default KeyPointTiles;
