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
  const suffix = missing ? ` · not included: ${missing}` : '';
  // A host whose older rows could not load has not ended: no "end of" claim (PagingFailures says why).
  if (pagingFailedHosts(p.slices).length) return `${p.count} loaded${suffix}`;
  return `— end of ${p.count} —${suffix}`;
}

/**
 * The sentinel line plus per-host paging failures under a per-host list. Pages keep it mounted
 * whenever rows have loaded, even while their own filters or Find hide every one of them, so
 * scrolling keeps loading (a later page may match) and the user sees what is still coming.
 */
export function PerHostFooter<T>({
  sentinelRef,
  text,
  slices,
  onRetry,
}: {
  sentinelRef: (el: HTMLDivElement | null) => void;
  text: string;
  slices: HostSlice<T>[];
  onRetry: (hostId: string) => void;
}) {
  return (
    <>
      <div ref={sentinelRef} className="py-2 text-center text-note text-ink-mute">
        {text}
      </div>
      <PagingFailures slices={slices} onRetry={onRetry} />
    </>
  );
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
