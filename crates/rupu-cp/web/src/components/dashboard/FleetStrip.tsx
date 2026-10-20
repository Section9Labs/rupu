// FleetStrip — the fleet's health in one compact, scannable line instead of a
// grid of tiles. It answers one thing at a glance: what's alive and what's not.
// A leading summary (N live · K down) then a wrapping row of small host chips,
// each a status dot + mono name, sorted active-first: a running host shows its
// run count (blue), an idle-but-alive host a green dot, a down host a red dot
// with a dimmed name. Freshness and the down-reason live in the hover title, not
// as visible chrome — the age-per-card was noise. Reads the per-host state
// `useDashboardData` already tracks; rendering only, each chip links to /hosts.

import { Link } from 'react-router-dom';
import { Dot, type DotColor } from '../ui/Dot';
import type { DashboardHostState } from '../../lib/dashboard/useDashboardData';

type Health = 'running' | 'idle' | 'loading' | 'down';

interface HostView {
  health: Health;
  dot: DotColor;
  running: number;
  /** Hover detail: the down reason, or the freshness age. */
  title?: string;
}

/** Compact freshness age ("8s" / "6m" / "2h"); em-dash when unknown. */
function ageLabel(capturedAt: string | null | undefined, now: number): string {
  if (!capturedAt) return 'no data yet';
  const ms = now - Date.parse(capturedAt);
  if (!Number.isFinite(ms) || ms < 0) return 'no data yet';
  const s = Math.floor(ms / 1000);
  if (s < 60) return `updated ${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `updated ${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `updated ${h}h ago`;
  return `updated ${Math.floor(h / 24)}d ago`;
}

function viewOf(h: DashboardHostState, now: number): HostView {
  if (h.state === 'loading') return { health: 'loading', dot: 'mute', running: 0, title: 'loading…' };
  if (h.state === 'offline' || h.state === 'unavailable') {
    return { health: 'down', dot: 'failed', running: 0, title: h.reason ?? h.state };
  }
  const running = h.summary?.active.running ?? 0;
  return {
    health: running > 0 ? 'running' : 'idle',
    dot: running > 0 ? 'running' : 'done',
    running,
    title: ageLabel(h.summary?.captured_at, now),
  };
}

const ORDER: Record<Health, number> = { running: 0, idle: 1, loading: 2, down: 3 };

export function FleetStrip({ hosts }: { hosts: DashboardHostState[] }) {
  const now = Date.now();
  if (hosts.length === 0) return null;

  const views = hosts
    .map((h) => ({ h, v: viewOf(h, now) }))
    .sort((a, b) => ORDER[a.v.health] - ORDER[b.v.health] || b.v.running - a.v.running);

  const live = views.filter((x) => x.v.health === 'running' || x.v.health === 'idle').length;
  const down = views.filter((x) => x.v.health === 'down').length;
  const loading = views.filter((x) => x.v.health === 'loading').length;

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border border-border bg-panel px-3.5 py-2.5">
      {/* summary: alive vs not */}
      <span className="flex shrink-0 items-center gap-1.5 text-note">
        <Dot color={down > 0 ? 'failed' : 'done'} />
        <span className="font-semibold text-ink">
          <span className="tabular-nums">{live}</span> of{' '}
          <span className="tabular-nums">{hosts.length}</span> live
        </span>
        {down > 0 && (
          <span className="font-medium text-status-failed">
            · <span className="tabular-nums">{down}</span> down
          </span>
        )}
        {loading > 0 && (
          <span className="text-ink-mute">
            · <span className="tabular-nums">{loading}</span> loading
          </span>
        )}
      </span>

      <span className="h-4 w-px shrink-0 bg-border" aria-hidden />

      {/* host chips, active first */}
      <div className="flex flex-wrap items-center gap-x-1.5 gap-y-1.5">
        {views.map(({ h, v }) => (
          <Link
            key={h.hostId}
            to="/hosts"
            data-testid={`host-${h.hostId}`}
            title={v.title}
            className="inline-flex items-center gap-1.5 rounded-full border border-border px-2 py-0.5 transition-colors hover:bg-surface"
          >
            <Dot color={v.dot} />
            <span
              className={`font-mono text-note ${v.health === 'down' ? 'text-ink-mute' : 'text-ink'}`}
            >
              {h.name}
            </span>
            {v.running > 0 && (
              <span className="tabular-nums text-note font-semibold text-status-running">
                {v.running}
              </span>
            )}
          </Link>
        ))}
      </div>
    </div>
  );
}

export default FleetStrip;
