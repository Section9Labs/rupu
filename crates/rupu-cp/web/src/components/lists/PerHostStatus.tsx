// Per-host list chrome (spec §6.4 / §8): the freshness strip, the honest
// sentinel footer, and per-host "older rows couldn't load · Retry".

import { HostFreshnessStrip } from '../dashboard/HostFreshnessStrip';
import type { HostSlice } from '../../lib/perHost/types';
import { notIncluded, pagingFailedHosts, toFreshnessEntries, waitingOn } from '../../lib/perHost/status';

/** Per-host freshness for an All-hosts list. Nothing for a single host. */
export function PerHostStrip<T>({ slices }: { slices: HostSlice<T>[] }) {
  if (slices.length < 2) return null;
  return (
    <div className="mb-3">
      <HostFreshnessStrip hosts={toFreshnessEntries(slices)} />
    </div>
  );
}

/** The sentinel line under a per-host list (`count` = rows the page shows after its own filters). */
export function perHostFooterText<T>(p: {
  slices: HostSlice<T>[];
  loading: boolean;
  hasMore: boolean;
  ended: boolean;
  count: number;
}): string {
  if (p.loading) return 'loading more…';
  if (p.hasMore) return 'scroll for more';
  const waiting = waitingOn(p.slices);
  if (!p.ended && waiting.length) return `waiting on ${waiting.join(', ')}…`;
  const missing = notIncluded(p.slices);
  return `— end of ${p.count} —${missing ? ` · not included: ${missing}` : ''}`;
}

export function PagingFailures<T>({ slices, onRetry }: { slices: HostSlice<T>[]; onRetry: (hostId: string) => void }) {
  const failed = pagingFailedHosts(slices);
  if (failed.length === 0) return null;
  return (
    <div className="py-1 text-center text-note text-status-failed">
      {failed.map((h) => (
        <span key={h.hostId} className="mr-3">
          older rows from {h.name} couldn't load ·{' '}
          <button type="button" className="underline" aria-label={`Retry ${h.name}`} onClick={() => onRetry(h.hostId)}>
            Retry
          </button>
        </span>
      ))}
    </div>
  );
}
