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

function age(capturedAt: string, now: number): string {
  const ms = now - Date.parse(capturedAt);
  if (ms < LIVE_THRESHOLD_MS) return 'live';
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  return `${Math.round(ms / 3_600_000)}h`;
}

export function HostFreshnessStrip({ hosts }: { hosts: HostFreshness[] }) {
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
          h.state === 'ok' && h.captured_at ? age(h.captured_at, now) : h.state;
        const isLive = label === 'live';
        const tone =
          h.state === 'ok'
            ? isLive
              ? 'bg-[rgb(var(--c-status-running))]'
              : 'bg-[rgb(var(--c-status-pending))]'
            : 'bg-[rgb(var(--c-status-failed))]';
        return (
          <span
            key={h.host_id}
            className="inline-flex items-center gap-1.5 text-[rgb(var(--c-ink-dim))]"
            title={h.reason ?? `${h.transport_kind} host`}
          >
            <span className={`h-1.5 w-1.5 rounded-full ${tone}`} aria-hidden />
            <span className="font-medium text-[rgb(var(--c-ink))]">{h.name}</span>
            <span className="text-[rgb(var(--c-ink-mute))]">·</span>
            <span>{label}</span>
          </span>
        );
      })}
    </div>
  );
}
