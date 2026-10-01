// Per-host slice maintenance: appending a page, and splicing a refreshed
// page 0 over a host's existing rows (spec §6.3 "Refresh merge rule").
//
// Replaces usePagedList's `[...page0, ...rows.slice(page0.length)]`, which
// dropped the boundary row when one new run arrived at the top, and
// duplicated a row when a run left the list (e.g. finished and left the
// Running tab).

import type { HostTagged, RowAccess } from './types';
import { compareNewestFirst } from './watermarkMerge';

/** Tag rows from an older server that does not set `host_id`. */
export function withHost<T extends HostTagged>(rows: readonly T[], hostId: string): T[] {
  return rows.map((r) => (r.host_id ? r : { ...r, host_id: hostId }));
}

/** Newest first, de-duplicated by key. The FIRST copy of a key wins. */
export function sortSlice<T>(rows: readonly T[], access: RowAccess<T>, hostId: string): T[] {
  const seen = new Set<string>();
  const entries: { row: T; t: number; key: string }[] = [];
  for (const row of rows) {
    const key = access.keyOf(row, hostId);
    if (seen.has(key)) continue;
    seen.add(key);
    entries.push({ row, t: access.timeOf(row), key });
  }
  entries.sort(compareNewestFirst);
  return entries.map((e) => e.row);
}

/** Add a fetched page. The page's copy of an overlapping row wins (fresher status). */
export function appendPage<T>(old: readonly T[], page: readonly T[], access: RowAccess<T>, hostId: string): T[] {
  return sortSlice([...page, ...old], access, hostId);
}

export interface SpliceResult<T> {
  rows: T[];
  /** Page 0 came back short: it is this host's whole list. */
  fullyListed: boolean;
}

/**
 * Splice a fresh page 0 (requested with `limit`) over a host's rows. Within
 * the span the page covers, it is authoritative: old rows newer than its
 * oldest row that it no longer contains have left the list. Older rows are
 * kept. A row exactly at the cutoff is kept so a timestamp tie is never lost.
 */
export function spliceHead<T>(
  old: readonly T[],
  fresh: readonly T[],
  limit: number,
  access: RowAccess<T>,
  hostId: string,
): SpliceResult<T> {
  if (fresh.length < limit) return { rows: sortSlice(fresh, access, hostId), fullyListed: true };
  let cutoff = Infinity;
  for (const r of fresh) cutoff = Math.min(cutoff, access.timeOf(r));
  const freshKeys = new Set(fresh.map((r) => access.keyOf(r, hostId)));
  const kept = old.filter((o) => access.timeOf(o) <= cutoff && !freshKeys.has(access.keyOf(o, hostId)));
  return { rows: sortSlice([...fresh, ...kept], access, hostId), fullyListed: false };
}
