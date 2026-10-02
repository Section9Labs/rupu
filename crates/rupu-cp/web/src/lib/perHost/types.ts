// Per-host list loading — the slice model (spec
// docs/superpowers/specs/2026-10-01-rupu-cp-progressive-per-host-loading-design.md §6.1).
//
// One slice per host. A list page fires one request per host and merges the
// slices client-side (watermarkMerge.ts), so a slow or dead host never holds
// back anyone else's rows.

export type HostState = 'loading' | 'ok' | 'offline' | 'unavailable';

/** Every row this machinery merges may carry a server-tagged `host_id`. */
export interface HostTagged {
  host_id?: string;
}

export interface HostSlice<T> {
  hostId: string;
  name: string;
  transportKind: string;
  state: HostState;
  /** Newest-first by `timeOf`, de-duplicated by `keyOf`. */
  rows: T[];
  hasMore: boolean;
  /** A late host fetching up to the floor before it gates (spec §6.3). */
  catchingUp: boolean;
  /** A next-page or catch-up request for this host failed. */
  pagingFailed: boolean;
  /** Why the host is not `ok`, or why its last refresh failed (stale). */
  reason: string | null;
  /** Epoch ms of this host's last successful answer. */
  receivedAt: number | null;
}

export interface RowAccess<T> {
  /** Sort instant (epoch ms); `-Infinity` for a missing/unparseable stamp. */
  timeOf: (row: T) => number;
  /** Merge identity: the row's own `host_id` (server-tagged), else the slice's. */
  keyOf: (row: T, sliceHostId: string) => string;
  idOf: (row: T) => string;
}

/** Parse an RFC-3339 stamp to epoch ms. A missing or unparseable one sorts oldest. */
export function instantOf(v: unknown): number {
  if (typeof v !== 'string' || v === '') return -Infinity;
  const t = Date.parse(v);
  return Number.isNaN(t) ? -Infinity : t;
}

export function rowAccess<T extends HostTagged>(
  timeField: keyof T & string,
  idField: keyof T & string,
): RowAccess<T> {
  return {
    timeOf: (row) => instantOf(row[timeField]),
    keyOf: (row, sliceHostId) => `${row.host_id || sliceHostId}\u0000${String(row[idField])}`,
    idOf: (row) => String(row[idField]),
  };
}

/** The minimum a slice needs to know about its host. `RegisteredHostView` fits. */
export interface HostSeed {
  id: string;
  name: string;
  transport_kind: string;
}

export function emptySlice<T>(h: HostSeed): HostSlice<T> {
  return {
    hostId: h.id,
    name: h.name,
    transportKind: h.transport_kind,
    state: 'loading',
    rows: [],
    hasMore: true,
    catchingUp: false,
    pagingFailed: false,
    reason: null,
    receivedAt: null,
  };
}
