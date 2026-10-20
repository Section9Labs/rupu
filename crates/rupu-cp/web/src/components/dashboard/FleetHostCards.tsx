// FleetHostCards — the fleet health band as cards (dashboard-redesign.html).
// One card per host: a state dot, the mono host name, a one-line state, and the
// freshness age. Reads the per-host state `useDashboardData` already tracks
// (`DashboardHostState`); rendering only. The dotted `HostFreshnessStrip` stays
// for its other callers — this is the dashboard's richer presentation.

import { Link } from 'react-router-dom';
import { Dot, type DotColor } from '../ui/Dot';
import type { DashboardHostState } from '../../lib/dashboard/useDashboardData';

/** Compact freshness age ("8s" / "6m" / "2h"); em-dash when unknown. */
function ageLabel(capturedAt: string | null | undefined, now: number): string {
  if (!capturedAt) return '—';
  const ms = now - Date.parse(capturedAt);
  if (!Number.isFinite(ms) || ms < 0) return '—';
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

interface HostView {
  dot: DotColor;
  state: string;
  title?: string;
}

function viewOf(h: DashboardHostState): HostView {
  if (h.state === 'loading') return { dot: 'mute', state: 'loading…' };
  if (h.state === 'offline') return { dot: 'failed', state: 'offline', title: h.reason ?? undefined };
  if (h.state === 'unavailable') return { dot: 'failed', state: 'unavailable', title: h.reason ?? undefined };
  // ok
  const running = h.summary?.active.running ?? 0;
  if (running > 0) {
    return { dot: 'running', state: `live · ${running} run${running === 1 ? '' : 's'}` };
  }
  return { dot: 'done', state: 'idle' };
}

export function FleetHostCards({ hosts }: { hosts: DashboardHostState[] }) {
  const now = Date.now();
  if (hosts.length === 0) return null;

  return (
    <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-3">
      {hosts.map((h) => {
        const v = viewOf(h);
        return (
          <Link
            key={h.hostId}
            to={`/hosts`}
            data-testid={`host-${h.hostId}`}
            className="flex items-center gap-2.5 rounded-lg border border-border bg-panel px-3.5 py-3 hover:bg-surface"
          >
            <Dot color={v.dot} />
            <div className="min-w-0">
              <div className="truncate font-mono text-ui font-semibold text-ink">{h.name}</div>
              <div className="text-note text-ink-dim" title={v.title}>
                {v.state}
              </div>
            </div>
            <span className="flex-1" />
            <span className="shrink-0 text-note tabular-nums text-ink-dim">
              {ageLabel(h.summary?.captured_at, now)}
            </span>
          </Link>
        );
      })}
    </div>
  );
}

export default FleetHostCards;
