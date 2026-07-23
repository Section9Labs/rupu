// HostFreshnessStrip — per-host truth about how current this data is.
//
// One global "live" pill would lie about the SSH host. Liveness is
// per-transport (spec §5.4): local and HTTP hosts are sub-second via SSE, SSH
// and Bucket are poll-bounded, Tunnel/Bucket may not report at all. So each
// host carries its own freshness.
//
// This is also the host-status view rupu lacks entirely today.

import { useEffect, useState } from 'react';
import type { HostFreshness } from '../../lib/api';

/** Under this, a host reads as "live" rather than showing an age. */
const LIVE_THRESHOLD_MS = 5_000;

/**
 * The wire `HostFreshness.state` (`ok` | `offline` | `unavailable`) is only
 * three-valued because the SERVER never reports a host until it has already
 * resolved. `useDashboardData`'s per-host state is FOUR-valued: it seeds every
 * registered host as `loading` the instant the host list is known (so the
 * strip can render immediately), before that host's own `getDashboard` call
 * resolves to `ok`/`unavailable`. This type widens `state` to accept that
 * fourth value so the strip can render it distinctly — never folded into
 * `unavailable` (reads as dead) or `ok` (lies about freshness).
 */
export interface HostFreshnessEntry extends Omit<HostFreshness, 'state'> {
  state: HostFreshness['state'] | 'loading';
}

function age(capturedAt: string, now: number): string {
  const ms = now - Date.parse(capturedAt);
  if (ms < LIVE_THRESHOLD_MS) return 'live';
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  return `${Math.round(ms / 3_600_000)}h`;
}

export function HostFreshnessStrip({ hosts }: { hosts: HostFreshnessEntry[] }) {
  // Ticks so ages advance between refetches.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
      {hosts.map((h) => {
        const label =
          h.state === 'loading'
            ? 'loading…'
            : h.state === 'ok' && h.captured_at
              ? age(h.captured_at, now)
              : h.state;
        const isLive = label === 'live';
        const tone =
          h.state === 'loading'
            ? // Pulsing dot: this host has never actually reported, so it must
              // not read as the same "known-stale" gray a resolved-but-not-live
              // host gets below.
              'bg-status-pending animate-pulse'
            : h.state === 'ok'
              ? isLive
                ? 'bg-status-running'
                : 'bg-status-pending'
              : 'bg-status-failed';
        return (
          <span
            key={h.host_id}
            className="inline-flex items-center gap-1.5 text-ink-dim"
            title={h.reason ?? `${h.transport_kind} host`}
          >
            <span className={`h-1.5 w-1.5 rounded-full ${tone}`} aria-hidden />
            <span className="font-medium text-ink">{h.name}</span>
            <span className="text-ink-mute">·</span>
            <span>{label}</span>
          </span>
        );
      })}
    </div>
  );
}
