// Honest per-host state for list pages (spec §8): failure classification,
// "waiting on …", "not included: …", and the freshness-strip mapping.

import { ApiError, apiErrorMessage } from '../api';
import type { HostFreshnessEntry } from '../../components/dashboard/HostFreshnessStrip';
import type { HostSlice } from './types';

export type Failure = { kind: 'offline' | 'unavailable' | 'gone'; reason: string };

/**
 * The single-host list paths answer 501 when a host cannot serve the
 * listing (e.g. an old remote rupu), 404 when the host is no longer
 * registered, and 502 when it gave no usable answer (spec §7.1). Anything
 * else, including a network error, is treated as offline.
 */
export function classifyFailure(e: unknown): Failure {
  const reason = apiErrorMessage(e);
  if (e instanceof ApiError) {
    if (e.status === 501) return { kind: 'unavailable', reason };
    if (e.status === 404) return { kind: 'gone', reason };
  }
  return { kind: 'offline', reason };
}

export function waitingOn<T>(slices: readonly HostSlice<T>[]): string[] {
  return slices.filter((s) => s.state === 'loading' || s.catchingUp).map((s) => s.name);
}

export function waitingLabel<T>(slices: readonly HostSlice<T>[]): string | null {
  const names = waitingOn(slices);
  return names.length ? `Waiting on ${names.join(', ')}…` : null;
}

export function notIncluded<T>(slices: readonly HostSlice<T>[]): string | null {
  const out = slices
    .filter((s) => s.state === 'offline' || s.state === 'unavailable')
    .map((s) => `${s.name} (${s.state})`);
  return out.length ? out.join(', ') : null;
}

/** No host answered: every one is offline or unavailable. Empty states must not say "the hosts that answered". */
export function noHostAnswered<T>(slices: readonly HostSlice<T>[]): boolean {
  return slices.length > 0 && slices.every((s) => s.state === 'offline' || s.state === 'unavailable');
}

export function pagingFailedHosts<T>(slices: readonly HostSlice<T>[]): { hostId: string; name: string }[] {
  return slices.filter((s) => s.pagingFailed).map((s) => ({ hostId: s.hostId, name: s.name }));
}

/**
 * Feed for `HostFreshnessStrip`. A catching-up host is still filling in, so
 * it shows as loading. Age is measured from when this browser received the
 * host's answer.
 */
export function toFreshnessEntries<T>(slices: readonly HostSlice<T>[]): HostFreshnessEntry[] {
  return slices.map((s) => ({
    host_id: s.hostId,
    name: s.name,
    transport_kind: s.transportKind,
    state: s.catchingUp ? 'loading' : s.state,
    captured_at: s.state === 'ok' && s.receivedAt != null ? new Date(s.receivedAt).toISOString() : null,
    reason: s.reason,
  }));
}
